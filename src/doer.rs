use std::{sync::mpsc::{Sender, Receiver}, time::{Instant, SystemTime}, fmt::{Display, self}, io::{Write}, path::{PathBuf, Path}};
use clap::Parser;
use log::{error, debug};
use serde::{Serialize, Deserialize};
use walkdir::WalkDir;

use crate::*;

#[derive(clap::Parser)]
struct DoerCliArgs {
    /// [Internal] Launches as a doer process, rather than a boss process. 
    /// This shouldn't be needed for regular operation.
    #[arg(long)]
    doer: bool,
}

/// Converts a platform-specific relative path (inside the source or dest root)
/// to something that can be sent over our comms. We can't simply use PathBuf
/// because the syntax of this path might differ between the boss and doer
/// platforms (e.g. Windows vs Linux), and so the type might have different 
/// meaning/behaviour on each side.
/// We instead convert a normalized representation using forward slashes (i.e. Unix-style).
fn normalize_path(p: &Path) -> Result<String, String> {
    if p.is_absolute() {
        return Err("Must be relative".to_string());
    }

    let mut result = String::new();
    for c in p.iter() {
        let cs = match c.to_str() {
            Some(x) => x,
            None => return Err("Can't convert path component".to_string()),
        };
        if !result.is_empty() {
            result += "/";
        }
        result += cs;
    }

    return Ok(result);
}

/// Commands are sent from the boss to the doer, to request something to be done.
#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    // Note we shouldn't use PathBufs, because the syntax of this path might differ between the boss and doer
    // platforms (e.g. Windows vs Linux), and so the type might have different meaning/behaviour on each side.
    //TODO: what format do we use then, do we need to convert between them??
    GetEntries {
        root: String,
    },
    GetFileContent {
        path: String,
    },
    CreateOrUpdateFile {
        path: String,
        data: Vec<u8>,
        set_modified_time: Option<SystemTime>, //TODO: is this compatible between platforms, time zone changes, precision differences, etc. etc.
        //TODO: can we safely serialize this on one platform and deserialize on another?
    },
    CreateFolder {
        path: String,
    },
    DeleteFile {
        path: String,
    },
    DeleteFolder {
        path: String,
    },
   Shutdown,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum EntryType {
    File,
    Folder,
}

/// Details of a file or folder.
//TODO: use an enum as folders don't have modified_time or size!
#[derive(Serialize, Deserialize, Debug)]
pub struct EntryDetails {
    pub path: String,
    pub entry_type: EntryType,
    pub modified_time: SystemTime, //TODO: is this compatible between platforms, time zone changes, precision differences, etc. etc.
    //TODO: can we safely serialize this on one platform and deserialize on another?
    pub size: u64,
}

/// Responses are sent back from the doer to the boss to report on something, usually
/// the result of a Command.
#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    // The result of GetEntries is split into lots of individual messages (rather than one big list) 
    // so that the boss can start doing stuff before receiving the full list.
    Entry(EntryDetails),
    EndOfEntries,

    FileContent {
        data: Vec<u8>
    },

    Ack,
    Error(String),
}

/// Abstraction of two-way communication channel between this doer and the boss, which might be 
/// remote (communicating through our stdin and stdout) or local (communicating via a channel to the main thread).
enum Comms {
    Local {
        sender: Sender<Response>,
        receiver: Receiver<Command>,
    },
    Remote, // Remote doesn't need to store anything, as it uses the process' stdin and stdout which are globally available
}
impl Comms {
    fn send_response(&self, r: Response) -> Result<(), String> {
        debug!("Sending response {:?} to {}", r, &self);
        let mut res;
        match self {
            Comms::Local { sender, receiver: _ } => {
                res = sender.send(r).map_err(|e| e.to_string());
            },
            Comms::Remote => {
                res = bincode::serialize_into(std::io::stdout(), &r).map_err(|e| e.to_string());
                if res.is_ok() {
                    res = std::io::stdout().flush().map_err(|e| e.to_string()); // Otherwise could be buffered and we hang!
                }
            }
        }
        if res.is_err() {
            error!("Error sending response: {:?}", res);
        }
        return res;
   }

    fn receive_command(&self) -> Result<Command, String> {
        debug!("Waiting for command from {}", &self);
        let c;
        match self {
            Comms::Local { sender: _, receiver } => {
                c = receiver.recv().map_err(|e| e.to_string());
            },
            Comms::Remote => {
                c = bincode::deserialize_from(std::io::stdin()).map_err(|e| e.to_string());
            },
        }
        debug!("Received command {:?} from {}", c, &self);
        return c;
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
    // Configure logging. 
    // Note that we can't use stdout as that is our communication channel with the boss.
    // We use stderr instead, which the boss will read from and echo for easier debugging.
    // TODO: We could additionally log to a file, which might be useful for cases where the logs don't
    // make it back to the boss (e.g. communication errors)
    stderrlog::StdErrLog::new().verbosity(log::Level::Debug).init().unwrap();

    let _args = DoerCliArgs::parse();

    // We take commands from our stdin and send responses on our stdout. These will be piped over ssh
    // back to the Boss.

    // The first thing we send is a special handshake message that the Boss will recognise, 
    // to know that we've started up correctly and to make sure we are running compatible versions.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    let msg = format!("{}{}", HANDSHAKE_MSG, VERSION);
    println!("{}", msg);
    eprintln!("{}", msg);

    // If the Boss isn't happy (e.g. we are an old version), they will stop us and deploy a new version.
    // So at this point we can assume they are happy and move on to processing commands they (might) send us.
    match message_loop(Comms::Remote) {
        Ok(_) => {
            debug!("doer process finished successfully!");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            debug!("doer process finished with error: {:?}", e);
            return ExitCode::from(20);
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
fn message_loop (comms: Comms) -> Result<(), ()> {
    let mut context = DoerContext { root: PathBuf::new() };
    loop {
        match comms.receive_command() {
            Ok(c) => {
                if !exec_command(c, &comms, &mut context) {
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
fn exec_command(command : Command, comms: &Comms, context: &mut DoerContext) -> bool {
    match command {
        Command::GetEntries { root } => {
            // Store the root folder for future operations
            context.root = PathBuf::from(root);

            let start = Instant::now();
            // Don't follow symlinks - we want to sync the links themselves
            let walker = WalkDir::new(&context.root).follow_links(false).into_iter();
            let mut count = 0;
            for entry in walker.filter_entry(|_e| true) {
                match entry {
                    Ok(e) => {
                        let entry_type;
                        if e.file_type().is_dir() {
                            entry_type = EntryType::Folder;
                        } else if e.file_type().is_file() { 
                            entry_type = EntryType::File;
                        } else {
                            comms.send_response(Response::Error("Unknown file type".to_string())).unwrap();
                            return true;
                        }

                        // Paths returned by WalkDir will include the root, but we want paths relative to the root
                        let path = e.path().strip_prefix(&context.root).unwrap();
                        // Convert to platform-agnostic representation
                        let path = match normalize_path(path) {
                            Ok(p) => p,
                            Err(e) => {
                                comms.send_response(Response::Error(format!("normalize_path failed: {}", e))).unwrap();
                                return true;    
                            }
                        };

                        let metadata = match e.metadata() {
                            Ok(m) => m,
                            Err(e) => {
                                comms.send_response(Response::Error(format!("Unable to get metadata: {}", e))).unwrap();
                                return true;                                   
                            }
                        };

                        let modified_time = match metadata.modified() {
                            Ok(m) => m,
                            Err(e) => {
                                comms.send_response(Response::Error(format!("Unknown modified time: {}", e))).unwrap();
                                return true;                                   
                            }
                        };

                        let d = EntryDetails {
                            path: path.to_string(),
                            entry_type,
                            modified_time,
                            size: metadata.len()
                        };

                        comms.send_response(Response::Entry(d)).unwrap();
//                      if e.file_type().is_file() {
//                         let bytes = std::fs::read(e.path()).unwrap();
//                         let hash = md5::compute(&bytes);
//                         hash_sum += hash.into_iter().sum::<u8>();
//                         count += 1;
//                      }
                   }
                    Err(e) => {
                        comms.send_response(Response::Error(e.to_string())).unwrap();
                        break;
                    }
                }
                count += 1;
            }
            let elapsed = start.elapsed().as_millis();
            comms.send_response(Response::EndOfEntries).unwrap();
            debug!("Walked {} in {}ms ({}/s)", count, elapsed, 1000.0 * count as f32 / elapsed as f32);
        },
        Command::GetFileContent { path } => {
            let full_path = context.root.join(&path);
            match std::fs::read(full_path) {
                Ok(data) => comms.send_response(Response::FileContent{ data }).unwrap(),
                Err(e) =>  comms.send_response(Response::Error(e.to_string())).unwrap(),
            }
        },
        Command::CreateOrUpdateFile { path, data, set_modified_time } => {
            let full_path = context.root.join(&path);
            let r = std::fs::write(&full_path, data);
            if let Err(e) = r {
                comms.send_response(Response::Error(e.to_string())).unwrap();
                return true;
            }

            // After changing the content, we need to override the modified time of the file to that of the original,
            // otherwise it will immediately count as modified again if we do another sync.
            if let Some(t) = set_modified_time {
                let r = filetime::set_file_mtime(&full_path, filetime::FileTime::from_system_time(t));
                if let Err(e) = r {
                    comms.send_response(Response::Error(e.to_string())).unwrap();
                    return true;
                }  
            }

            comms.send_response(Response::Ack).unwrap();
        },
        Command::CreateFolder { path } => {
            let full_path = context.root.join(&path);
            match std::fs::create_dir(full_path) { 
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) =>  comms.send_response(Response::Error(e.to_string())).unwrap(),
            }
        }
        Command::DeleteFile { path } => {
            let full_path = context.root.join(&path);
            match std::fs::remove_file(full_path) { 
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) =>  comms.send_response(Response::Error(e.to_string())).unwrap(),
            }
        }
        Command::DeleteFolder { path } => {
            let full_path = context.root.join(&path);
            match std::fs::remove_dir(full_path) {
                Ok(()) => comms.send_response(Response::Ack).unwrap(),
                Err(e) =>  comms.send_response(Response::Error(e.to_string())).unwrap(),
            }
        }
        Command::Shutdown => {
            return false;
        }
    }
    return true;
}