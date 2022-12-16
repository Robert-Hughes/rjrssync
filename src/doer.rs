use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::{Aes128Gcm, KeyInit};
use clap::Parser;
use env_logger::Env;
use log::{debug, error, trace};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Read};
use std::path;
use std::{
    fmt::{self, Display},
    io::{Write},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender},
    time::{Instant, SystemTime}, net::{TcpListener, TcpStream},
};
use walkdir::WalkDir;

use crate::*;
use crate::encrypted_comms::AsyncEncryptedComms;

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
}

fn normalize_path(p: &Path) -> Result<RootRelativePath, String> {
    if p.is_absolute() {
        return Err("Must be relative".to_string());
    }

    let mut result = String::new();
    for c in p.iter() {
        let cs = match c.to_str() {
            Some(x) => x,
            None => return Err("Can't convert path component".to_string()),
        };
        if cs.contains('/') || cs.contains('\\') {
            // Slashes in any component would mess things up, once we change which slash is significant
            return Err("Illegal characters in path".to_string());
        }
        if !result.is_empty() {
            result += "/";
        }
        result += cs;
    }

    Ok(RootRelativePath { inner: result })
}

/// Converts a platform-specific relative path (inside the source or dest root)
/// to something that can be sent over our comms. We can't simply use PathBuf
/// because the syntax of this path might differ between the boss and doer
/// platforms (e.g. Windows vs Linux), and so the type might have different
/// meaning/behaviour on each side.
/// We instead convert to a normalized representation using forward slashes (i.e. Unix-style).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RootRelativePath {
    inner: String,
}
impl RootRelativePath {
    pub fn root() -> RootRelativePath {
        RootRelativePath { inner: "".to_string() }
    }

    /// Does this path refer to the root itself?
    pub fn is_root(&self) -> bool {
        self.inner.is_empty()
    }

    /// Gets the full path consisting of the root and this root-relative path.
    pub fn get_full_path(&self, root: &Path) -> PathBuf {
        if self.is_root() { root.to_path_buf() } else { root.join(&self.inner) }
    }

    /// Rather than exposing the inner string, expose just regex matching.
    /// This reduces the risk of incorrect usage of the raw string value (e.g. by using
    /// local-platform Path functions).
    pub fn matches_regex_full(&self, r: &Regex) -> bool {
        match r.find(&self.inner) {
            None => false,
            // Must match the entire path, not just part of it
            Some(m) => m.start() == 0 && m.end() == self.inner.len()
        }
    }
}
impl Display for RootRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            write!(f, "<ROOT>")
        } else {
            write!(f, "{}", self.inner)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Filter {
    Include(String),
    Exclude(String),
}

/// Commands are sent from the boss to the doer, to request something to be done.
#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    // Checks the root file/folder and send back information about it,
    // as the boss may need to do something before we send it all the rest of the entries
    SetRoot {
        root: String, // Note this doesn't use a RootRelativePath as it isn't relative to the root - it _is_ the root!
    },
    GetEntries {
        filters: Vec<Filter>,
    },
    CreateRootAncestors,
    GetFileContent {
        path: RootRelativePath,
    },
    CreateOrUpdateFile {
        path: RootRelativePath,
        #[serde(with = "serde_bytes")] // Make serde fast
        data: Vec<u8>,
        set_modified_time: Option<SystemTime>,
        /// If set, there is more data for this same file being sent in a following Command.
        /// This is used to split up large files so that we don't send them all in one huge message.
        /// See GetFileContent for more details.
        more_to_follow: bool,
    },
    CreateSymlink {
        path: RootRelativePath,
        kind: SymlinkKind,
        target: SymlinkTarget,
    },
    CreateFolder {
        path: RootRelativePath,
    },
    DeleteFile {
        path: RootRelativePath,
    },
    DeleteFolder {
        path: RootRelativePath,
    },
    DeleteSymlink {
        path: RootRelativePath,
        kind: SymlinkKind,
    },

    #[cfg(feature = "profiling")]
    ProfilingTimeSync,

    Shutdown,
}

/// We need to distinguish what a symlink points to, as Windows filesystems
/// have this distinction and so we need to know when creating one on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymlinkKind {
    File, // A symlink that points to a file
    Folder, // A symlink that points to a folder
    Unknown, // Unix-only - a symlink that we couldn't determine the target type for, e.g. if it is broken.
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymlinkTarget {
    /// A symlink target which we identified as a relative path and converted the slashes to
    /// forward slashes, so it can be converted to the destination platform's local path syntax.
    Normalized(String),
    /// A symlink target which we couldn't normalize, e.g. because it is an absolute path.
    /// This is transferred without any changes.
    NotNormalized(String)
}

/// Details of a file or folder.
/// Note that this representation is consistent with the approach described in the README,
/// and so doesn't consider the name of the node to be part of the node itself.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EntryDetails {
    File {
        modified_time: SystemTime,
        size: u64
    },
    Folder,
    Symlink {
        kind: SymlinkKind,
        target: SymlinkTarget,
    },
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
        let target = match normalize_path(&target) {
            Ok(r) => SymlinkTarget::Normalized(r.inner),
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

/// Responses are sent back from the doer to the boss to report on something, usually
/// the result of a Command.
#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    RootDetails {
        root_details: Option<EntryDetails>, // Option<> because the root might not exist at all
        /// Whether or not this platform differentiates between file and folder symlinks (e.g. Windows),
        /// vs. treating all symlinks the same (e.g. Linux).
        platform_differentiates_symlinks: bool,
    },

    // The result of GetEntries is split into lots of individual messages (rather than one big list)
    // so that the boss can start doing stuff before receiving the full list.
    Entry((RootRelativePath, EntryDetails)),
    EndOfEntries,

    FileContent {
        #[serde(with = "serde_bytes")] // Make serde fast
        data: Vec<u8>,
        /// If set, there is more data for this same file being sent in a following Response.
        /// This is used to split up large files so that we don't send them all in one huge message:
        ///   - better memory usage
        ///   - doesn't crash for really large files
        ///   - more opportunities for pipelining
        more_to_follow: bool,
    },

    #[cfg(feature = "profiling")]
    ProfilingTimeSync(std::time::Duration),
    #[cfg(feature = "profiling")]
    ProfilingData(ProcessProfilingData),

    Ack,
    Error(String),
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
    pub fn send_response(&mut self, r: Response) {
        trace!("Sending response {:?} to {}", r, &self);
        let sender = match self {
            Comms::Local { sender, .. } => sender,
            Comms::Remote { encrypted_comms, .. } => &mut encrypted_comms.sender,
        };
        sender.send(r).expect("Error sending on channel");
    }

    pub fn receive_command(&mut self) -> Command {
        profile_this!();
        trace!("Waiting for command from {}", &self);
        let receiver = match self {
            Comms::Local { receiver, .. } => receiver,
            Comms::Remote { encrypted_comms, .. } => &mut encrypted_comms.receiver,
        };
        receiver.recv().expect("Error receiving from channel")
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
    // will be forced to do a --force-redeploy which isn't very nice.
    let msg = format!("{}{}", HANDSHAKE_STARTED_MSG, VERSION);
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
        // TODO: We could additionally log to a file, which might be useful for cases where the logs don't
        // make it back to the boss (e.g. communication errors)
        let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or(args.log_filter));
        builder.target(env_logger::Target::Stderr);
        // Configure format so that the boss can parse and re-log it
        builder.format(|buf, record| {
            writeln!(
                buf,
                "{} {}",
                record.level(),
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

    let secret_bytes = match base64::decode(secret) {
        Ok(b) => b,
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
            "<> boss",
    )};

    if let Err(e) = message_loop(&mut comms) {
        debug!("doer process finished with error: {:?}", e);
        return ExitCode::from(20)
    }

    stop_timer(main_timer);

    // Send our profiling data (if enabled) back to the boss process so it can combine it with its own
    #[cfg(feature="profiling")]
    comms.send_response(Response::ProfilingData(get_local_process_profiling()));

    debug!("doer process finished successfully!");
    ExitCode::SUCCESS
}

// When the source and/or dest is local, the doer is run as a thread in the boss process,
// rather than over ssh.
pub fn doer_thread_running_on_boss(receiver: Receiver<Command>, sender: Sender<Response>) {
    debug!("doer thread running");
    profile_this!();
    match message_loop(&mut Comms::Local { sender, receiver }) {
        Ok(_) => debug!("doer thread finished successfully!"),
        Err(e) => debug!("doer thread finished with error: {:?}", e),
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
            c => {
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
            // Err(e) => {
            //     error!("Error receiving command: {}", e);
            //     return Err(());
            // }
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
                comms.send_response(Response::Error(e));
            }
        }
        Command::GetEntries { filters } => {
            profile_this!("GetEntries");
            if let Err(e) = handle_get_entries(comms, context.as_mut().unwrap(), &filters) {
                comms.send_response(Response::Error(e));
            }
        }
        Command::CreateRootAncestors => {
            let path_to_create = context.as_ref().unwrap().root.parent();
            trace!("Creating {:?} and all its ancestors", path_to_create);
            if let Some(p) = path_to_create {
                profile_this!(format!("CreateRootAncestors {}", p.to_str().unwrap().to_string()));
                match std::fs::create_dir_all(p) {
                    Ok(()) => comms.send_response(Response::Ack),
                    Err(e) => comms.send_response(Response::Error(format!("Error creating folder and ancestors for '{}': {e}", p.display()))),
                }
            }
        }
        Command::GetFileContent { path } => {
            let full_path = path.get_full_path(&context.as_ref().unwrap().root);
            profile_this!(format!("GetFileContent {}", path.to_string()));
            if let Err(e) = handle_get_file_contents(comms, &full_path) {
                comms.send_response(Response::Error(e));
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

            // Check if this is the continuation of an existing file
            let mut f = match context.as_mut().unwrap().in_progress_file_receive.take() {
                Some((in_progress_path, f)) => {
                    if in_progress_path == path {
                        f
                    } else {
                        comms.send_response(Response::Error(format!("Unexpected continued file transfer!")));
                        return Ok(true);
                    }
                },
                None => match std::fs::File::create(&full_path) {
                    Ok(f) => f,
                    Err(e) => {
                        comms.send_response(Response::Error(format!("Error writing file contents to '{}': {e}", full_path.display())));
                        return Ok(true);
                    }
                }
            };

            let r = f.write_all(&data);
            if let Err(e) = r {
                comms.send_response(Response::Error(format!("Error writing file contents to '{}': {e}", full_path.display())));
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
                    comms.send_response(Response::Error(format!("Error setting modified time of '{}': {e}", full_path.display())));
                    return Ok(true);
                }
            }

            comms.send_response(Response::Ack);
        }
        Command::CreateFolder { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Creating folder '{}'", full_path.display());
            profile_this!(format!("CreateFolder {}", full_path.to_str().unwrap().to_string()));
            match std::fs::create_dir(&full_path) {
                Ok(()) => comms.send_response(Response::Ack),
                Err(e) => comms.send_response(Response::Error(format!("Error creating folder '{}': {e}", full_path.display()))),
            }
        }
        Command::CreateSymlink { path, kind, target } => {
            match handle_create_symlink(path, context.as_mut().unwrap(), kind, target) {
                Ok(()) => comms.send_response(Response::Ack),
                Err(e) => comms.send_response(Response::Error(e)),
            }
        },
        Command::DeleteFile { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting file '{}'", full_path.display());
            profile_this!(format!("DeleteFile {}", path.to_string()));
            match std::fs::remove_file(&full_path) {
                Ok(()) => comms.send_response(Response::Ack),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting file '{}': {e}", full_path.display()))),
            }
        }
        Command::DeleteFolder { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting folder '{}'", full_path.display());
            profile_this!(format!("DeleteFolder {}", path.to_string()));
            match std::fs::remove_dir(&full_path) {
                Ok(()) => comms.send_response(Response::Ack),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting folder '{}': {e}", full_path.display()))),
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
                        comms.send_response(Response::Error(format!("Can't delete symlink of unknown type '{}'", full_path.display())));
                        return Ok(true);
                    }
                }
            } else {
                // On Linux, any kind of symlink is removed with remove_file
                std::fs::remove_file(&full_path)
            };
            match res {
                Ok(()) => comms.send_response(Response::Ack),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting symlink '{}': {e}", full_path.display()))),
            }
        },
        #[cfg(feature="profiling")]
        Command::ProfilingTimeSync => {
            comms.send_response(Response::ProfilingTimeSync(PROFILING_START.elapsed()));
        },
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

    // Respond to the boss with what type of file/folder the root is, as it makes some decisions
    // based on this.
    // We use symlink_metadata so that we see the metadata of a symlink, not its target
    let metadata = std::fs::symlink_metadata(&context.root);
    match metadata {
        Ok(m) => {
            let entry_details = entry_details_from_metadata(m, &context.root)?;
            comms.send_response(Response::RootDetails { root_details: Some(entry_details), platform_differentiates_symlinks });
        },
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Report this as a special error, as we handle it differently on the boss side
            comms.send_response(Response::RootDetails { root_details: None, platform_differentiates_symlinks });
        }
        Err(e) => return Err(format!(
                    "root '{}' can't be read: {}", context.root.display(), e)),
    }

    Ok(())
}

enum CompiledFilter {
    Include(Regex),
    Exclude(Regex)
}

#[derive(PartialEq, Debug)]
enum FilterResult {
    Include,
    Exclude
}

fn apply_filters(path: &RootRelativePath, filters: &[CompiledFilter]) -> FilterResult {
    if path.is_root() {
        // The root is always included, otherwise it would be difficult to write filter lists that start with include,
        // because you'd need to include the root (empty string) explicitly
        return FilterResult::Include;
    }

    let mut result = match filters.get(0) {
        Some(CompiledFilter::Include(_)) => FilterResult::Exclude,
        Some(CompiledFilter::Exclude(_)) => FilterResult::Include,
        None => FilterResult::Include
    };

    for f in filters {
        match result {
            FilterResult::Include => {
                match f {
                    CompiledFilter::Include(_) => (), // No point checking - we are already including this file
                    CompiledFilter::Exclude(f) => {
                        // trace!("match to exclude: {}, {}, {}", path, f, path.matches_regex_full(f));
                        if path.matches_regex_full(f) {
                            result = FilterResult::Exclude;
                        }
                    }
                }
            },
            FilterResult::Exclude => {
                match f {
                    CompiledFilter::Include(f) => {
                        // trace!("match to include: {}, {}, {}", path, f, path.matches_regex_full(f));
                        if path.matches_regex_full(f) {
                            result = FilterResult::Include;
                        }
                    }
                    CompiledFilter::Exclude(_) => (), // No point checking - we are already excluding this file
                }
            }
        };
    }

    result
}

fn handle_get_entries(comms: &mut Comms, context: &mut DoerContext, filters: &[Filter]) -> Result<(), String> {
    // Compile filter regexes up-front
    let mut compiled_filters: Vec<CompiledFilter> = vec![];
    for f in filters {
        let r =  match f {
            Filter::Include(s) | Filter::Exclude(s) => Regex::new(s),
        };
        match r {
            Ok(r) => {
                let c = match f {
                    Filter::Include(_) => CompiledFilter::Include(r),
                    Filter::Exclude(_) => CompiledFilter::Exclude(r),
                };
                compiled_filters.push(c);
            }
            Err(e) => return Err(format!("Invalid regex for filter '{:?}': {e}", f)),
        };
    }

    let start = Instant::now();
    // Due to the way the WalkDir API works, we unfortunately need to do the iter loop manually
    // so that we can avoid normalizing the path twice (once for the filter, once for the conversion
    // of the entry to our representation).
    // Note that we can't use this to get metadata for a single root entry when that entry is a broken symlink,
    // as the walk will fail before we can get the metadata for the broken link. Therefore we only use this
    // when walking what's known to be a directory (discovered in SetRoot).
    let mut walker_it = WalkDir::new(&context.root)
        .follow_links(false)  // We want to see the symlinks, not their targets
        .into_iter();
    let mut count = 0;
    loop {
        match walker_it.next() {
            None => break,
            Some(Err(e)) => return Err(format!("Error fetching entries of root '{}': {e}", context.root.display())),
            Some(Ok(e)) => {
                trace!("Processing entry {:?}", e);
                profile_this!("Processing entry");

                // Skip the first entry - the root, as the boss already has details of this from SetRoot.
                if e.depth() == 0 {
                    continue;
                }

                // Check if we should filter this entry.
                // First normalize the path to our platform-independent representation, so that the filters
                // apply equally well on both source and dest sides, if they are different platforms.

                // Paths returned by WalkDir will include the root, but we want paths relative to the root
                // The strip_prefix should always be successful, because the entry has to be inside the root.
                let path = e.path().strip_prefix(&context.root).unwrap();
                // Convert to platform-agnostic representation
                let path = match normalize_path(path) {
                    Ok(p) => p,
                    Err(e) => return Err(format!("normalize_path failed on '{}': {e}", path.display())),
                };

                if apply_filters(&path, &compiled_filters) == FilterResult::Exclude {
                    trace!("Skipping '{}' due to filter", path);
                    if e.file_type().is_dir() {
                        // Filtering a folder prevents iterating into child files/folders, so this is efficient.
                        walker_it.skip_current_dir();
                    }
                    continue;
                }

                let metadata = match e.metadata() {
                    Ok(m) => m,
                    Err(err) => return Err(format!("Unable to get metadata for '{}': {err}", path)),
                };

                let d = entry_details_from_metadata(metadata, e.path())?;

                comms.send_response(Response::Entry((path, d)));
            }
        }
        count += 1;
    }

    let elapsed = start.elapsed().as_millis();
    comms.send_response(Response::EndOfEntries);
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
        match f.read(&mut next_buf) {
            Ok(n) if n == 0 => {
                // End of file - send the data that we got previously, and report that there is no more data to follow.
                prev_buf.truncate(prev_buf_valid);
                comms.send_response(Response::FileContent { data: prev_buf, more_to_follow: false });
                return Ok(());
            },
            Ok(n) => {
                // Some data read - send any previously retrieved data, and report that there is more data to follow
                if prev_buf_valid > 0 {
                    prev_buf.truncate(prev_buf_valid);
                    comms.send_response(Response::FileContent { data: prev_buf, more_to_follow: true });
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
                    chunk_size = std::cmp::min(chunk_size * 2, 1024*1024*4);  // 4 MB, chosen pretty arbitirarily

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
    use super::*;

    #[test]
    fn test_normalize_path_is_root() {
        let x = normalize_path(Path::new(""));
        assert_eq!(x, Ok(RootRelativePath::root()));
        assert_eq!(x.unwrap().is_root(), true);
    }

    #[test]
    fn test_normalize_path_absolute() {
        let x = if cfg!(windows) {
            "C:\\Windows"
        } else {
            "/etc/hello"
        };
        assert_eq!(normalize_path(Path::new(x)), Err("Must be relative".to_string()));
    }

    #[cfg(unix)] // This test isn't possible on Windows, because both kinds of slashes are valid separators
    #[test]
    fn test_normalize_path_slashes_in_component() {
        assert_eq!(normalize_path(Path::new("a path with\\backslashes/adsa")), Err("Illegal characters in path".to_string()));
    }

    #[test]
    fn test_normalize_path_multiple_components() {
        assert_eq!(normalize_path(Path::new("one/two/three")), Ok(RootRelativePath { inner: "one/two/three".to_string() }));
    }

    #[test]
    fn test_apply_filters_root() {
        // Filters specify to exclude everything
        let filters = vec![
            CompiledFilter::Exclude(Regex::new(".*").unwrap())
        ];
        assert_eq!(apply_filters(&RootRelativePath { inner: "will be excluded".to_string() }, &filters), FilterResult::Exclude);
        // But the root is always included anyway
        assert_eq!(apply_filters(&RootRelativePath::root(), &filters), FilterResult::Include);
    }

    #[test]
    fn test_apply_filters_no_filters() {
        let filters = vec![];
        assert_eq!(apply_filters(&RootRelativePath { inner: "yes".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "no".to_string() }, &filters), FilterResult::Include);
    }

    #[test]
    fn test_apply_filters_single_include() {
        let filters = vec![
            CompiledFilter::Include(Regex::new("yes").unwrap())
        ];
        assert_eq!(apply_filters(&RootRelativePath { inner: "yes".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "no".to_string() }, &filters), FilterResult::Exclude);
    }

    #[test]
    fn test_apply_filters_single_exclude() {
        let filters = vec![
            CompiledFilter::Exclude(Regex::new("no").unwrap())
        ];
        assert_eq!(apply_filters(&RootRelativePath { inner: "yes".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "no".to_string() }, &filters), FilterResult::Exclude);
    }

    /// Checks that the regex must match the full path, not just part of it.
    #[test]
    fn test_apply_filters_partial_match() {
        let filters = vec![
            CompiledFilter::Include(Regex::new("a").unwrap())
        ];
        assert_eq!(apply_filters(&RootRelativePath { inner: "a".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "aa".to_string() }, &filters), FilterResult::Exclude);
    }

    #[test]
    fn test_apply_filters_complex() {
        let filters = vec![
            CompiledFilter::Include(Regex::new(".*").unwrap()),
            CompiledFilter::Exclude(Regex::new("build/.*").unwrap()),
            CompiledFilter::Exclude(Regex::new("git/.*").unwrap()),
            CompiledFilter::Include(Regex::new("build/output.exe").unwrap()),
            CompiledFilter::Exclude(Regex::new("src/build/.*").unwrap()),
        ];
        assert_eq!(apply_filters(&RootRelativePath { inner: "README".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "build/file.o".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "git/hash".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "build/rob".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "build/output.exe".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "src/build/file.o".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "src/source.cpp".to_string() }, &filters), FilterResult::Include);
    }
}
