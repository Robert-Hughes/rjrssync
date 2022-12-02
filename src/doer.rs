use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::{Aes128Gcm, KeyInit};
use clap::Parser;
use env_logger::Env;
use log::{debug, error, trace};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::{
    fmt::{self, Display},
    io::{Write},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender},
    time::{Instant, SystemTime}, net::{TcpListener, TcpStream},
};
use walkdir::WalkDir;

use crate::*;

#[derive(clap::Parser)]
struct DoerCliArgs {
    /// [Internal] Launches as a doer process, rather than a boss process.
    /// This shouldn't be needed for regular operation.
    #[arg(long)]
    doer: bool,
    /// The network port to listen on for a connection from the boss.
    #[arg(long)]
    port: u16,
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
        symlink_mode: SymlinkMode,
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
        data: Vec<u8>,
        set_modified_time: Option<SystemTime>,
    },
    CreateSymlink {
        path: RootRelativePath,
        kind: SymlinkKind,
        target: String, // We don't assume anything about this text
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
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymlinkKind {
    File, // Windows-only
    Folder, // Windows-only
    Generic, // Unix-only
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
        target: String, // We don't assume anything about this text
    },
}

/// Details of the root, returned from SetRoot.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum RootDetails {
    File,
    Folder,
    Symlink,
    None
}

/// Responses are sent back from the doer to the boss to report on something, usually
/// the result of a Command.
#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    RootDetails(RootDetails),

    // The result of GetEntries is split into lots of individual messages (rather than one big list)
    // so that the boss can start doing stuff before receiving the full list.
    Entry((RootRelativePath, EntryDetails)),
    EndOfEntries,

    FileContent { data: Vec<u8> },

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
        tcp_connection: TcpStream,
        cipher: Aes128Gcm,
        sending_nonce_counter: u64,
        receiving_nonce_counter: u64,
    },
}
impl Comms {
    pub fn send_response(&mut self, r: Response) -> Result<(), String> {
        trace!("Sending response {:?} to {}", r, &self);
        let res =
            match self {
                Comms::Local { sender, .. } => {
                    sender.send(r).map_err(|e| "Error sending on channel: ".to_string() + &e.to_string())
                }
                Comms::Remote { tcp_connection, cipher, sending_nonce_counter, .. } => {
                    encrypted_comms::send(r, tcp_connection, cipher, sending_nonce_counter, 1)
                }
            };
        if let Err(ref e) = &res {
            error!("Error sending response: {:?}", e);
        }
        res
    }

    pub fn receive_command(&mut self) -> Result<Command, String> {
        trace!("Waiting for command from {}", &self);
        let res = match self {
            Comms::Local { receiver, .. } => {
                receiver.recv().map_err(|e| "Error receiving from channel: ".to_string() + &e.to_string())
            }
            Comms::Remote { tcp_connection, cipher, receiving_nonce_counter, .. } => {
                encrypted_comms::receive(tcp_connection, cipher, receiving_nonce_counter, 0)
            }
        };
        match &res {
            Err(ref e) => error!("{}", e),
            Ok(ref r) => trace!("Received command {:?} from {}", r, &self),
        }
        res
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
    let args = DoerCliArgs::parse();

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

    // The first thing we send is a special handshake message that the Boss will recognise,
    // to know that we've started up correctly and to make sure we are running compatible versions.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    let msg = format!("{}{}", HANDSHAKE_STARTED_MSG, VERSION);
    println!("{}", msg);
    eprintln!("{}", msg);

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

    // Start listening on the requested port. Listen on all interfaces as we don't know which one is needed.
    let addr = ("0.0.0.0", args.port);
    let listener = match TcpListener::bind(addr) {
        Ok(l) => {
            debug!("Listening on {:?}", addr);
            l
        }
        Err(e) => {
            error!("Failed to bind to {:?}: {}", addr, e);
            return ExitCode::from(24);
        }
    };

    // Let the boss know that we are ready for the network connection
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    println!("{}", HANDSHAKE_COMPLETED_MSG);
    eprintln!("{}", HANDSHAKE_COMPLETED_MSG);

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

    // Start command processing loop, receiving commands and sending responses over the TCP connection, with encryption
    // so that we know it's the boss.
    let comms = Comms::Remote {
        tcp_connection,
        cipher: Aes128Gcm::new(secret_key),
        sending_nonce_counter: 1, // Nonce counters must be different, so sender and receiver don't reuse
        receiving_nonce_counter: 0,
    };

    match message_loop(comms) {
        Ok(_) => {
            debug!("doer process finished successfully!");
            ExitCode::SUCCESS
        }
        Err(e) => {
            debug!("doer process finished with error: {:?}", e);
            ExitCode::from(20)
        }
    }
}

// When the source and/or dest is local, the doer is run as a thread in the boss process,
// rather than over ssh.
pub fn doer_thread_running_on_boss(receiver: Receiver<Command>, sender: Sender<Response>) {
    debug!("doer thread running");
    match message_loop(Comms::Local { sender, receiver }) {
        Ok(_) => debug!("doer thread finished successfully!"),
        Err(e) => debug!("doer thread finished with error: {:?}", e),
    }
}

/// Context for each doer instance. We can't use anything global (e.g. like changing the
/// process' current directory), because there might be multiple doer threads in the same process
/// (if these are local doers).
struct DoerContext {
    pub root: PathBuf,
    pub symlink_mode: SymlinkMode
}

// Repeatedly waits for Commands from the boss and processes them (possibly sending back Responses).
// This function returns when we receive a Shutdown Command, or there is an unrecoverable error
// (recoverable errors while handling Commands will not stop the loop).
fn message_loop(mut comms: Comms) -> Result<(), ()> {
    let mut context : Option<DoerContext> = None;
    loop {
        match comms.receive_command() {
            Ok(c) => {
                match exec_command(c, &mut comms, &mut context) {
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
            Err(e) => {
                error!("Error receiving command: {}", e);
                return Err(());
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
        Command::SetRoot { root, symlink_mode } => {
            // Store the root path for future operations
            *context = Some(DoerContext {
                root: PathBuf::from(root),
                symlink_mode,
            });
            let context = context.as_ref().unwrap();

            // Respond to the boss with what type of file/folder the root is, as it makes some decisions
            // based on this.
            // We use symlink_metadata depending on the symlink mode
            let metadata = match symlink_mode {
                SymlinkMode::Unaware => std::fs::metadata(&context.root),
                SymlinkMode::Preserve => std::fs::symlink_metadata(&context.root),
            };

            match metadata {
                Ok(m) => {
                    let root_details = if m.file_type().is_dir() {
                        RootDetails::Folder
                    } else if m.file_type().is_file() {
                        RootDetails::File
                    } else if m.file_type().is_symlink() {
                        RootDetails::Symlink
                    } else {
                        comms.send_response(Response::Error(format!(
                                "root '{}' has unknown type: {:?}", context.root.display(), m
                            )))?;
                        return Ok(true);
                    };

                    comms.send_response(Response::RootDetails(root_details))?;
                },
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // Report this as a special error, as we handle it differently on the boss side
                    comms.send_response(Response::RootDetails(RootDetails::None))?;
                }
                Err(e) => {
                    comms
                        .send_response(Response::Error(format!(
                            "root '{}' can't be read: {}", context.root.display(), e
                        )))?;
                }
            }
        }
        Command::GetEntries { filters } => {
            profile_this!("GetEntries");
            if let Err(e) = handle_get_entries(comms, context.as_mut().unwrap(), &filters) {
                comms.send_response(Response::Error(e)).unwrap();
            }
        }
        Command::CreateRootAncestors => {
            let path_to_create = context.as_ref().unwrap().root.parent();
            trace!("Creating {:?} and all its ancestors", path_to_create);
            if let Some(p) = path_to_create {
                profile_this!("CreateRootAncestors", p.to_str().unwrap().to_string());
                match std::fs::create_dir_all(p) {
                    Ok(()) => comms.send_response(Response::Ack).unwrap(),
                    Err(e) => comms.send_response(Response::Error(format!("Error creating folder and ancestors for '{}': {e}", p.display()))).unwrap(),
                }
            }
        }
        Command::GetFileContent { path } => {
            let full_path = path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Getting content of '{}'", full_path.display());
            profile_this!("GetFileContent", path.to_string());
            match std::fs::read(&full_path) {
                Ok(data) => comms.send_response(Response::FileContent { data }).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error getting file content of '{}': {e}", full_path.display()))).unwrap(),
            }
        }
        Command::CreateOrUpdateFile {
            path,
            data,
            set_modified_time,
        } => {
            let full_path = path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Creating/updating content of '{}'", full_path.display());
            profile_this!("CreateOrUpdateFile", path.to_string());
            let r = std::fs::write(&full_path, data);
            if let Err(e) = r {
                comms.send_response(Response::Error(format!("Error writing file contents to '{}': {e}", full_path.display()))).unwrap();
                return Ok(true);
            }

            // After changing the content, we need to override the modified time of the file to that of the original,
            // otherwise it will immediately count as modified again if we do another sync.
            if let Some(t) = set_modified_time {
                trace!("Setting modifited time of '{}'", full_path.display());
                let r =
                    filetime::set_file_mtime(&full_path, filetime::FileTime::from_system_time(t));
                if let Err(e) = r {
                    comms.send_response(Response::Error(format!("Error setting modified time of '{}': {e}", full_path.display()))).unwrap();
                    return Ok(true);
                }
            }

            comms.send_response(Response::Ack).unwrap();
        }
        Command::CreateFolder { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Creating folder '{}'", full_path.display());
            profile_this!("CreateFolder", full_path.to_str().unwrap().to_string());
            match std::fs::create_dir(&full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error creating folder '{}': {e}", full_path.display()))).unwrap(),
            }
        }
        Command::CreateSymlink { path, kind, target } => {
            match handle_create_symlink(path, context.as_mut().unwrap(), kind, target) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),               
                Err(e) => comms.send_response(Response::Error(e)).unwrap(),
            }
        },
        Command::DeleteFile { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting file '{}'", full_path.display());
            profile_this!("DeleteFile", path.to_string());
            match std::fs::remove_file(&full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting file '{}': {e}", full_path.display()))).unwrap(),
            }
        }
        Command::DeleteFolder { path } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting folder '{}'", full_path.display());
            profile_this!("DeleteFolder", path.to_string());
            match std::fs::remove_dir(&full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting folder '{}': {e}", full_path.display()))).unwrap(),
            }
        }
        Command::DeleteSymlink { path, kind } => {
            let full_path =  path.get_full_path(&context.as_ref().unwrap().root);
            trace!("Deleting symlink '{}'", full_path.display());
            let res = match kind {
                SymlinkKind::File => std::fs::remove_file(&full_path),
                SymlinkKind::Folder => std::fs::remove_dir(&full_path),
                // Unspecified is only used for Unix, and remove_file is the correct way to delete these.
                SymlinkKind::Generic => std::fs::remove_file(&full_path),
            };
            match res {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting symlink '{}': {e}", full_path.display()))).unwrap(),
            }
        }
        Command::Shutdown => {
            return Ok(false);
        }
    }
    Ok(true)
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
    let mut walker_it = WalkDir::new(&context.root)
        .follow_links(context.symlink_mode == SymlinkMode::Unaware) 
        .into_iter();
    let mut count = 0;
    loop {
        match walker_it.next() {
            None => break,
            Some(Err(e)) => return Err(format!("Error fetching entries of root '{}': {e}", context.root.display())),
            Some(Ok(e)) => {
                trace!("Processing entry {:?}", e);
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

                let d = if e.file_type().is_dir() {
                    EntryDetails::Folder
                } else if e.file_type().is_file() {
                    let metadata = match e.metadata() {
                        Ok(m) => m,
                        Err(err) => return Err(format!("Unable to get metadata for '{}': {err}", path)),
                    };

                    let modified_time = match metadata.modified() {
                        Ok(m) => m,
                        Err(err) => return Err(format!("Unknown modified time for '{}': {err}", path)),
                    };

                    EntryDetails::File {
                        modified_time,
                        size: metadata.len(),
                    }
                } else if e.file_type().is_symlink() {
                    let target = match std::fs::read_link(e.path()) {
                        Ok(t) => match t.to_str() {
                            Some(t) => t.to_string(),
                            None => return Err(format!("Unable to convert symlink target for '{}' to UTF-8", path)),
                        },
                        Err(err) => return Err(format!("Unable to read symlink target for '{}': {err}", path)),
                    };
   
                    let metadata = match e.metadata() {
                        Ok(m) => m,
                        Err(err) => return Err(format!("Unable to get metadata for '{}': {err}", path)),
                    };

                    // On Windows, symlinks are either file-symlinks or dir-symlinks
                    #[cfg(windows)]
                    let kind = if std::os::windows::fs::FileTypeExt::is_symlink_file(&metadata.file_type()) {
                        SymlinkKind::File
                    } else if std::os::windows::fs::FileTypeExt::is_symlink_dir(&metadata.file_type()) {
                        SymlinkKind::Folder
                    } else {
                        return Err(format!("Unknown symlink type time for '{}'", path));
                    };
                    #[cfg(not(windows))]
                    let kind = SymlinkKind::Generic;
            
                    EntryDetails::Symlink { kind, target }
                } else {
                    return Err(format!("Unknown file type for '{}': {:?}", path, e.file_type()));
                };

                comms.send_response(Response::Entry((path, d))).unwrap();

                // If this was the root entry, and is a directory symlink, and we're in preserve mode,
                // then the WalkDir crate will correctly report the root as a symlink, _but_ it still follows
                // the symlink and then walks the contents of that directory, which we _don't_ want, so we can
                // cancel the iteration here
                if e.depth() == 0 && context.symlink_mode == SymlinkMode::Preserve && e.file_type().is_symlink() {
                    break;
                }
            }
        }
        count += 1;
    }

    let elapsed = start.elapsed().as_millis();
    comms.send_response(Response::EndOfEntries).unwrap();
    debug!(
        "Walked {} in {}ms ({}/s)",
        count,
        elapsed,
        1000.0 * count as f32 / elapsed as f32
    );

    Ok(())
}

fn handle_create_symlink(path: RootRelativePath, context: &mut DoerContext, kind: SymlinkKind, target: String) -> Result<(), String> {
    let full_path = path.get_full_path(&context.root);
    trace!("Creating symlink at '{}'", full_path.display());
    let res = match kind {
        SymlinkKind::File => {
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(target, &full_path)
            }
            // Non-windows platforms can't create explicit file symlinks, but we can just create a generic
            // symlink, which will behave the same.
            #[cfg(not(windows))] 
            {
                std::os::unix::fs::symlink(target, &full_path)
            }
        },
        SymlinkKind::Folder => {
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(target, &full_path)
            }
            // Non-windows platforms can't create explicit folder symlinks, but we can just create a generic
            // symlink, which will behave the same.
            #[cfg(not(windows))]
            {
                std::os::unix::fs::symlink(target, &full_path)
            }
        }
        SymlinkKind::Generic => {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, &full_path)
            }
            #[cfg(not(unix))] // Windows can't create unspecified symlinks - it needs to be either a file or folder symlink
            {
                //TODO: we could do a best-effort here by checking the type of the target on the other doer (if it exists), and using that.
                return Err(format!("Can't create unspecified symlink on this platform '{}'", full_path.display()));
            }      
        },
    };
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
        assert_eq!(x, Ok(RootRelativePath { inner: "".to_string() }));
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
        assert_eq!(apply_filters(&RootRelativePath { inner: "".to_string() }, &filters), FilterResult::Include);
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
