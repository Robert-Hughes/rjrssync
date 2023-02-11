use aes_gcm::aead::generic_array::GenericArray;

use clap::Parser;
use env_logger::Env;
use log::{debug, error, trace, info};
use std::io::{ErrorKind, Read};
use std::path;
use std::{
    fmt::{self, Display},
    io::{Write},
    path::{Path, PathBuf},
    time::{Instant}, net::{TcpListener},
};

use crate::*;
use crate::boss_doer_interface::{EntryDetails, SymlinkTarget, Response, Command, SymlinkKind, Filters, FilterKind, HANDSHAKE_STARTED_MSG, HANDSHAKE_COMPLETED_MSG};
use crate::encrypted_comms::AsyncEncryptedComms;
use crate::memory_bound_channel::{Sender, Receiver};
use crate::parallel_walk_dir::parallel_walk_dir;
use crate::root_relative_path::RootRelativePath;

#[derive(clap::Parser)]
struct DoerCliArgs {
    /// [Internal] Launches as a doer process, rather than a boss process.
    /// This shouldn't be needed for regular operation.
    #[arg(long)]
    doer: bool,
    /// The network port to listen on for a connection from the boss.
    /// If not specified, a free port is chosen.
    #[arg(long)]
    port: Option<u16>,
    /// Logging configuration.
    #[arg(long, default_value="info")]
    log_filter: String,
    #[arg(long)]
    dump_memory_usage: bool,
}

fn entry_details_from_metadata(m: std::fs::Metadata, path: &Path) -> Result<EntryDetails, String> {
    if m.is_dir() {
        Ok(EntryDetails::Folder)
    } else if m.is_file() {
        let modified_time = match m.modified() {
            Ok(m) => m,
            Err(err) => return Err(format!("Unknown modified time for '{}': {err}", path.display())),
        };

        Ok(EntryDetails::File {
            modified_time,
            size: m.len(),
        })
    } else if m.is_symlink() {
        let target = match std::fs::read_link(path) {
            Ok(t) => t,
            Err(err) => return Err(format!("Unable to read symlink target for '{}': {err}", path.display())),
        };

        // Attempt to normalize the target, if possible, so that we can convert the slashes on
        // the destination platform (which might be different).
        // We use RootRelativePath for this even though it might not be root-relative, but this does the right thing
        let target = match RootRelativePath::try_from(&target as &Path) {
            Ok(r) => SymlinkTarget::Normalized(r.to_string()),
            Err(_) => SymlinkTarget::NotNormalized(target.to_string_lossy().to_string()),
        };

        // On Windows, symlinks are either file-symlinks or dir-symlinks
        #[cfg(windows)]
        let kind = {
            if std::os::windows::fs::FileTypeExt::is_symlink_file(&m.file_type()) {
                SymlinkKind::File
            } else if std::os::windows::fs::FileTypeExt::is_symlink_dir(&m.file_type()) {
                SymlinkKind::Folder
            } else {
                return Err(format!("Unknown symlink type time for '{}'", path.display()));
            }
        };
        // On Linux, all symlinks are created equal. In case we need to recreate this symlink on a Windows platform though,
        // we need to figure out what it's pointing to.
        #[cfg(not(windows))]
        let kind = {
            // Use the symlink-following metadata API
            match std::fs::metadata(path) {
                Ok(m) if m.is_file() => SymlinkKind::File,
                Ok(m) if m.is_dir() => SymlinkKind::Folder,
                _ => SymlinkKind::Unknown
            }
        };

        Ok(EntryDetails::Symlink { kind, target })
    } else {
        return Err(format!("Unknown file type for '{}': {:?}", path.display(), m));
    }
}


/// Abstraction of two-way communication channel between this doer and the boss, which might be
/// remote (communicating over an encrypted TCP connection) or local (communicating via a channel to the main thread).
#[allow(clippy::large_enum_variant)]
enum Comms {
    Local {
        sender: Sender<Response>,
        receiver: Receiver<Command>,
    },
    Remote {
        encrypted_comms: AsyncEncryptedComms<Response, Command>,
    },
}
impl Comms {
    /// This will block if there is not enough capacity in the channel, so
    /// that we don't use up infinite memory if the boss is being slow.
    pub fn send_response(&mut self, r: Response) -> Result<(), String> {
        trace!("Sending response {:?} to {}", r, &self);
        let sender = match self {
            Comms::Local { sender, .. } => sender,
            Comms::Remote { encrypted_comms, .. } => &mut encrypted_comms.sender,
        };
        sender.send(r).map_err(|_| format!("Lost communication with {}", &self))
    }

    /// Blocks until a command is received. If the channel is closed (i.e. the boss has disconnected),
    /// then returns Err. Note that normally the boss should send us a Shutdown command rather than
    /// just disconnecting, but in the case of errors, this may not happen so we want to deal with this
    /// cleanly too.
    pub fn receive_command(&mut self) -> Result<Command, String> {
        trace!("Waiting for command from {}", &self);
        let receiver = match self {
            Comms::Local { receiver, .. } => receiver,
            Comms::Remote { encrypted_comms, .. } => &mut encrypted_comms.receiver,
        };
        receiver.recv().map_err(|_| format!("Lost communication with {}", &self))
    }
}
impl Display for Comms {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Comms::Local { .. } => write!(f, "Local boss"),
            Comms::Remote { .. } => write!(f, "Remote boss"),
        }
    }
}

pub fn doer_main() -> ExitCode {
    let main_timer = start_timer(function_name!());

    // The first thing we send is a special handshake message that the Boss will recognise,
    // to know that we've started up correctly and to make sure we are running compatible versions.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    // Note that this needs to be done even before parsing cmd line args, because the cmd line args interface might change
    // (e.g. adding a new required parameter), then we wouldn't be able to launch the doer, and users
    // will be forced to do a --deploy=force which isn't very nice.
    let msg = format!("{}{}", HANDSHAKE_STARTED_MSG, boss_doer_interface::get_version_string());
    println!("{}", msg);
    eprintln!("{}", msg);

    let args = DoerCliArgs::parse();

    {
        profile_this!("Configuring logging");
        // Configure logging.
        // Because the doer is launched via SSH, and on Windows there isn't an easy way of setting the
        // RUST_LOG environment variable, we support configuring logging via a command-line arg, passed
        // from the boss.
        // Note that we can't use stdout as that is our communication channel with the boss.
        // We use stderr instead, which the boss will read from and echo for easier debugging.
        let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or(args.log_filter));
        builder.target(env_logger::Target::Stderr);
        // Configure format so that the boss can parse and re-log it
        builder.format(|buf, record| {
            writeln!(
                buf,
                "{} {} {} {}",
                buf.timestamp_nanos(),
                record.level(),
                record.target(),
                record.args()
            )
        });
        builder.init();
    }

    let timer = start_timer("Handshaking");


    // If the Boss isn't happy (e.g. we are an old version), they will stop us and deploy a new version.
    // So at this point we can assume they are happy and set up the network connection.
    // We use a separate network connection for data transfer as it is faster than using stdin/stdout over ssh.

    // In order to make sure that incoming network connection is in fact the boss,
    // we first receive a secret (shared) key over stdin which we will use to authenticate/encrypt
    // the TCP connection. This exchange is secure because stdin/stdout is run over ssh.
    let mut secret = String::new();
    if let Err(e) = std::io::stdin().read_line(&mut secret) {
        error!("Failed to receive secret: {}", e);
        return ExitCode::from(22);
    }
    secret.pop(); // remove trailing newline

    // The key is 16 bytes, so we can use u128 to parse the hex string.
    let secret_bytes = match u128::from_str_radix(&secret, 16) {
        Ok(b) => b.to_be_bytes(), // Big-endian because this the string formatting on the boss places most-significant bytes first
        Err(e) => {
            error!("Failed to decode secret: {}", e);
            return ExitCode::from(23);
        }
    };
    let secret_key = GenericArray::from_slice(&secret_bytes);

    // Start listening on the requested port, or 0 (automatic).
    // Automatic is better as we don't know which ones might be free, and we might have more than one doer
    // running on the same device, which would then need different ports.
    // It also reduces issues if we ever leave behind orphaned doer instances which would otherwise block us
    // from using that port.
    // Listen on all interfaces as we don't know which one is needed.
    let addr = ("0.0.0.0", args.port.unwrap_or(0));
    let listener = match TcpListener::bind(addr) {
        Ok(l) => {
            debug!("Listening on {:?}", l.local_addr()); // This will include the actual port chosen, if we bound to 0
            l
        }
        Err(e) => {
            error!("Failed to bind to {:?}: {}", addr, e);
            return ExitCode::from(24);
        }
    };

    // Let the boss know that we are ready for the network connection,
    // and tell them which port to connect on (we may have chosen automatically).
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    let msg = format!("{}{}", HANDSHAKE_COMPLETED_MSG, listener.local_addr().unwrap().port());
    println!("{}", msg);
    eprintln!("{}", msg);

    stop_timer(timer);

    let timer = start_timer("Waiting for connection");

    // Spawn a thread to keep track of our stdin, to check if the boss disconnects.
    // This is particularly useful before the boss connects via TCP, as if there is e.g. a firewall
    // issue then we could be stuck waiting forever, and would never exit as we would never detect
    // that the ssh connection has dropped (because in non-interactive sessions, ssh won't terminate
    // the spawned process when the connection drops, it will just close its stdin/out, so we need to be
    // reading or writing from them to detect this).
    // Remaining alive forever causes problems (e.g. can't deploy new version)
    std::thread::spawn(stdin_reading_thread);

    // Wait for a connection from the boss
    let tcp_connection = match listener.accept() {
        Ok((socket, addr)) => {
            debug!("Client connected: {socket:?} {addr:?}");
            socket
        }
        Err(e) => {
            error!("Failed to accept: {}", e);
            return ExitCode::from(25);
        }
    };

    stop_timer(timer);

    // Start command processing loop, receiving commands and sending responses over the TCP connection, with encryption
    // so that we know it's the boss.
    let mut comms = Comms::Remote {
        encrypted_comms: AsyncEncryptedComms::new(
            tcp_connection,
            *secret_key,
            1, // Nonce counters must be different, so sender and receiver don't reuse
            0,
            ("doer", "remote boss"),
    )};

    if let Err(e) = message_loop(&mut comms) {
        debug!("doer process finished with error: {:?}", e);
        return ExitCode::from(20)
    }

    stop_timer(main_timer);

    if let Comms::Remote{ encrypted_comms } = comms { // This is always true, we just need a way of getting the fields
        // Send our profiling data (if enabled) back to the boss process so it can combine it with its own
        encrypted_comms.shutdown_with_final_message_sent_after_threads_joined(|| Response::ProfilingData(get_local_process_profiling()));
    }

    // Dump memory usage figures when used for benchmarking. There isn't a good way of determining this from the benchmarking app
    // (especially for remote processes), so we instrument it instead.
    if args.dump_memory_usage {
        info!("Doer peak memory usage: {}", profiling::get_peak_memory_usage());
    }

    debug!("doer process finished successfully!");
    ExitCode::SUCCESS
}

fn stdin_reading_thread() {
    loop {
        let mut l: String = "".to_string();
        match std::io::stdin().read_line(&mut l) {
            Ok(0) | Err(_) => {
                // Boss has disconnected prematurely - nothing we can do, just exit.
                error!("Boss disconnected from stdin - exiting process");
                std::process::exit(321);
            }
            Ok(_) => (), // We're not expecting to receive anything over stdin, so we just ignore it
        }
    }
}

// When the source and/or dest is local, the doer is run as a thread in the boss process,
// rather than over ssh.
pub fn doer_thread_running_on_boss(receiver: Receiver<Command>, sender: Sender<Response>) -> Result<(), String> {
    debug!("doer thread running");
    profile_this!();
    match message_loop(&mut Comms::Local { sender, receiver }) {
        Ok(_) => {
            debug!("doer thread finished successfully!");
            Ok(())
        }
        Err(e) => {
            error!("doer thread finished with error: {:?}", e);
            Err(format!("doer thread finished with error: {:?}", e))
        }
    }
}

/// Context for each doer instance. We can't use anything global (e.g. like changing the
/// process' current directory), because there might be multiple doer threads in the same process
/// (if these are local doers).
struct DoerContext {
    root: PathBuf,
    /// Stores details of a file we're partway through receiving.
    in_progress_file_receive: Option<(RootRelativePath, std::fs::File)>,
}

// Repeatedly waits for Commands from the boss and processes them (possibly sending back Responses).
// This function returns when we receive a Shutdown Command, or there is an unrecoverable error
// (recoverable errors while handling Commands will not stop the loop).
fn message_loop(comms: &mut Comms) -> Result<(), ()> {
    profile_this!();
    let mut context : Option<DoerContext> = None;
    loop {
        match comms.receive_command() {
            Ok(c) => {
                match exec_command(c, comms, &mut context) {
                    Ok(false) => {
                        debug!("Shutdown command received - finishing message_loop");
                        return Ok(());
                    }
                    Ok(true) => (), // Continue processing commands
                    Err(e) => {
                        error!("Error processing command: {}", e);
                        return Err(());
                    }
                }
            }
            Err(_) => {
                // Boss has disconnected
                debug!("Boss disconnected - finishing message loop");
                return Ok(());
            }
        }
    }
}

/// Handles a Command from the boss, possibly replying with one or more Responses.
/// Returns false if we received a Shutdown Command, otherwise true.
/// Note that if processing a command results in an error which is related to the command itself (e.g. we are asked
/// to fetch details of a file that doesn't exist), then this is reported back to the boss in a Response::Error,
/// and this function still returns Ok(). Error() variants returned from this function indicate a more catastrophic
/// error, like a communication failure.
fn exec_command(command: Command, comms: &mut Comms, context: &mut Option<DoerContext>) -> Result<bool, String> {
    match command {
        Command::SetRoot { root } => {
            if let Err(e) = handle_set_root(comms, context, root) {
                comms.send_response(Response::Error(e))?;
            }
        }
        Command::GetEntries { filters } => {
            profile_this!("GetEntries");
            if let Err(e) = handle_get_entries(comms, context.as_mut().unwrap(), filters) {
                comms.send_response(Response::Error(e))?;
            }
        }
        Command::CreateRootAncestors => {
            let path_to_create = context.as_ref().unwrap().root.parent();
            trace!("Creating {:?} and all its ancestors", path_to_create);
            if let Some(p) = path_to_create {
                profile_this!(format!("CreateRootAncestors {}", p.to_str().unwrap().to_string()));
                if let Err(e) = std::fs::create_dir_all(p) {
                    comms.send_response(Response::Error(format!("Error creating folder and ancestors for '{}': {e}", p.display())))?;
                }
            }
        }
        Command::GetFileContent { path } => {
            let full_path = path.get_full_path(&context.as_ref().unwrap().root);
            profile_this!(format!("GetFileContent {}", path.to_string()));
            if let Err(e) = handle_get_file_contents(comms, &full_path) {
                comms.send_response(Response::Error(e))?;
            }
        }
        Command::CreateOrUpdateFile {
            path,
            data,
            set_modified_time,
            more_to_follow
        } => {
            let full_path = path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Creating/updating content of '{}'", full_path.display());
            profile_this!(format!("CreateOrUpdateFile {}", path.to_string()));
        //    std::thread::sleep(std::time::Duration::from_nanos(1));

            // Check if this is the continuation of an existing file
            let mut f = match context.as_mut().unwrap().in_progress_file_receive.take() {
                Some((in_progress_path, f)) => {
                    if in_progress_path == path {
                        f
                    } else {
                        comms.send_response(Response::Error(format!("Unexpected continued file transfer!")))?;
                        return Ok(true);
                    }
                },
                None => match std::fs::File::create(&full_path) {
                    Ok(f) => f,
                    Err(e) => {
                        comms.send_response(Response::Error(format!("Error writing file contents to '{}': {e}", full_path.display())))?;
                        return Ok(true);
                    }
                }
            };

            let r = f.write_all(&data);
            if let Err(e) = r {
                comms.send_response(Response::Error(format!("Error writing file contents to '{}': {e}", full_path.display())))?;
                return Ok(true);
            }

            // If there is more data to follow, store the open file handle for next time
            context.as_mut().unwrap().in_progress_file_receive = if more_to_follow {
                Some((path, f))
            } else {
                None
            };

            // After changing the content, we need to override the modified time of the file to that of the original,
            // otherwise it will immediately count as modified again if we do another sync.
            if let Some(t) = set_modified_time {
                trace!("Setting modifited time of '{}'", full_path.display());
                let r =
                    filetime::set_file_mtime(&full_path, filetime::FileTime::from_system_time(t));
                if let Err(e) = r {
                    comms.send_response(Response::Error(format!("Error setting modified time of '{}': {e}", full_path.display())))?;
                    return Ok(true);
                }
            }
        }
        Command::CreateFolder { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Creating folder '{}'", full_path.display());
            profile_this!(format!("CreateFolder {}", full_path.to_str().unwrap().to_string()));
            if let Err(e) = std::fs::create_dir(&full_path) {
                comms.send_response(Response::Error(format!("Error creating folder '{}': {e}", full_path.display())))?;
            }
        }
        Command::CreateSymlink { path, kind, target } => {
            if let Err(e) = handle_create_symlink(path, context.as_mut().unwrap(), kind, target) {
                comms.send_response(Response::Error(e))?;
            }
        },
        Command::DeleteFile { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting file '{}'", full_path.display());
            profile_this!(format!("DeleteFile {}", path.to_string()));
            if let Err(e) = std::fs::remove_file(&full_path) {
                comms.send_response(Response::Error(format!("Error deleting file '{}': {e}", full_path.display())))?;
            }
        }
        Command::DeleteFolder { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting folder '{}'", full_path.display());
            profile_this!(format!("DeleteFolder {}", path.to_string()));
            if let Err(e) = std::fs::remove_dir(&full_path) {
                comms.send_response(Response::Error(format!("Error deleting folder '{}': {e}", full_path.display())))?;
            }
        }
        Command::DeleteSymlink { path, kind } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting symlink '{}'", full_path.display());
            let res = if cfg!(windows) {
                // On Windows, we need to use remove_dir/file depending on the kind of symlink
                match kind {
                    SymlinkKind::File => std::fs::remove_file(&full_path),
                    SymlinkKind::Folder => std::fs::remove_dir(&full_path),
                    // We should never be asked to delete an Unknown symlink on Windows, but just in case:
                    SymlinkKind::Unknown => {
                        comms.send_response(Response::Error(format!("Can't delete symlink of unknown type '{}'", full_path.display())))?;
                        return Ok(true);
                    }
                }
            } else {
                // On Linux, any kind of symlink is removed with remove_file
                std::fs::remove_file(&full_path)
            };
            if let Err(e) = res {
                comms.send_response(Response::Error(format!("Error deleting symlink '{}': {e}", full_path.display())))?;
            }
        },
        Command::ProfilingTimeSync => {
            comms.send_response(Response::ProfilingTimeSync(PROFILING_START.elapsed()))?;
        },
        Command::Marker(x) => {
            comms.send_response(Response::Marker(x))?;
        }
        Command::Shutdown => {
            return Ok(false);
        },
    }
    Ok(true)
}

fn handle_set_root(comms: &mut Comms, context: &mut Option<DoerContext>, root: String) -> Result<(), String> {
    // Store the root path for future operations
    *context = Some(DoerContext {
        root: PathBuf::from(root),
        in_progress_file_receive: None,
    });
    let context = context.as_ref().unwrap();

    let platform_differentiates_symlinks = cfg!(windows);
    let platform_dir_separator = std::path::MAIN_SEPARATOR;

    // Respond to the boss with what type of file/folder the root is, as it makes some decisions
    // based on this.
    // We use symlink_metadata so that we see the metadata of a symlink, not its target
    let metadata = std::fs::symlink_metadata(&context.root);
    match metadata {
        Ok(m) => {
            let entry_details = entry_details_from_metadata(m, &context.root)?;
            comms.send_response(Response::RootDetails { root_details: Some(entry_details), platform_differentiates_symlinks, platform_dir_separator })?;
        },
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Report this as a special error, as we handle it differently on the boss side
            comms.send_response(Response::RootDetails { root_details: None, platform_differentiates_symlinks, platform_dir_separator })?;
        }
        Err(e) => return Err(format!(
                    "root '{}' can't be read: {}", context.root.display(), e)),
    }

    Ok(())
}

#[derive(PartialEq, Debug)]
enum FilterResult {
    Include,
    Exclude
}

fn apply_filters(path: &RootRelativePath, filters: &Filters) -> FilterResult {
    if path.is_root() {
        // The root is always included, otherwise it would be difficult to write filter lists that start with include,
        // because you'd need to include the root (empty string) explicitly
        return FilterResult::Include;
    }

    // Depending on whether the first filter is include or exclude, the default state is the opposite
    let mut result = match filters.kinds.get(0) {
        Some(FilterKind::Include) => FilterResult::Exclude,
        Some(FilterKind::Exclude) => FilterResult::Include,
        None => FilterResult::Include
    };

    // Check for matches against all the filters using the RegexSet. This is more efficient than
    // testing each regex individually. This does however miss out on a potential optimisation where
    // we can avoid checking against an include filter if the current state is already include (and the
    // same for exclude), but hopefully using RegexSet is still faster (not been benchmarked).
    let matches = path.regex_set_matches(&filters.regex_set);

    // Now we go through the filters which matches, and work out the final include/exclude state
    for matched_filter_idx in matches {
        let filter_kind = filters.kinds[matched_filter_idx];
        match filter_kind {
            FilterKind::Include => result = FilterResult::Include,
            FilterKind::Exclude => result = FilterResult::Exclude,
        }
    }

    result
}

/// Filter callback used when iterating over directory contents.
fn filter_func(entry: &std::fs::DirEntry, root: &Path, filters: &Filters) -> Result<parallel_walk_dir::FilterResult<RootRelativePath>, String> {
    // First normalize the path to our platform-independent representation, so that the filters
    // apply equally well on both source and dest sides, if they are different platforms.

    // Paths returned by DirEntry will include the root, but we want paths relative to the root
    // The strip_prefix should always be successful, because the entry has to be inside the root.
    let path = entry.path().strip_prefix(root).expect("Strip prefix failed").to_path_buf();
    // Convert to platform-agnostic representation
    let path = match RootRelativePath::try_from(&path as &Path) {
        Ok(p) => p,
        Err(e) => return Err(format!("normalize_path failed on '{}': {e}", path.display())),
    };

    let skip = apply_filters(&path, &filters) == FilterResult::Exclude;
    if skip {
        trace!("Skipping '{}' due to filter", path);
    }
    // Store the normalized root-relative path so that we don't need to re-calculate this when we process
    // this entry
    Ok(parallel_walk_dir::FilterResult::<RootRelativePath> {
        skip,
        additional_data: path,
    })
}

fn handle_get_entries(comms: &mut Comms, context: &mut DoerContext, filters: Filters) -> Result<(), String> {
    let start = Instant::now();
    // Note that we can't use this to get metadata for a single root entry when that entry is a symlink,
    // as the iteration will fail before we can get the metadata for the root. Therefore we only use this
    // when walking what's known to be a directory (discovered in SetRoot).
    let root = context.root.clone();
    let entry_receiver = parallel_walk_dir(&context.root, move |e| filter_func(e, &root, &filters));
    let mut count = 0;
    while let Ok(entry) = entry_receiver.recv() {
        count += 1;
        match entry {
            Err(e) => return Err(format!("Error fetching entries of root '{}': {e}", context.root.display())),
            Ok(e) => {
                trace!("Processing entry {:?}", e);
                profile_this!("Processing entry");

                // The root-relative path was stored when this entry was tested against the filter,
                // so that we don't need to re-normalize it here.
                let path = e.additional_data;

                let metadata = match e.dir_entry.metadata() {
                    Ok(m) => m,
                    Err(err) => return Err(format!("Unable to get metadata for '{}': {err}", path)),
                };

                let d = entry_details_from_metadata(metadata, &e.dir_entry.path())?;

                comms.send_response(Response::Entry((path, d)))?;
            }
        }
    }

    let elapsed = start.elapsed().as_millis();
    comms.send_response(Response::EndOfEntries)?;
    debug!(
        "Walked {} in {}ms ({}/s)",
        count,
        elapsed,
        1000.0 * count as f32 / elapsed as f32
    );

    Ok(())
}

fn handle_get_file_contents(comms: &mut Comms, full_path: &Path) -> Result<(), String> {
    trace!("Getting content of '{}'", full_path.display());

    let mut f = match std::fs::File::open(&full_path) {
        Ok(f) => f,
        Err(e) => return Err(format!("Error opening file '{}': {e}", full_path.display())),
    };

    // Split large files into several chunks (see more_to_follow flag for more details)
    // Inspired somewhat by https://doc.rust-lang.org/src/std/io/mod.rs.html#358.
    // We don't know how big the file is so this algorithm tries to handle any size efficiently.
    // (We could find the size out beforehand but we'd have to either check the metadata (an extra filesystem call
    // that might slow things down) or use the metadata that we already retrieved, but we don't have a nice way of getting
    // that here).
    // Start with a small chunk size to minimize initialization overhead for small files,
    // but we'll increase this if the file is big
    let mut chunk_size = 4 * 1024;
    let mut prev_buf = vec![0; 0];
    let mut prev_buf_valid = 0;
    let mut next_buf = vec![0; chunk_size];
    loop {
        profile_this!("Read iteration");
        match f.read(&mut next_buf) {
            Ok(n) if n == 0 => {
                // End of file - send the data that we got previously, and report that there is no more data to follow.
                prev_buf.truncate(prev_buf_valid);
                comms.send_response(Response::FileContent { data: prev_buf, more_to_follow: false })?;
                return Ok(());
            },
            Ok(n) => {
                // Some data read - send any previously retrieved data, and report that there is more data to follow
                if prev_buf_valid > 0 {
                    prev_buf.truncate(prev_buf_valid);
                    comms.send_response(Response::FileContent { data: prev_buf, more_to_follow: true })?;
                }

                // The data we just retrieved will be sent in the next iteration (once we know if there is more data to follow or not)
                prev_buf = next_buf;
                prev_buf_valid = n;

                if n < prev_buf.len() {
                    // We probably just found the end of the file, but we can't be sure until we read() again and get zero,
                    // so allocate a small buffer instead of a big one for next time to minimize initialization overhead.
                    next_buf = vec![0; 32];
                } else {
                    // There might be lots more data, so gradually increase the chunk size up to a practical limit
                    // 4 MB, chosen pretty arbitirarily. If this changes, will also need to update the fixed size pre-allocated buffers in encrypted_comms.rs!
                    chunk_size = std::cmp::min(chunk_size * 2, 1024*1024*4);

                    next_buf = vec![0; chunk_size];
                }
            }
            Err(e) => return Err(format!("Error getting file content of '{}': {e}", full_path.display())),
        }
    }
}

fn handle_create_symlink(path: RootRelativePath, context: &mut DoerContext, #[allow(unused)] kind: SymlinkKind, target: SymlinkTarget) -> Result<(), String> {
    let full_path = path.get_full_path(&context.root);
    trace!("Creating symlink at '{}'", full_path.display());

    // Convert the normalized forwards slashes to backwards slashes if this is windows
    let target = match target {
        SymlinkTarget::Normalized(s) => s.replace("/", &path::MAIN_SEPARATOR.to_string()),
        SymlinkTarget::NotNormalized(s) => s, // No normalisation was possible on the src, so leave it as-is
    };

    #[cfg(windows)]
    let res = match kind {
        SymlinkKind::File => std::os::windows::fs::symlink_file(target, &full_path),
        SymlinkKind::Folder => std::os::windows::fs::symlink_dir(target, &full_path),
        SymlinkKind::Unknown => {
            // Windows can't create unknown symlinks - it needs to be either a file or folder symlink
            return Err(format!("Can't create symlink of unknown kind on this platform '{}'", full_path.display()));
        },
    };
    #[cfg(not(windows))]
    // Non-windows platforms can't create explicit file/folder symlinks, but we can just create a generic
    // symlink, which will behave the same. All types of symlink are just generic ones.
    let res = std::os::unix::fs::symlink(target, &full_path);

    if let Err(e) = res {
        return Err(format!("Failed to create symlink '{}': {e}", full_path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use regex::RegexSet;

    use super::*;

    #[test]
    fn test_apply_filters_root() {
        // Filters specify to exclude everything
        let filters = Filters {
            regex_set: RegexSet::new(&["^.*$"]).unwrap(),
            kinds: vec![FilterKind::Exclude]
        };
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("will be excluded")).unwrap(), &filters), FilterResult::Exclude);
        // But the root is always included anyway
        assert_eq!(apply_filters(&RootRelativePath::root(), &filters), FilterResult::Include);
    }

    #[test]
    fn test_apply_filters_no_filters() {
        let filters = Filters {
            regex_set: RegexSet::empty(),
            kinds: vec![]
        };
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("yes")).unwrap(), &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("no")).unwrap(), &filters), FilterResult::Include);
    }

    #[test]
    fn test_apply_filters_single_include() {
        let filters = Filters {
            regex_set: RegexSet::new(&["^yes$"]).unwrap(),
            kinds: vec![FilterKind::Include]
        };
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("yes")).unwrap(), &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("no")).unwrap(), &filters), FilterResult::Exclude);
    }

    #[test]
    fn test_apply_filters_single_exclude() {
        let filters = Filters {
            regex_set: RegexSet::new(&["^no$"]).unwrap(),
            kinds: vec![FilterKind::Exclude]
        };
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("yes")).unwrap(), &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("no")).unwrap(), &filters), FilterResult::Exclude);
    }

    #[test]
    fn test_apply_filters_complex() {
        let filters = Filters {
            regex_set: RegexSet::new(&[
                "^.*$",
                "^build/.*$",
                "^git/.*$",
                "^build/output.exe$",
                "^src/build/.*$",
            ]).unwrap(),
            kinds: vec![
                FilterKind::Include,
                FilterKind::Exclude,
                FilterKind::Exclude,
                FilterKind::Include,
                FilterKind::Exclude,
            ]
        };
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("README")).unwrap(), &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("build/file.o")).unwrap(), &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("git/hash")).unwrap(), &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("build/rob")).unwrap(), &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("build/output.exe")).unwrap(), &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("src/build/file.o")).unwrap(), &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath::try_from(Path::new("src/source.cpp")).unwrap(), &filters), FilterResult::Include);
    }
}
