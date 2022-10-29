use std::net::{TcpListener, TcpStream};

use clap::Parser;
use log::{info, error};

#[derive(clap::Parser)]
struct CliArgs {
    src: String,
    dest: String,
}

fn parse_remote_folder(s: &str) -> (String, String) {
    match s.split_once(':') {
        None => ("localhost".to_string(), s.to_string()),
        Some((a, b)) => (a.to_string(), b.to_string())
    }
}

fn main() {
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let args = CliArgs::parse();

    // The src and/or dest may be on another computer. We need to run a copy of rjrssync on the remote 
    // computer(s) and set up network commmunication. For consistency, we assume both are remote, even if they
    // are actually local.
    // There are therefore three copies of our program involved (although some may actually be the same as each other)
    //   Initiator - this copy, which received the command line from the user
    //   Source - the computer specified by the `src` command-line arg, which may simply be the local computer
    //   Dest - the computer specified by the `dest` command-line arg, which may simply be the local computer, 
    //          and/or it may be the same as the Source computer.

    // Get list of hosts to launch and estabilish communication with
    let (src_host, src_folder) = parse_remote_folder(&args.src);
    let (dest_host, dest_folder) = parse_remote_folder(&args.dest);

    // Launch rjrssync (if not already running) on both remote hosts and estabilish communication
    let src_comms = setup_comms(&src_host);
    if src_comms.is_none() {
        return;
    }
    let src_comms = src_comms.unwrap();

    let dest_comms = setup_comms(&dest_host);
    if dest_comms.is_none() {
        return;
    }
    let dest_comms = dest_comms.unwrap();

    //TODO: tell the src to initiate the command, and it will contact the dest to request/send stuff

    // Start command-processing loop, listening on a port for other instances to connect to us and make requests.
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    info!("Waiting for incoming connections...");
    for stream in listener.incoming() {
        let stream = stream.unwrap();

        info!("Incoming connection from {:?}", stream);
    }


    //TODO: if get a message telling us to start a transfer, setup_comms with the dest.
    //TODO:     get a list of all the files in the local dir, and ask the dest to do the same
    //TODO:     compare the lists
    //TODO:     send over any files that have changed

    //TODO: if get a message telling us to provide a file list, do so

    //TODO: if get a message telling us to receive a file, do so
}

fn setup_comms(remote_host: &str) -> Option::<TcpStream> {
    let remote_addr = remote_host.to_string() + ":7878";

    info!("setup_comms with '{}'", remote_addr);

    if let Ok(stream) = TcpStream::connect(&remote_addr) {
        info!("Connected to '{}'", &remote_addr);
        return Some(stream);
    } else {
        error!("Couldn't connect to '{}'", &remote_addr);
        return None;
    }
    

    //TODO: attempt to connect to an already running instance, to save time
    //TODO: check the version is correct
    //TODO: if not, sync sources and run cargo build/run?
}