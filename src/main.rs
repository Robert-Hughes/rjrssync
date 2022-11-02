use std::{net::{TcpListener, TcpStream}, io::{Write, Read, stdout, stdin, stderr, BufReader, BufRead}, path::PathBuf, process::{Stdio, ExitCode, ExitStatus, ChildStdout, ChildStdin, ChildStderr}, thread::sleep, time::Duration, sync::mpsc::RecvError};
use std::sync::mpsc;
use std::sync::mpsc::{Sender, Receiver};
use clap::Parser;
use log::{info, error, warn};
use rust_embed::RustEmbed;
use tempdir::TempDir;
use std::process::{Command};

const VERSION: i32 = 1;
const MAGIC: [u8; 4] = [19, 243, 129, 88];

#[derive(clap::Parser)]
struct PrimaryCliArgs {
    src: String,
    dest: String,
}
#[derive(clap::Parser)]
struct SecondaryCliArgs {
    #[arg(short, long)]
    secondary: bool,
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
            r.folder = after_user.to_string();
        },
        Some((a, b)) => {
            r.hostname = a.to_string();
            r.folder = b.to_string();
        }
    };

    return r;
}

fn main() -> ExitCode {
    // Configure logging
    simple_logger::SimpleLogger::new().env().init().unwrap();

    // The process can run as either a CLI which takes input from the command line, performs
    // a transfer and then exits once complete ("primary"), or as a remote process on either the source
    // or destination computer which responds to commands from the primary (this is a "secondary").
    // The primary (CLI) and secondary modes have different command-line arguments, so handle them separately.
    if std::env::args().any(|a| a == "--secondary") {
        return secondary_main();
    } else {
        return primary_main();
    }
}

fn primary_main() -> ExitCode {
    info!("Running as primary");

    let args = PrimaryCliArgs::parse();

    // The src and/or dest may be on another computer. We need to run a copy of rjrssync on the remote 
    // computer(s) and set up network commmunication. For consistency, we assume both are remote, even if they
    // are actually local.
    // There are therefore up to three copies of our program involved (although some may actually be the same as each other)
    //   Primary - this copy, which received the command line from the user
    //   Source - runs on the computer specified by the `src` command-line arg, and so if this is the local computer
    //            then this may be the same copy as the Primary.
    //   Dest - the computer specified by the `dest` command-line arg, and so if this is the local computer
    //          then this may be the same copy as the Primary.
    //          If Source and Dest are the same computer, they are still separate copies for simplicity.

    // Get list of hosts to launch and estabilish communication with
    let src_folder_desc = parse_remote_folder(&args.src);
    let dest_folder_desc = parse_remote_folder(&args.dest);

    // Launch rjrssync (if not already running) on both remote hosts and estabilish communication (check version etc.)
    let src_comms = setup_comms(&src_folder_desc.hostname, &src_folder_desc.user, true);
    if src_comms.is_none() {
        return ExitCode::from(10);
    }
    let _src_comms = src_comms.unwrap();

    let dest_comms = setup_comms(&dest_folder_desc.hostname, &dest_folder_desc.user, true);
    if dest_comms.is_none() {
        return ExitCode::from(11);
    }
    let _dest_comms = dest_comms.unwrap();
    
    error!("Communicate with Source and Dest to coordinate transfer - Not implemented!");
    return ExitCode::from(12);
}

const SECONDARY_BOOT_MSG : &str = "hello I have booted properly m8";

fn secondary_main() -> ExitCode {
    info!("Running as secondary");

    let _args = SecondaryCliArgs::parse();

    // We take commands from our stdin and send responses on our stdout. These will be piped over ssh
    // back to the Primary.

    // The first thing we send is a special message that the Primary will recognise, to know that we've started up correctly.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    println!("{}", SECONDARY_BOOT_MSG);
    eprintln!("{}", SECONDARY_BOOT_MSG);

    // Before starting the command-processing loop, we perform a basic handshake with the Primary
    // to make sure we are running compatible versions etc.
    //TODO: we probably don't need this anymore, as the boot message above fulfils this purpose (if we include some kind of magic and a version number in there)
    
    // Send our magic and version number, so the Primary can check if it's compatible.
    // Note that the 'protocol' used to check version number etc. needs to always be backwards-compatible,
    // so is very basic.
    // info!("Sending version number {}", VERSION);
    // let mut magic_and_version = MAGIC.to_vec();
    // magic_and_version.append(&mut VERSION.to_le_bytes().to_vec());
    // if stdout().write(&magic_and_version).is_err() {
    //     error!("Error writing magic and version");
    //     return ExitCode::from(20);
    // }

    // If the Primary isn't happy, they will stop us and deploy a new version. So at this point we can assume
    // they are happy and move on to processing commands they (might) send us

    // Message-processing loop, until Primary disconnects.
    let mut buf: [u8; 1] = [0];
    while stdin().read(&mut buf).unwrap_or(0) > 0 {
      //  info!("Received data from Primary: {}", buf[0]);

        // echo back
        stdout().write(&buf);

        //TODO: if get a message telling us to start a transfer, setup_comms(false) with the dest.
        //        (false cos the Dest should already have been set up with new version if necessary, so don't do it again)
        //TODO:     get a list of all the files in the local dir, and ask the dest to do the same
        //TODO:     compare the lists
        //TODO:     send over any files that have changed

        //TODO: if get a message telling us to provide a file list, do so

        //TODO: if get a message telling us to receive a file, do so
    }

   // info!("Primary disconnected!");
    return ExitCode::from(22);
}

fn setup_comms(remote_hostname: &str, remote_user: &str, allow_restart_remote_daemon_if_necessary: bool) -> Option::<()> {
    info!("setup_comms with '{}'", remote_hostname);

    //TODO: if remote is empty (i.e. local), then start a thread to handle that instead?
    // separate thread to avoid synchornisation with the Primary (and both Source and Dest may be on same PC!, so all three in one process)
    // need to find an appropriate level of abstraction to switch between the local and remote 'modes'.

    // We first attempt to run a previously-deployed copy of the program on the remote, to save time.
    // If it exists and is a compatible version, we can use that.
    loop {
        match launch_secondary_via_ssh(remote_hostname, remote_user) {
            SshSecondaryLaunchResult::FailedToRunSsh => {

            },
            SshSecondaryLaunchResult::NotPresentOnRemote | SshSecondaryLaunchResult::HandshakeIncompatibleVersion => {
                if deploy_to_remote(remote_hostname, remote_user) {
                    info!("Successfully deployed, attempting to run again");
                    continue;                    
                } else {
                    error!("Failed to deploy to remote");
                    return None;
                }
            },
            SshSecondaryLaunchResult::ExitedBeforeHandshake => {
                error!("Not gonna try doing anything, as we don't know what happened");
                return None;
            }
            SshSecondaryLaunchResult::Success { mut stdin, mut stdout, stderr } => {
                info!("Connection estabilished");

                // Test echoing
                stdin.write(&[1, 2, 5, 10]);

                let mut buf: [u8; 1] = [0];
                while stdout.read(&mut buf).unwrap_or(0) > 0 {
                    info!("Received data from Secondary echoed: {}", buf[0]);
                }

                return Some(());
            },
        };
    }

    // if let Ok(mut stream) = TcpStream::connect(&remote_addr) {
    //     info!("Connected to '{}'", &remote_addr);
 
    //     // Wait for the server to send their magic and version, so we can check if it's compatible with ours
    //     // Note that the 'protocol' used to check version number etc. needs to always be backwards-compatible,
    //     // so is very basic.
    //     let server_version;
    //     info!("Waiting for version number");
    //     let mut buf: [u8; 8] = [0; 8]; // 4 bytes for magic, 4 bytes for version
    //     if stream.read(&mut buf).unwrap_or(0) == 8 {
    //         let server_magic = &buf[0..4];
    //         if server_magic != MAGIC {
    //             error!("Server replied with wrong magic. Not attempting to restart it, as there may be an unknown process (not us) listening on that port and we don't want to interfere with it.");
    //             return None;
    //         }

    //         let mut b : [u8; 4] = [0; 4];
    //         b.copy_from_slice(&buf[4..8]);
    //         server_version = i32::from_le_bytes(b);
    //         info!("Received server version {}", server_version);
    //     } else {
    //         error!("Server is not replying as expected. Not attempting to restart it, as there may be an unknown process (not us) listening on that port and we don't want to interfere with it.");
    //         return None;
    //     }

    //     if server_version == VERSION {
    //         info!("Server has compatible version. Replying as such.");
    //         // Send packet to tell server all is OK
    //         if stream.write(&[0 as u8]).is_ok() {
    //             info!("Connection estabilished!");
    //             return Some(stream);
    //         } else {
    //             warn!("Failed to reply to server");
    //         }
    //     } else {
    //         if allow_restart_remote_daemon_if_necessary {
    //             warn!("Server has incompatible version - telling it to stop so we can restart it");
    //             // Send packet to tell server to restart
    //             let _ = stream.write(&[1 as u8]); // Don't need to check result here - even if it failed, we will still try to launch new server
    //         } else {
    //             error!("Server has incompatible version, and we're not going to restart it.");
    //         }
    //     }
    // } else {
    //     info!("No remote daemon running");
    // }

    // // No instance running - spawn a new one
    // if allow_restart_remote_daemon_if_necessary {   
    //     if spawn_daemon_on_remote(remote_hostname, remote_user) {
    //         // Try again to connect to the new daemon. 
    //         // Don't allow this recursion to spawn a new daemon again though in case we still can't connect,
    //         // otherwise it would keep trying forever!
    //         let result = setup_comms(remote_hostname, remote_user, false);
    //         if result.is_none() {
    //             error!("Failed to setup_comms even after spawning a new daemon");
    //         }
    //         return result;
    //     } else {
    //         error!("Failed to spawn a new daemon. Please launch it manually.");
    //         return None;
    //     }
    // } else {
    //     return None;
    // }
}

enum SshSecondaryLaunchResult {
    FailedToRunSsh,
    NotPresentOnRemote,
    ExitedBeforeHandshake,
    HandshakeIncompatibleVersion,
    Success {
        stdin: ChildStdin,
        stdout: BufReader<Box<dyn Read + Send>>,
        stderr: BufReader<Box<dyn Read + Send>>,
    }
}

// Generic thread function for reading from stdout or stderr of the ssh process.
// We need to handle both in the same way - waiting until we receive the magic line indicating that the secondary
// copy has started up correctly. And so that each background thread knows when it's time to return control 
// of the stream.
enum OutputReaderThreadMsg {
    Line(String),
    Error(std::io::Error),
    StreamClosed,
    HandshakePassed(BufReader<Box<dyn Read + Send>>),
}

fn output_reader_thread_main<T>(stream: T, stream_name: &'static str, sender: Sender<(&str, OutputReaderThreadMsg)>) where T : Read + Send + 'static {
    let mut reader: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(stream));
    loop  {
        let mut l : String = "".to_string();
        match reader.read_line(&mut l) {
            Err(e) => {
                sender.send((stream_name, OutputReaderThreadMsg::Error(e)));
                return;
            },
            Ok(0) => {
                // end of stream
                sender.send((stream_name, OutputReaderThreadMsg::StreamClosed));
                return;
            }
            Ok(_) if l.starts_with(SECONDARY_BOOT_MSG) => {
                //TODO: check version. Here or on the main thread?

                // remote end has booted up properly and is ready for comms.
                // finish this thread and return control of the stdout to the main thread.
                sender.send((stream_name, OutputReaderThreadMsg::HandshakePassed(reader)));
                return;
            } 
            Ok(_) => {            
                sender.send((stream_name, OutputReaderThreadMsg::Line(l)));
            }
        }
    }
}

fn launch_secondary_via_ssh(remote_hostname: &str, remote_user: &str) -> SshSecondaryLaunchResult {
    info!("launch_secondary_via_ssh on '{}'", remote_hostname);

    let user_prefix = if remote_user.is_empty() { "".to_string() } else { remote_user.to_string() + "@" };
    let remote_temp_folder = PathBuf::from("/tmp/rjrssync/");
    let remote_command = format!("cd {} && target/release/rjrssync --secondary", remote_temp_folder.display());
    info!("Running remote command: {}", remote_command);
    //TODO: should we run "cmd /C ssh ..." rather than just "ssh", otherwise the line endings get messed up and subsequent log messages are broken?
    let mut ssh = match Command::new("ssh")
        .arg(user_prefix + remote_hostname)
        .arg(remote_command)
        .stdin(Stdio::piped()) //TODO: even though we're piping this, it still seems able to accept password input somehow?? Using /dev/tty?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()) 
        .spawn() {
        Ok(c) => c,
        Err(e) => {
            error!("Error launching ssh: {}", e); 
            return SshSecondaryLaunchResult::FailedToRunSsh;
        },
    };

    let mut ssh_stdin = ssh.stdin.take().unwrap(); // stdin should always be available as we piped it
    let mut ssh_stdout = ssh.stdout.take().unwrap(); // stdin should always be available as we piped it
    let mut ssh_stderr = ssh.stderr.take().unwrap(); // stdin should always be available as we piped it

    let (sender, receiver): (Sender<(&str, OutputReaderThreadMsg)>, Receiver<(&str, OutputReaderThreadMsg)>) = mpsc::channel();
    let sender2 = sender.clone();
    // std::thread::spawn(move || {
    //     std::io::copy(&mut stdin(), &mut ssh_stdin).expect("stdin error");
    //  });

    std::thread::spawn(move || output_reader_thread_main(ssh_stdout, "stdout", sender));
    std::thread::spawn(move || output_reader_thread_main(ssh_stderr, "stderr", sender2));

    info!("Waiting for remote copy to send version handshake");
    //TODO: wait until ssh either exits (e.g. due to an error), or we see a special print from our secondary program
    // that says it's up and running. Perhaps this replaces the "Magic"? as we won't need that anymore? We could include
    // the version number in the special print too, so we can replace that too?
    // then we can transition into command mode, where we talk in binary?
    //TOOD: the ssh process could not exit but also never print anything useful, e.g. if it hangs or is waiting for input that never comes.
    // In this case the user would need to kill it, so need Ctrl+C to work.
    //TODO: can/should/how can we forward Ctrl+C to ssh?
    //TODO: can we make it so that the remote copy is killed once teh ssh session is dropped, e.g. if the primary copy is killed.
    // do we need to close all the streams that we have open too??
    let mut not_present_on_remote = false;
    let mut handshook_stdout : Option<BufReader<Box<dyn Read + Send>>> = None;
    let mut handshook_stderr : Option<BufReader<Box<dyn Read + Send>>> = None;
    loop {
        match receiver.recv() {
            Err(RecvError) => {
                info!("Receiver error - all threads done, process exited?");
                // Wait for the process to exit, for tidyness?
                let result = ssh.wait();
                info!("Process exited with {:?}", result);
                if not_present_on_remote {
                    return SshSecondaryLaunchResult::NotPresentOnRemote;
                } else {
                    return SshSecondaryLaunchResult::ExitedBeforeHandshake;
                }
            }
            Ok((stream_name, OutputReaderThreadMsg::Line(l))) => {
                info!("Line received from {}: {}", stream_name, l);               
                if l.contains("No such file or directory") {
                    not_present_on_remote = true;
                }
            },
            Ok((stream_name, OutputReaderThreadMsg::HandshakePassed(s))) => {
                info!("HandshakePassed from {}", stream_name);
                // Need to wait for both stdout and stderr to pass the handshake
                if (stream_name == "stdout") {
                    handshook_stdout = Some(s);
                } else if (stream_name == "stderr") {
                    handshook_stderr = Some(s);
                } else {
                    panic!("Unknown stream"); //TODO: use an enum instead??
                }
                //TODO: check version. Here or on the background thread? Check both stdout and stderr?
                if (handshook_stdout.is_some() && handshook_stderr.is_some()) {
                    return SshSecondaryLaunchResult::Success { stdin: ssh_stdin, stdout: handshook_stdout.unwrap(), stderr: handshook_stderr.unwrap() };
                }

            },
            Ok((stream_name, OutputReaderThreadMsg::Error(e))) => {
                info!("Error from {}", stream_name);
                // Wait for the process to exit, for tidyness?
                ssh.wait();
            },
            Ok((stream_name, OutputReaderThreadMsg::StreamClosed)) => {
                info!("StreamClosed {}", stream_name);
                // Wait for the process to exit, for tidyness?
                ssh.wait();       
            }
        }
    }

    //TODO: wait for threads to exit, for completeness?
   
    //TODO: wait for SSH password prompt etc. to finish, then do version handshake
}

// This embeds the source code of the program into the executable, so it can be deployed remotely and built on other platforms
#[derive(RustEmbed)]
#[folder = "."]
#[include = "src/*"]
#[include = "Cargo.*"]
struct EmbeddedSource;

fn deploy_to_remote(remote_hostname: &str, remote_user: &str) -> bool {
    info!("Deploying onto '{}'", &remote_hostname);

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
    // Note that we run "cmd /C scp ..." rather than just "scp", otherwise the line endings get messed up and subsequent log messages are broken.
    match Command::new("cmd")
        .arg("/C")
        .arg("scp")
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
    
    // Build the program remotely (using the cargo on the remote system)
    // Note that we could merge this ssh command with the one to run the program once it's built (in launch_secondary_via_ssh),
    // but this would make error reporting slightly more difficult as the command in launch_secondary_via_ssh is more tricky as 
    // we are parsing the stdout, but the command here we can wait for it to finish easily.
    //TODO: add flag to hide warnings when building remotely?
    let remote_command = format!("cd {} && cargo build --release", remote_temp_folder.display());
    info!("Running remote command: {}", remote_command);
     // Note that we run "cmd /C ssh ..." rather than just "ssh", otherwise the line endings get messed up and subsequent log messages are broken.
     match Command::new("cmd")
        .arg("/C")
        .arg("ssh")
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
            error!("Error building on remote. Exit status from ssh: {}", s); 
            return false;
        },
    };

    return true;
}


// use clap::Parser;
// use walkdir::WalkDir;
// use std::time::Instant;

// #[derive(Parser)]
// struct Cli {
//     path: std::path::PathBuf,
// }

// fn main() {
//     let args = Cli::parse();
//     {
//         let start = Instant::now();
//         let walker = WalkDir::new(&args.path).into_iter();
//         let mut count = 0;
//         for _entry in walker.filter_entry(|e| e.file_name() != ".git" && e.file_name() != "dependencies") {
//             count += 1;
//         }
//         let elapsed = start.elapsed().as_millis();
//         println!("Walked {} in {} ({}/s)", count, elapsed, 1000.0 * count as f32 / elapsed as f32);
//     }

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

// }

//  Host:           Windows     Linux
//  Filesystem:
//    Windows        100k        9k
//     Linux          1k         500k