use std::{net::{TcpListener, TcpStream}, io::{Write, Read}};

use clap::Parser;
use log::{info, error, warn};

const VERSION: i32 = 1;

#[derive(clap::Parser)]
struct CliArgs {
    src: String,
    dest: String,
}
#[derive(clap::Parser)]
struct DaemonCliArgs {
    
}


fn parse_remote_folder(s: &str) -> (String, String) {
    match s.split_once(':') {
        None => ("localhost".to_string(), s.to_string()),
        Some((a, b)) => (a.to_string(), b.to_string())
    }
}

fn main() {
    simple_logger::SimpleLogger::new().env().init().unwrap();

    if std::env::args().any(|a| a == "--daemon") {
        daemon_main();
    } else 
    {
        cli_main();
    }
}

fn cli_main() {
    info!("Running as CLI");

    let args = CliArgs::parse();

    // The src and/or dest may be on another computer. We need to run a copy of rjrssync on the remote 
    // computer(s) and set up network commmunication. For consistency, we assume both are remote, even if they
    // are actually local.
    // There are therefore three copies of our program involved (although some may actually be the same as each other)
    //   Initiator - this copy, which received the command line from the user
    //   Source - runs on the computer specified by the `src` command-line arg, which may simply be the local computer.
    //            This is still a different instance to the Initiator, as the Initiator will close once the transfer is done,
    //            but the daemon will keep running in the background to serve other requests.
    //   Dest - the computer specified by the `dest` command-line arg, which may simply be the local computer, 
    //          and/or it may be the same as the Source computer.
    //
    // Note that we don't strictly need to set up the Dest now, as the Source could do that once we instruct it, however
    // it's easier to check for communication errors in the Initiator process, as we can report these nicely to the user.
    // Also, in future we may want to use the Initiator as some sort of network bridge between Source and Dest (e.g. if
    // Dest isn't reachable from Source).

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
    error!("Not implemented!");
}

fn daemon_main() {
    info!("Running as background daemon");

    // Start command-processing loop, listening on a port for other instances to connect to us and make requests.
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    info!("Waiting for incoming connections...");
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();

        info!("Incoming connection from {:?}", stream);

        // Spawn a new thread to look after this connection. We need to be able to keep listening for new connections
        // on the main thread (we can't block it), as when Source and Dest computers are the same, we talk to ourselves
        // and would deadlock!
        std::thread::spawn(move || { daemon_connection_handler(stream) });        
    }
}

fn daemon_connection_handler(mut stream: TcpStream) {
    // Send our version number, so the client can check if it's compatible.
    // Note that the 'protocol' used to check version number etc. needs to always be backwards-compatible!
    info!("Sending version number {}", VERSION);
    if stream.write(&VERSION.to_le_bytes()).is_err() {
        warn!("Error - giving up on this client");
        return;
    }

    // Wait for the client to acknowledge the version, or ask us to shut down so they can spawn a new version
    // that is compatible with them
    info!("Waiting for reply");
    let mut buf: [u8; 1] = [0];
    if stream.read(&mut buf).is_err() {
        warn!("Error - giving up on this client");
        return;
    }

    if buf[0] == 0 {
        info!("Client is happy");
    } else {
        info!("Client is unhappy - terminating");     
        //TODO: how do this from background thread??       
        return;
    }

    // Message-processing loop, until client disconnects.
    while true { //TODO: stop once connection is closed!

        //TODO: if get a message telling us to start a transfer, setup_comms with the dest.
        //TODO:     get a list of all the files in the local dir, and ask the dest to do the same
        //TODO:     compare the lists
        //TODO:     send over any files that have changed

        //TODO: if get a message telling us to provide a file list, do so

        //TODO: if get a message telling us to receive a file, do so
    }

}

fn setup_comms(remote_host: &str) -> Option::<TcpStream> {
    let remote_addr = remote_host.to_string() + ":7878";

    info!("setup_comms with '{}'", remote_addr);

    // Attemnpt to connect to an already running instance, to save time
    if let Ok(mut stream) = TcpStream::connect(&remote_addr) {
        info!("Connected to '{}'", &remote_addr);
 
        // Wait for the server to send their version, so we can check if it's compatible with ours
    // Note that the 'protocol' used to check version number etc. needs to always be backwards-compatible!
    let mut server_version = -1;
        info!("Waiting for version number");
        let mut buf: [u8; 4] = [0; 4];
        if stream.read(&mut buf).is_ok() {
            server_version = i32::from_le_bytes(buf);
            info!("Received server version {}", server_version);
        } else {
            warn!("Server is not replying - attempting to start new one anyway");
        }

        if server_version == VERSION {
            info!("Server has compatible version. Replying as such.");
            // Send packet to tell server all is OK
            if stream.write(&[0 as u8]).is_ok() {
                info!("Connection estabilished!");
                return Some(stream);
            } else {
                warn!("Failed to reply to server - attempting to start a new one anyway");
            }
        } else {
            info!("Server has incompatible version - restarting it");
            // Send packet to tell server to restart
            stream.write(&[1 as u8]); // Don't need to check result here - even if it failed, we will still try to launch new server
        }
    }

    // No instance running - spawn a new one
    info!("Spawning new daemon on '{}'", &remote_addr);
  
    //TODO: sync sources and run cargo build/run?
    error!("Not implemented!");
    return None;
}