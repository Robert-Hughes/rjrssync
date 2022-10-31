use std::{net::{TcpListener, TcpStream}, io::{Write, Read}, path::PathBuf};
use clap::Parser;
use log::{info, error, warn};
use rust_embed::RustEmbed;
use tempdir::TempDir;
use std::process::{Command};

const VERSION: i32 = 1;
const MAGIC: [u8; 4] = [19, 243, 129, 88];

#[derive(clap::Parser)]
struct CliArgs {
    src: String,
    dest: String,
}
#[derive(clap::Parser)]
struct DaemonCliArgs {
    #[arg(short, long)]
    daemon: bool,
}

#[derive(Default)]
struct RemoteFolderDesc {
    user: String,
    hostname: String,
    folder: String,
}

fn parse_remote_folder(s: &str) -> RemoteFolderDesc {
    let mut r = RemoteFolderDesc::default();

    let after_user;
    match s.split_once('@') {
        None => after_user = s,
        Some((a, b)) => {
            r.user = a.to_string();
            after_user = b;
        }
    };
    match after_user.split_once(':') {
        None => {
            r.hostname = "localhost".to_string();
            r.folder = after_user.to_string();
        },
        Some((a, b)) => {
            r.hostname = a.to_string();
            r.folder = b.to_string();
        }
    };

    return r;
}

fn main() {
    // Configure logging
    simple_logger::SimpleLogger::new().env().init().unwrap();

    // The process can run as either a CLI which takes input from the command line, performs
    // a transfer and then exits once complete, or as a background daemon which stays alive
    // and processes commands until it is told to exit. Daemon(s) are used on each of the src
    // and dest computers to perform a transfer.
    // The daemon and CLI modes have different command-line arguments, so handle them separately.
    if std::env::args().any(|a| a == "--daemon") {
        daemon_main();
    } else {
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
    //          and/or it may be the same as the Source computer. If Source and Dest are the same, then that daemon process
    //          will 'talk to itself'.
    //
    // Note that we don't strictly need to set up the Dest now, as the Source could do that once we instruct it, however
    // it's easier to check for communication errors in the Initiator process, as we can report these nicely to the user.
    // Also, in future we may want to use the Initiator as some sort of network bridge between Source and Dest (e.g. if
    // Dest isn't reachable from Source).

    // Get list of hosts to launch and estabilish communication with
    let src_folder_desc = parse_remote_folder(&args.src);
    let dest_folder_desc = parse_remote_folder(&args.dest);

    // Launch rjrssync (if not already running) on both remote hosts and estabilish communication (check version etc.)
    let src_comms = setup_comms(&src_folder_desc.hostname, &src_folder_desc.user, true);
    if src_comms.is_none() {
        return;
    }
    let _src_comms = src_comms.unwrap();

    let dest_comms = setup_comms(&dest_folder_desc.hostname, &dest_folder_desc.user, true);
    if dest_comms.is_none() {
        return;
    }
    let _dest_comms = dest_comms.unwrap();

    //TODO: tell the src to initiate the command, and it will contact the dest to request/send stuff as necessary
    error!("Instruct src to begin transfer - Not implemented!");
}

fn daemon_main() {
    info!("Running as background daemon");

    let _args = DaemonCliArgs::parse();

    // Start command-processing loop, listening on a port for other instances to connect to us and make requests.
    //TODO: will need to listen on other interfaces so can accept remote connections, but might want to fix
    // security issues first! 
    //TODO: Even while we're just listening on localhost though, we're giving access to other local users to files
    // which they might not be allowed!
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    info!("Waiting for incoming connections...");
    for stream in listener.incoming() {
        let stream = stream.unwrap();

        info!("Incoming connection from {:?}", stream);

        // Spawn a new thread to look after this connection. We need to be able to keep listening for new connections
        // on the main thread (we can't block it), because for example when Source and Dest computers are the same, 
        // we talk to ourselves and would otherwise deadlock!
        std::thread::spawn(move || { daemon_connection_handler(stream) });        
    }
}

fn daemon_connection_handler(mut stream: TcpStream) {
    // Send our magic and version number, so the client can check if it's compatible.
    // Note that the 'protocol' used to check version number etc. needs to always be backwards-compatible,
    // so is very basic.
    info!("Sending version number {}", VERSION);
    let mut magic_and_version = MAGIC.to_vec();
    magic_and_version.append(&mut VERSION.to_le_bytes().to_vec());
    if stream.write(&magic_and_version).is_err() {
        warn!("Error - giving up on this client");
        return;
    }

    // Wait for the client to acknowledge the version, or ask us to shut down so they can spawn a new version
    // that is compatible with them
    info!("Waiting for reply");
    let mut buf: [u8; 1] = [0];
    if stream.read(&mut buf).unwrap_or(0) != 1 {
        warn!("Error - giving up on this client");
        return;
    }

    if buf[0] == 0 {
        info!("Client is happy");
    } else {
        info!("Client is unhappy - terminating so client can start a different version of the daemon");     
        std::process::exit(2);
    }

    // Message-processing loop, until client disconnects.
    let mut buf: [u8; 1] = [0];
    while stream.read(&mut buf).unwrap_or(0) > 0 {
        info!("Received data from client {:?}: {}", stream, buf[0]);

        //TODO: if get a message telling us to start a transfer, setup_comms(false) with the dest.
        //        (false cos the Dest should already have been set up with new version if necessary, so don't do it again)
        //TODO:     get a list of all the files in the local dir, and ask the dest to do the same
        //TODO:     compare the lists
        //TODO:     send over any files that have changed

        //TODO: if get a message telling us to provide a file list, do so

        //TODO: if get a message telling us to receive a file, do so
    }

    info!("Dropped client {:?}", stream);

}

fn setup_comms(remote_hostname: &str, remote_user: &str, allow_restart_remote_daemon_if_necessary: bool) -> Option::<TcpStream> {
    info!("setup_comms with '{}'", remote_hostname);

    let remote_addr = remote_hostname.to_string() + ":7878";

    // Attempt to connect to an already running instance, to save time
    //TODO: this seems to be quite slow, even when the remote is already listening.
    if let Ok(mut stream) = TcpStream::connect(&remote_addr) {
        info!("Connected to '{}'", &remote_addr);
 
        // Wait for the server to send their magic and version, so we can check if it's compatible with ours
        // Note that the 'protocol' used to check version number etc. needs to always be backwards-compatible,
        // so is very basic.
        let server_version;
        info!("Waiting for version number");
        let mut buf: [u8; 8] = [0; 8]; // 4 bytes for magic, 4 bytes for version
        if stream.read(&mut buf).unwrap_or(0) == 8 {
            let server_magic = &buf[0..4];
            if server_magic != MAGIC {
                error!("Server replied with wrong magic. Not attempting to restart it, as there may be an unknown process (not us) listening on that port and we don't want to interfere with it.");
                return None;
            }

            let mut b : [u8; 4] = [0; 4];
            b.copy_from_slice(&buf[4..8]);
            server_version = i32::from_le_bytes(b);
            info!("Received server version {}", server_version);
        } else {
            error!("Server is not replying as expected. Not attempting to restart it, as there may be an unknown process (not us) listening on that port and we don't want to interfere with it.");
            return None;
        }

        if server_version == VERSION {
            info!("Server has compatible version. Replying as such.");
            // Send packet to tell server all is OK
            if stream.write(&[0 as u8]).is_ok() {
                info!("Connection estabilished!");
                return Some(stream);
            } else {
                warn!("Failed to reply to server");
            }
        } else {
            if allow_restart_remote_daemon_if_necessary {
                info!("Server has incompatible version - telling it to stop so we can restart it");
                // Send packet to tell server to restart
                let _ = stream.write(&[1 as u8]); // Don't need to check result here - even if it failed, we will still try to launch new server
            } else {
                error!("Server has incompatible version, and we're not going to restart it.");
            }
        }
    } else {
        info!("No remote daemon running");
    }

    // No instance running - spawn a new one
    if allow_restart_remote_daemon_if_necessary {   
        if spawn_daemon_on_remote(remote_hostname, remote_user) {
            // Try again to connect to the new daemon. 
            // Don't allow this recursion to spawn a new daemon again though in case we still can't connect,
            // otherwise it would keep trying forever!
            let result = setup_comms(remote_hostname, remote_user, false);
            if result.is_none() {
                error!("Failed to setup_comms even after spawning a new daemon");
            }
            return result;
        } else {
            error!("Failed to spawn a new daemon. Please launch it manually.");
            return None;
        }
    } else {
        return None;
    }
}

// This embeds the source code of the program into the executable, so it can be deployed remotely and built on other platforms
#[derive(RustEmbed)]
#[folder = "."]
#[include = "src/*"]
#[include = "Cargo.*"]
struct EmbeddedSource;

fn spawn_daemon_on_remote(remote_hostname: &str, remote_user: &str) -> bool {
    info!("Spawning new daemon on '{}'", &remote_hostname);

    // Copy our embedded source tree to the remote, so we can build it there. 
    // (we can't simply copy the binary as it might not be compatible with the remote platform) 
    // We use the user's existing ssh/scp tool so that their config/settings will be used for 
    // logging in to the remote system (as opposed to using an ssh library called from our code).

    // Save to a temporary local folder
    let temp_dir = match TempDir::new("rjrssync") {
        Ok(x) => x,
        Err(e) => { 
            error!("Error creating temp dir: {}", e); 
            return false;
        }
    };
    info!("Writing embedded source to temp dir: {}", temp_dir.path().display());
    for file in EmbeddedSource::iter() {
        let temp_path = temp_dir.path().join("rjrssync").join(&*file); // Add an extra "rjrssync" folder with a fixed name (as opposed to the temp dir, whose name varies), to work around SCP weirdness below.

        if let Err(e) = std::fs::create_dir_all(temp_path.parent().unwrap()) {
            error!("Error creating folders for temp file: {}", e); 
            return false;
        }

        let mut f = match std::fs::File::create(&temp_path) {
            Ok(x) => x,
            Err(e) => { 
                error!("Error creating temp file {}: {}", temp_path.display(), e); 
                return false;
            }    
        };

        if let Err(e) = f.write_all(&EmbeddedSource::get(&file).unwrap().data) {
            error!("Error writing temp file: {}", e); 
            return false;
        }
    }

    // Deploy to remote target
    // Note we need to deal with the case where the the remote folder doesn't exist, and the case where it does, so 
    // we copy into /tmp (which should always exist).
    let remote_temp_folder = PathBuf::from("/tmp/rjrssync/");
    let user_prefix = if remote_user.is_empty() { "".to_string() } else { remote_user.to_string() + "@" };
    let source_spec = temp_dir.path().join("rjrssync");
    let remote_spec = user_prefix.clone() + remote_hostname + ":" + remote_temp_folder.parent().unwrap().to_str().unwrap();
    info!("Copying source to {}", remote_spec);
    match Command::new("scp")
            .arg("-r")
            .arg(source_spec)
            .arg(remote_spec)
            .status() {
        Err(e) => {
            error!("Error launching scp: {}", e); 
            return false;
        },
        Ok(s) if s.code() == Some(0) => {
            // good!
        }
        Ok(s) => {
            error!("Error copying source code. Exit status from scp: {}", s); 
            return false;
        },
    };
    
    // Build and run the daemon remotely (using the cargo on the remote system)
    // This rather complicated command is the best I've found to run it in the background without needing to keep the 
    // ssh connection open.
    let remote_command = format!("cd {} && cargo build && (nohup cargo run -- --daemon > out.log 2> err.log </dev/null &)", remote_temp_folder.display());
    info!("Running remote command: {}", remote_command);
    match Command::new("ssh")
            .arg(user_prefix + remote_hostname)
            .arg(remote_command)
            .status() {
        Err(e) => {
            error!("Error launching ssh: {}", e); 
            return false;
        },
        Ok(s) if s.code() == Some(0) => {
            // good!
        }
        Ok(s) => {
            error!("Error building or launching on remote. Exit status from ssh: {}", s); 
            return false;
        },
    };
     //TODO: detach from the now running process! Maybe use cargo build and run separately? So we can wait for the build but then run it detached?
     // (don't want to keep our local ssh process running the whole time though! It needs to be detached on the other end!)
           
    return true;
}