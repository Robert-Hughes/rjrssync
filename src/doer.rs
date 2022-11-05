use std::{sync::mpsc::{Sender, Receiver}, time::Instant, fmt::{Display, self}, io::Write};
use clap::Parser;
use log::{info, error, debug};
use serde::{Serialize, Deserialize};
use walkdir::WalkDir;

use crate::*;

#[derive(clap::Parser)]
struct DoerCliArgs {
    #[arg(short, long)]
    doer: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    GetFiles {
        root: String,
    },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    // Split into lots of individual messages (rather than one big file list) so that the boss can start doing stuff before receiving the full list
    File(String),
    EndOfFileList,
    Error(String)
}

enum Comms {
    Local { // Local uses mpsc to communicate with the boss thread
        sender: Sender<Response>,
        receiver: Receiver<Command>,
    },
    Remote // Remote doesn't need to store anything, as it uses the process' stdin and stdout
}
impl Comms {
    fn send_response(&self, r: Response) -> Result<(), String> {
        info!("Sending response {:?} to {}", r, &self);
        let res;
        match self {
            Comms::Local { sender, receiver: _ } => {
                res = sender.send(r).map_err(|e| e.to_string());
            },
            Comms::Remote => {
                res = bincode::serialize_into(std::io::stdout(), &r).map_err(|e| e.to_string());
                std::io::stdout().flush().unwrap(); // Otherwise could be buffered and we hang!
            }
        }
        if res.is_err() {
            error!("Error sending response: {:?}", res);
        }
        return res;
   }

    fn receive_command(&self) -> Result<Command, String> {
        info!("Waiting for command from {}", &self);
        let c;
        match self {
            Comms::Local { sender: _, receiver } => {
                c = receiver.recv().map_err(|e| e.to_string());
            },
            Comms::Remote => {
                c = bincode::deserialize_from(std::io::stdin()).map_err(|e| e.to_string());
            },
        }
        info!("Received command {:?} from {}", c, &self);
        return c;
    }
}
impl Display for Comms {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Comms::Local { .. } => {
                write!(f, "Local")
            },
            Comms::Remote { .. } => {
                write!(f, "Remote")
            }
        }
    }
}
//TODO: impl Drop?

pub fn doer_main() -> ExitCode {
    // Configure logging. Note that we can't use stdout as that is our communication channel with the boss!
    // We use stderr instead.
    //TODO: who is reading stderr? :O Log to file instead?
   // stderrlog::StdErrLog::new().init().unwrap();

    info!("Running as doer");

    let _args = DoerCliArgs::parse();

    // We take commands from our stdin and send responses on our stdout. These will be piped over ssh
    // back to the Boss.

    // The first thing we send is a special message that the Boss will recognise, to know that we've started up correctly
    // and to make sure we are running compatible versions etc.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    let msg = format!("{}{}", HANDSHAKE_MSG, VERSION);
    println!("{}", msg);
    eprintln!("{}", msg);

    // If the Boss isn't happy, they will stop us and deploy a new version. So at this point we can assume
    // they are happy and move on to processing commands they (might) send us

    // Message-processing loop, until Boss disconnects.
    message_loop(Comms::Remote).unwrap();

   // info!("Boss disconnected!");

    return ExitCode::from(22);
}

pub fn doer_thread_running_on_boss(receiver: Receiver<Command>, sender: Sender<Response>) {
    debug!("doer thread running");
    // Message-processing loop, until Boss disconnects.
    message_loop(Comms::Local { sender, receiver }).unwrap();
    debug!("doer thread finished");
}

fn message_loop (comms: Comms) -> Result<(), ()> {
    loop {
        match comms.receive_command() {
            Ok(c) => {
                if !exec_command(c, &comms) {
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

fn exec_command(command : Command, comms: &Comms) -> bool {
    match command {
        Command::GetFiles { root } => {
            let start = Instant::now();
            let walker = WalkDir::new(&root).into_iter();
            let mut _count = 0;
      //      for entry in walker.filter_entry(|e| e.file_name() != ".git" && e.file_name() != "dependencies") {
            for entry in walker {
                match entry {
                    Ok(e) => {
                        comms.send_response(Response::File(e.file_name().to_str().unwrap().to_string())).unwrap();
                    }
                    Err(e) => {
                        comms.send_response(Response::Error(e.to_string())).unwrap();
                        break;
                    }
                }
                _count += 1;
            }
            let _elapsed = start.elapsed().as_millis();
            comms.send_response(Response::EndOfFileList).unwrap();
            //println!("Walked {} in {} ({}/s)", count, elapsed, 1000.0 * count as f32 / elapsed as f32);

        }
        Command::Shutdown => {
            return false;
        }
 //     {
//         let start = Instant::now();
//         let walker = WalkDir::new(&args.path).into_iter();
//         let mut hash_sum: u8 = 0;
//         let mut count = 0;
//         for entry in walker.filter_entry(|e| e.file_name() != ".git" && e.file_name() != "dependencies") {
//             let e = entry.unwrap();
//             if e.file_type().is_file() {
//                 let bytes = std::fs::read(e.path()).unwrap();
//                 let hash = md5::compute(&bytes);
//                 hash_sum += hash.into_iter().sum::<u8>();
//                 count += 1;
//             }
//         }
//         let elapsed = start.elapsed().as_millis();
//         println!("Hashed {} ({}) in {} ({}/s)", count, hash_sum, elapsed, 1000.0 * count as f32 / elapsed as f32);
//     }

//  Host:           Windows     Linux
//  Filesystem:
//    Windows        100k        9k
//     Linux          1k         500k

//    let mut buf: [u8; 1] = [0];
//    while stdin().read(&mut buf).unwrap_or(0) > 0 {
      //  info!("Received data from Boss: {}", buf[0]);

        // echo back
//        stdout().write(&buf).unwrap();

        //exec_command();

        //TODO: if get a message telling us to start a transfer, setup_comms(false) with the dest.
        //        (false cos the Dest should already have been set up with new version if necessary, so don't do it again)
        //TODO:     get a list of all the files in the local dir, and ask the dest to do the same
        //TODO:     compare the lists
        //TODO:     send over any files that have changed

        //TODO: if get a message telling us to provide a file list, do so

        //TODO: if get a message telling us to receive a file, do so
 //   }


    }
    return true;
}