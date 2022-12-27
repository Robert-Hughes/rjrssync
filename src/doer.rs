use aes_gcm::aead::generic_array::GenericArray;
use clap::Parser;
use env_logger::Env;
use log::{debug, error, trace, info};
use regex::{RegexSet, SetMatches};
use serde::{Deserialize, Serialize, Serializer, Deserializer, de::Error};
use std::io::{ErrorKind, Read};
use std::path;
use std::{
    fmt::{self, Display},
    io::{Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime}, net::{TcpListener},
};
use walkdir::WalkDir;

use crate::*;
use crate::encrypted_comms::AsyncEncryptedComms;
use crate::memory_bound_channel::{Sender, Receiver};

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
    pub fn regex_set_matches(&self, r: &RegexSet) -> SetMatches {
        r.matches(&self.inner)
    }

    /// Puts the slashes back to what is requested, so that the path is appropriate for
    /// another platform.
    pub fn to_platform_path(&self, dir_separator: char) -> String {
        self.inner.replace('/', &dir_separator.to_string())
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
pub struct Filters {
    /// Use a RegexSet rather than separate Regex objects for better performance.
    /// Serialize the regexes as strings - even though they will need compiling on the 
    /// doer side as well (as part of deserialization), we compile them on the boss side to report earlier errors to 
    /// user (as part of input validation, rather than waiting until the sync starts),
    /// and so that we don't need to validate them again on the doer side,
    /// and won't report the same error twice (from both doers).
    #[serde(serialize_with = "serialize_regex_set_as_strings", deserialize_with="deserialize_regex_set_from_strings")] 
    pub regex_set: RegexSet,
    /// For each regex in the RegexSet above, is it an include filter or an exclude filter.
    pub kinds: Vec<FilterKind>, 
}

/// Serializes a RegexSet by serializing the patterns (strings) that it was originally created from.
/// This won't preserve any non-default creation options!
fn serialize_regex_set_as_strings<S: Serializer>(r: &RegexSet, s: S) -> Result<S::Ok, S::Error> {
    r.patterns().serialize(s)
}

/// Deserializes a RegexSet by deserializing the patterns (strings) that it was originally created from
/// and then re-compiling the RegexSet.
/// This won't preserve any non-default creation options!
fn deserialize_regex_set_from_strings<'de, D: Deserializer<'de>>(d: D) -> Result<RegexSet, D::Error> {
    let patterns = <Vec<String>>::deserialize(d)?;
    RegexSet::new(patterns).map_err(|e| D::Error::custom(e))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum FilterKind {
    Include,
    Exclude,
}

/// Commands are sent from the boss to the doer, to request something to be done.
#[derive(Serialize, Deserialize)]
pub enum Command {
    // Checks the root file/folder and send back information about it,
    // as the boss may need to do something before we send it all the rest of the entries
    SetRoot {
        root: String, // Note this doesn't use a RootRelativePath as it isn't relative to the root - it _is_ the root!
    },
    GetEntries {
        filters: Filters,
    },
    CreateRootAncestors,
    GetFileContent {
        path: RootRelativePath,
    },
    CreateOrUpdateFile {
        path: RootRelativePath,
        #[serde(with = "serde_bytes")] // Make serde fast
        data: Vec<u8>,
        // Note that SystemTime is safe to serialize across platforms, because Serde serializes this 
        // as the elapsed time since UNIX_EPOCH, so it is platform-independent.        
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

    /// Used to mark a position in the sequence of commands, which the doer will echo back 
    /// so that the boss knows when the doer has reached this point. 
    /// The boss can't update the progress bar as soon as it sends a command, as the doer hasn't actually done 
    /// anything yet, so instead it inserts a marker and when the doer echoes this marker back 
    /// (meaning it got this far), the boss updates the progress bar.
    Marker(u64),

    Shutdown,
}
// The default Debug implementation prints all the file data, which is way too much, so we have to override this :(
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note that rust-analyzer can auto-generate the complete version of this for us (delete the function, then Ctrl+Space),
        // then we can make the tweaks that we need.
        match self {
            Self::SetRoot { root } => f.debug_struct("SetRoot").field("root", root).finish(),
            Self::GetEntries { filters } => f.debug_struct("GetEntries").field("filters", filters).finish(),
            Self::CreateRootAncestors => write!(f, "CreateRootAncestors"),
            Self::GetFileContent { path } => f.debug_struct("GetFileContent").field("path", path).finish(),
            Self::CreateOrUpdateFile { path, data: _, set_modified_time, more_to_follow } => f.debug_struct("CreateOrUpdateFile").field("path", path).field("data", &"...").field("set_modified_time", set_modified_time).field("more_to_follow", more_to_follow).finish(),
            Self::CreateSymlink { path, kind, target } => f.debug_struct("CreateSymlink").field("path", path).field("kind", kind).field("target", target).finish(),
            Self::CreateFolder { path } => f.debug_struct("CreateFolder").field("path", path).finish(),
            Self::DeleteFile { path } => f.debug_struct("DeleteFile").field("path", path).finish(),
            Self::DeleteFolder { path } => f.debug_struct("DeleteFolder").field("path", path).finish(),
            Self::DeleteSymlink { path, kind } => f.debug_struct("DeleteSymlink").field("path", path).field("kind", kind).finish(),
            #[cfg(feature = "profiling")]
            Self::ProfilingTimeSync => write!(f, "ProfilingTimeSync"),
            Self::Marker(arg0) => f.debug_tuple("Marker").field(arg0).finish(),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
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
        // Note that SystemTime is safe to serialize across platforms, because Serde serializes this 
        // as the elapsed time since UNIX_EPOCH, so it is platform-independent.
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
#[derive(Serialize, Deserialize)]
pub enum Response {
    RootDetails {
        root_details: Option<EntryDetails>, // Option<> because the root might not exist at all
        /// Whether or not this platform differentiates between file and folder symlinks (e.g. Windows),
        /// vs. treating all symlinks the same (e.g. Linux).
        platform_differentiates_symlinks: bool,
        /// Forward vs backwards slash.
        platform_dir_separator: char,
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

    /// The doer echoes back Marker commands, so the boss can keep track of the doer's progress.
    Marker(u64),

    Error(String),
}
// The default Debug implementation prints all the file data, which is way too much, so we have to override this :(
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note that rust-analyzer can auto-generate the complete version of this for us (delete the function, then Ctrl+Space),
        // then we can make the tweaks that we need.
        match self {
            Self::RootDetails { root_details, platform_differentiates_symlinks, platform_dir_separator } => f.debug_struct("RootDetails").field("root_details", root_details).field("platform_differentiates_symlinks", platform_differentiates_symlinks).field("platform_dir_separator", platform_dir_separator).finish(),
            Self::Entry(arg0) => f.debug_tuple("Entry").field(arg0).finish(),
            Self::EndOfEntries => write!(f, "EndOfEntries"),
            Self::FileContent { data: _, more_to_follow } => f.debug_struct("FileContent").field("data", &"...").field("more_to_follow", more_to_follow).finish(),
            #[cfg(feature = "profiling")]
            Self::ProfilingTimeSync(arg0) => f.debug_tuple("ProfilingTimeSync").field(arg0).finish(),
            #[cfg(feature = "profiling")]
            Self::ProfilingData(_) => f.debug_tuple("ProfilingData").finish(),
            Self::Marker(arg0) => f.debug_tuple("Marker").field(arg0).finish(),
            Self::Error(arg0) => f.debug_tuple("Error").field(arg0).finish(),
        }
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
    pub fn send_response(&mut self, r: Response) -> Result<(), &'static str> {
        trace!("Sending response {:?} to {}", r, &self);
        let sender = match self {
            Comms::Local { sender, .. } => sender,
            Comms::Remote { encrypted_comms, .. } => &mut encrypted_comms.sender,
        };
        sender.send(r).map_err(|_| ("Communications channel broken"))
    }

    /// Blocks until a command is received. If the channel is closed (i.e. the boss has disconnected),
    /// then returns Err. Note that normally the boss should send us a Shutdown command rather than
    /// just disconnecting, but in the case of errors, this may not happen so we want to deal with this 
    /// cleanly too.
    pub fn receive_command(&mut self) -> Result<Command, &'static str> {
        trace!("Waiting for command from {}", &self);
        let receiver = match self {
            Comms::Local { receiver, .. } => receiver,
            Comms::Remote { encrypted_comms, .. } => &mut encrypted_comms.receiver,
        };
        receiver.recv().map_err(|_| ("Communications channel broken"))
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
        let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or(args.log_filter));
        builder.target(env_logger::Target::Stderr);
        // Configure format so that the boss can parse and re-log it
        builder.format(|buf, record| {
            writeln!(
                buf,
                "{} {} {}",
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
            ("doer", "remote boss"),
    )};

    if let Err(e) = message_loop(&mut comms) {
        debug!("doer process finished with error: {:?}", e);
        return ExitCode::from(20)
    }

    stop_timer(main_timer);

    if let Comms::Remote{ encrypted_comms } = comms { // This is always true, we just need a way of getting the fields
        #[cfg(feature="profiling")]
        // Send our profiling data (if enabled) back to the boss process so it can combine it with its own
        encrypted_comms.shutdown_with_final_message_sent_after_threads_joined(|| Response::ProfilingData(get_local_process_profiling()));
        #[cfg(not(feature="profiling"))]
        encrypted_comms.shutdown(); // Simple clean shutdown
    }

    // Dump memory usage figures when used for benchmarking. There isn't a good way of determining this from the benchmarking app
    // (especially for remote processes), so we instrument it instead.
    if args.dump_memory_usage {
        info!("Doer peak memory usage: {}", profiling::get_peak_memory_usage());
    }
   
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
        #[cfg(feature="profiling")]
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

fn handle_get_entries(comms: &mut Comms, context: &mut DoerContext, filters: Filters) -> Result<(), String> {
    let start = Instant::now();
    // Due to the way the WalkDir API works, we unfortunately need to do the iter loop manually
    // so that we can avoid normalizing the path twice (once for the filter, once for the conversion
    // of the entry to our representation).
    // Note that we can't use this to get metadata for a single root entry when that entry is a broken symlink,
    // as the walk will fail before we can get the metadata for the broken link. Therefore we only use this
    // when walking what's known to be a directory (discovered in SetRoot).
    let mut walker_it = WalkDir::new(&context.root)
        .follow_links(false)  // We want to see the symlinks, not their targets
        // To ensure deterministic order, mainly for tests. If this turns out to have a performance impact,
        // we could enable it only for tests perhaps.
        .sort_by_file_name() 
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

                if apply_filters(&path, &filters) == FilterResult::Exclude {
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

                comms.send_response(Response::Entry((path, d)))?;
            }
        }
        count += 1;
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
        let filters = Filters { 
            regex_set: RegexSet::new(&["^.*$"]).unwrap(),
            kinds: vec![FilterKind::Exclude]
        };
        assert_eq!(apply_filters(&RootRelativePath { inner: "will be excluded".to_string() }, &filters), FilterResult::Exclude);
        // But the root is always included anyway
        assert_eq!(apply_filters(&RootRelativePath::root(), &filters), FilterResult::Include);
    }

    #[test]
    fn test_apply_filters_no_filters() {
        let filters = Filters { 
            regex_set: RegexSet::empty(),
            kinds: vec![]
        };
        assert_eq!(apply_filters(&RootRelativePath { inner: "yes".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "no".to_string() }, &filters), FilterResult::Include);
    }

    #[test]
    fn test_apply_filters_single_include() {
        let filters = Filters { 
            regex_set: RegexSet::new(&["^yes$"]).unwrap(),
            kinds: vec![FilterKind::Include]
        };
        assert_eq!(apply_filters(&RootRelativePath { inner: "yes".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "no".to_string() }, &filters), FilterResult::Exclude);
    }

    #[test]
    fn test_apply_filters_single_exclude() {
        let filters = Filters { 
            regex_set: RegexSet::new(&["^no$"]).unwrap(),
            kinds: vec![FilterKind::Exclude]
        };
        assert_eq!(apply_filters(&RootRelativePath { inner: "yes".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "no".to_string() }, &filters), FilterResult::Exclude);
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
        assert_eq!(apply_filters(&RootRelativePath { inner: "README".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "build/file.o".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "git/hash".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "build/rob".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "build/output.exe".to_string() }, &filters), FilterResult::Include);
        assert_eq!(apply_filters(&RootRelativePath { inner: "src/build/file.o".to_string() }, &filters), FilterResult::Exclude);
        assert_eq!(apply_filters(&RootRelativePath { inner: "src/source.cpp".to_string() }, &filters), FilterResult::Include);
    }
}
