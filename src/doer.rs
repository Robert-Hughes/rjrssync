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

//TODO: unit tests for this
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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
    pub fn matches_regex(&self, r: &Regex) -> bool {
        r.find(&self.inner).is_some()
    }
}
impl Display for RootRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
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
        exclude_filters: Vec<String>,
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
    CreateFolder {
        path: RootRelativePath,
    },
    DeleteFile {
        path: RootRelativePath,
    },
    DeleteFolder {
        path: RootRelativePath,
    },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum EntryType {
    File,
    Folder,
}

/// Details of a file or folder.
//TODO: use an enum as folders don't have modified_time or size!
#[derive(Serialize, Deserialize, Debug)]
pub struct EntryDetails {
    pub path: RootRelativePath,
    pub entry_type: EntryType,
    pub modified_time: SystemTime,
    pub size: u64
}

/// Responses are sent back from the doer to the boss to report on something, usually
/// the result of a Command.
#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    RootDetails(Option<EntryType>),
    // The result of GetEntries is split into lots of individual messages (rather than one big list)
    // so that the boss can start doing stuff before receiving the full list.
    Entry(EntryDetails),
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
}

// Repeatedly waits for Commands from the boss and processes them (possibly sending back Responses).
// This function returns when we receive a Shutdown Command, or there is an unrecoverable error
// (recoverable errors while handling Commands will not stop the loop).
fn message_loop(mut comms: Comms) -> Result<(), ()> {
    let mut context = DoerContext {
        root: PathBuf::new(),
    };
    loop {
        match comms.receive_command() {
            Ok(c) => {
                if !exec_command(c, &mut comms, &mut context) {
                    debug!("Shutdown command received - finishing message_loop");
                    return Ok(());
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
fn exec_command(command: Command, comms: &mut Comms, context: &mut DoerContext) -> bool {
    match command {
        Command::SetRoot { root } => {
            // Store the root path for future operations
            context.root = PathBuf::from(root);

            match std::fs::metadata(&context.root) {
                Ok(m) => {
                    let root_type = if m.file_type().is_dir() {
                        EntryType::Folder
                    } else if m.file_type().is_file() {
                        EntryType::File
                    } else {
                        comms.send_response(Response::Error(format!(
                                "root has unknown type: {:?}", m
                            )))
                        .unwrap();
                        return true;
                    };

                    comms.send_response(Response::RootDetails(Some(root_type))).unwrap();
                },
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // Report this as a special error, as we handle it differently on the boss side
                    comms.send_response(Response::RootDetails(None)).unwrap();
                    return true;
                }
                Err(e) => {
                    comms
                        .send_response(Response::Error(format!(
                            "root {} can't be read: {}", context.root.display(), e
                        )))
                    .unwrap();
                    return true;
                }
            }
        }
        Command::GetEntries { exclude_filters } => {
            if let Err(e) = handle_get_entries(comms, context, &exclude_filters) {
                comms.send_response(Response::Error(e)).unwrap();
            }
        }
        Command::CreateRootAncestors => {
            let path_to_create = context.root.parent();
            if let Some(p) = path_to_create {
                match std::fs::create_dir_all(p) {
                    Ok(()) => comms.send_response(Response::Ack).unwrap(),
                    Err(e) => comms.send_response(Response::Error(format!("Error creating folder: {e}"))).unwrap(),
                }           
            }
        }  
        Command::GetFileContent { path } => {
            let full_path =  path.get_full_path(&context.root);
            match std::fs::read(full_path) {
                Ok(data) => comms.send_response(Response::FileContent { data }).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error getting file content: {e}"))).unwrap(),
            }
        }
        Command::CreateOrUpdateFile {
            path,
            data,
            set_modified_time,
        } => {
            let full_path = path.get_full_path(&context.root);
            let r = std::fs::write(&full_path, data);
            if let Err(e) = r {
                comms.send_response(Response::Error(format!("Error writing file contents to {}: {e}", full_path.display()))).unwrap();
                return true;
            }

            // After changing the content, we need to override the modified time of the file to that of the original,
            // otherwise it will immediately count as modified again if we do another sync.
            if let Some(t) = set_modified_time {
                let r =
                    filetime::set_file_mtime(&full_path, filetime::FileTime::from_system_time(t));
                if let Err(e) = r {
                    comms.send_response(Response::Error(format!("Error setting modified time: {e}"))).unwrap();
                    return true;
                }
            }

            comms.send_response(Response::Ack).unwrap();
        }
        Command::CreateFolder { path } => {
            let full_path =  path.get_full_path(&context.root);
            match std::fs::create_dir(full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error creating folder: {e}"))).unwrap(),
            }
        }
        Command::DeleteFile { path } => {
            let full_path =  path.get_full_path(&context.root);
            match std::fs::remove_file(full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting file: {e}"))).unwrap(),
            }
        }
        Command::DeleteFolder { path } => {
            let full_path =  path.get_full_path(&context.root);
            match std::fs::remove_dir(full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) => comms.send_response(Response::Error(format!("Error deleting folder: {e}"))).unwrap(),
            }
        }
        Command::Shutdown => {
            return false;
        }
    }
    true
}

fn handle_get_entries(comms: &mut Comms, context: &mut DoerContext, exclude_filters: &[String]) -> Result<(), String> {
    // Compile filter regexes up-front
    //TODO: ideally we do this on the boss, not on both doers?
    //TODO: handle regex errors
    let exclude_regexes: Vec<Regex> = exclude_filters.iter().map(|f| Regex::new(f).unwrap()).collect();

    let start = Instant::now();
    // Due to the way the WalkDir API works, we unfortunately need to do the iter loop manually
    // so that we can avoid normalizing the path twice (once for the filter, once for the conversion
    // of the entry to our representation).
    let mut walker_it = WalkDir::new(&context.root)
        // Don't follow symlinks - we want to sync the links themselves
        .follow_links(false)
        .into_iter();
    let mut count = 0;
    loop {
        match walker_it.next() {
            None => break,
            Some(Err(e)) => return Err(format!("Error walking root: {e}")),
            Some(Ok(e)) => {
                // Check if we should filter this entry.
                // First normalize the path to our platform-independent representation, so that the filters
                // apply equally well on both source and dest sides, if they are different platforms.

                // Paths returned by WalkDir will include the root, but we want paths relative to the root
                let path = e.path().strip_prefix(&context.root).unwrap();
                // Convert to platform-agnostic representation
                let path = match normalize_path(path) {
                    Ok(p) => p,
                    Err(e) => return Err(format!("normalize_path failed: {e}")),
                };

                if exclude_regexes.iter().any(|r| path.matches_regex(r)) {
                    trace!("Skipping {} due to filter", path);
                    if e.file_type().is_dir() {
                        // Filtering a folder prevents iterating into child files/folders, so this is efficient.
                        walker_it.skip_current_dir();
                    }
                    continue;
                }

                let entry_type;
                if e.file_type().is_dir() {
                    entry_type = EntryType::Folder;
                } else if e.file_type().is_file() {
                    entry_type = EntryType::File;
                } else {
                    return Err(format!("Unknown file type for {}: {:?}", path, e.file_type()));
                }

                let metadata = match e.metadata() {
                    Ok(m) => m,
                    Err(e) => return Err(format!("Unable to get metadata: {e}")),
                };

                let modified_time = match metadata.modified() {
                    Ok(m) => m,
                    Err(e) => return Err(format!("Unknown modified time: {e}")),
                };

                let d = EntryDetails {
                    path,
                    entry_type,
                    modified_time,
                    size: metadata.len(),
                };

                comms.send_response(Response::Entry(d)).unwrap();
                //                      if e.file_type().is_file() {
                //                         let bytes = std::fs::read(e.path()).unwrap();
                //                         let hash = md5::compute(&bytes);
                //                         hash_sum += hash.into_iter().sum::<u8>();
                //                         count += 1;
                //                      }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_is_root() {
        let x = normalize_path(Path::new(""));
        assert_eq!(x, Ok(RootRelativePath { inner: "".to_string() }));
        assert_eq!(x.unwrap().is_root(), true);
    }
}