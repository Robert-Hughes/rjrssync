use std::{sync::mpsc::{Sender, Receiver}, time::Instant, fmt::{Display, self}, io::{Write}};
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

/// Commands are sent from the boss to the doer, to request something to be done.
#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    GetFiles {
        root: String,
    },
    Shutdown,
}

/// Responses are sent back from the doer to the boss to report on something, usually
/// the result of a Command.
#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    // The result of GetFiles is split into lots of individual messages (rather than one big file list) 
    // so that the boss can start doing stuff before receiving the full list.
    File(String),
    EndOfFileList,

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

// Repeatedly waits for Commands from the boss and processes them (possibly sending back Responses).
// This function returns when we receive a Shutdown Command, or there is an unrecoverable error
// (recoverable errors while handling Commands will not stop the loop).
fn message_loop (comms: Comms) -> Result<(), ()> {
    loop {
        match comms.receive_command() {
            Ok(c) => {
                if !exec_command(c, &comms) {
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
fn exec_command(command : Command, comms: &Comms) -> bool {
    match command {
        Command::GetFiles { root } => {
            let start = Instant::now();
            let walker = WalkDir::new(&root).into_iter();
            let mut count = 0;
            for entry in walker.filter_entry(|_e| true) {
                match entry {
                    Ok(e) => {
                        comms.send_response(Response::File(e.file_name().to_str().unwrap().to_string())).unwrap();
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
            comms.send_response(Response::EndOfFileList).unwrap();
            debug!("Walked {} in {}ms ({}/s)", count, elapsed, 1000.0 * count as f32 / elapsed as f32);
        }
        Command::Shutdown => {
            return false;
        }
    }
    return true;
}