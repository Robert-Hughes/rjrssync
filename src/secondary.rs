use std::{io::{stdin}, sync::mpsc::{Sender, Receiver, RecvError}, time::Instant};
use clap::Parser;
use log::{info};
use serde::{Serialize, Deserialize};
use walkdir::WalkDir;

use crate::*;

#[derive(clap::Parser)]
struct SecondaryCliArgs {
    #[arg(short, long)]
    secondary: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    GetFiles {
        root: String,
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    // Split into lots of individual messages (rather than one big file list) so that the primary can start doing stuff before receiving the full list
    File(String), 
    Error(String)
}

enum Comms {
    Local { // Local uses mpsc to communicate with the primary thread
        sender: Sender<Response>,
        receiver: Receiver<Command>,
    },
    Remote // Remote doesn't need to store anything, as it uses the process' stdin and stdout
}
impl Comms {
    fn send_response(&self, r: Response) {
        match self {
            Comms::Local => {
                error!("Not implemented!");
            },
            Comms::Remote { stdin, stdout, stderr } => {
                error!("Not implemented!");
            }
        }
    }

    fn receive_command(&self) -> Command {
        match self {
            Comms::Local => {
                match receiver.recv() {
                    Ok(c) => exec_command(c, &sender),
                    Err(RecvError) => break,
                }
            },
            Comms::Remote { stdin, stdout, stderr } => {
                match bincode::deserialize_from::<std::io::Stdin, Command>(stdin()) {
                    Ok(c) => {
                        exec_command(c, sender);
                    }
                    Err(e) => {
                        return ExitCode::from(22);
                    }
                }
            },
        }
    }
}

pub fn secondary_main() -> ExitCode {
    info!("Running as secondary");

    let _args = SecondaryCliArgs::parse();

    // We take commands from our stdin and send responses on our stdout. These will be piped over ssh
    // back to the Primary.

    // The first thing we send is a special message that the Primary will recognise, to know that we've started up correctly
    // and to make sure we are running compatible versions etc.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    let msg = format!("{}{}", SECONDARY_HANDSHAKE_MSG, VERSION);
    println!("{}", msg);
    eprintln!("{}", msg);

    // If the Primary isn't happy, they will stop us and deploy a new version. So at this point we can assume
    // they are happy and move on to processing commands they (might) send us

    // Message-processing loop, until Primary disconnects.
    message_loop(Comms::Remote);

   // info!("Primary disconnected!");

    return ExitCode::from(22);
}

pub fn secondary_thread_running_on_primary(receiver: Receiver<Command>, sender: Sender<Response>) {
    // Message-processing loop, until Primary disconnects.
    message_loop(Comms::Local { sender, receiver });
}

fn message_loop (comms: Comms) {
    loop {
        match comms.receive_command() {
            Ok(c) => {
                exec_command(c, &comms);
            }
            Err(e) => {
                return ExitCode::from(22);
            }
        }
    }
}

fn exec_command(command : Command, comms: &Comms) {
    match command {
        Command::GetFiles { root } => {
            let start = Instant::now();
            let walker = WalkDir::new(&root).into_iter();
            let mut count = 0;
      //      for entry in walker.filter_entry(|e| e.file_name() != ".git" && e.file_name() != "dependencies") {
            for entry in walker {
                comms.send_response(Response::File(entry.unwrap().file_name().to_str().unwrap().to_string()));
                count += 1;
            }
            let elapsed = start.elapsed().as_millis();
            //println!("Walked {} in {} ({}/s)", count, elapsed, 1000.0 * count as f32 / elapsed as f32);

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
      //  info!("Received data from Primary: {}", buf[0]);

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
}