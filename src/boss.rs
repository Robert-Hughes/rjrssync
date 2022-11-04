use std::{io::{Write, BufReader, BufRead, Read}, path::PathBuf, process::{Stdio, ExitCode, ChildStdout, ChildStdin, ChildStderr}, sync::mpsc::RecvError, fmt::{Display, self}, thread::JoinHandle};
use std::sync::mpsc;
use std::sync::mpsc::{Sender, Receiver};
use clap::{Parser};
use env_logger::{fmt::Color, Env};
use log::{info, error, warn};
use rust_embed::RustEmbed;
use tempdir::TempDir;

use crate::*;
use crate::boss_sync::*;

#[derive(clap::Parser)]
struct BossCliArgs {
    src: String,
    dest: String,
    #[arg(short, long)]
    doer: bool,
    //TODO: arg to force re-deploy the remote copy, e.g. useful if the handshake changes
}

#[derive(Default)]
pub struct RemoteFolderDesc {
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
    //TODO: colon clashes with windows drive letters!
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

pub fn boss_main() -> ExitCode {
    // Configure logging
    let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or("info"));

    builder.format(|buf, record| {
        let mut level_style = buf.style();

        let color = match record.module_path(){
            Some("rjrssync::boss") => Color::Rgb(255, 64, 255),
            Some("rjrssync::doer") => Color::Cyan,
            _ => Color::Green
        };

        level_style.set_color(color);

        writeln!(buf, "{:5} | {}: {}",
            record.level(),
            level_style.value(record.module_path().unwrap()),
            record.args())
    });
    builder.init();

    info!("Running as boss");

    let args = BossCliArgs::parse();

    // The src and/or dest may be on another computer. We need to run a copy of rjrssync on the remote
    // computer(s) and set up network commmunication.
    // There are therefore up to three copies of our program involved (although some may actually be the same as each other)
    //   Boss - this copy, which received the command line from the user
    //   Source - runs on the computer specified by the `src` command-line arg, and so if this is the local computer
    //            then this may be the same copy as the Boss.
    //   Dest - the computer specified by the `dest` command-line arg, and so if this is the local computer
    //          then this may be the same copy as the Boss.
    //          If Source and Dest are the same computer, they are still separate copies for simplicity.
    //          (It might be more efficient to just have one remote copy, but remember that there could be different users specified
    //           on the Source and Dest, with separate permissions to the folders being synced, so they can't access each others' folders,
    //           in which case we couldn't share a copy. Also might need to make it multithreaded on the other end to handle
    //           doing one command at the same time for each Source and Dest, which might be more complicated.)

    // Get list of hosts to launch and estabilish communication with
    let src_folder_desc = parse_remote_folder(&args.src);
    let dest_folder_desc = parse_remote_folder(&args.dest);

    // Launch rjrssync (if not already running) on both remote hosts and estabilish communication (check version etc.)
    let src_comms = setup_comms(&src_folder_desc.hostname, &src_folder_desc.user, true);
    if src_comms.is_none() {
        return ExitCode::from(10);
    }
    let src_comms = src_comms.unwrap();

    let dest_comms = setup_comms(&dest_folder_desc.hostname, &dest_folder_desc.user, true);
    if dest_comms.is_none() {
        return ExitCode::from(11);
    }
    let dest_comms = dest_comms.unwrap();

    let sync_result = sync(src_folder_desc.folder, dest_folder_desc.folder, src_comms, dest_comms);

    return match sync_result{
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(12),
    };
}

//TODO: can we share implementation between the two Comms classes (boss and doer?)
pub enum Comms {
    Local {
        _thread: JoinHandle<()>,
        sender: Sender<Command>,
        receiver: Receiver<Response>,
    },
    Remote {
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
        _stderr: BufReader<ChildStderr>, //TODO: should we be reading from this??
    }
}
impl Comms {
    pub fn send_command(&self, c: Command) -> Result<(), String> {
        info!("Sending command {:?} to {}", c, &self);
        let res;
        match self {
            Comms::Local { _thread: _, sender, receiver: _ } => {
                res = sender.send(c).map_err(|e| e.to_string());
            },
            Comms::Remote { stdin, stdout: _, _stderr: _ } => {
                res = bincode::serialize_into(stdin, &c).map_err(|e| e.to_string());
                std::io::stdout().flush().unwrap(); // Otherwise could be buffered and we hang!
            }
        }
        if res.is_err() {
            error!("Error sending command: {:?}", res);
        }
        return res;
    }

    pub fn receive_response(&mut self) -> Result<Response, String> {
        info!("Waiting for response from {}", &self);
        let r;
        match self {
            Comms::Local { _thread: _, sender: _, receiver } => {
                r = receiver.recv().map_err(|e| e.to_string());
            },
            Comms::Remote { stdin: _, stdout, _stderr: _ } => {
                r = bincode::deserialize_from(stdout.by_ref()).map_err(|e| e.to_string());
            },
        }
        info!("Received response {:?} from {}", r, &self);
        return r;
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


fn setup_comms(remote_hostname: &str, remote_user: &str, _allow_restart_remote_daemon_if_necessary: bool) -> Option<Comms> {
    info!("setup_comms with '{}'", remote_hostname);

    // If remote is empty (i.e. local), then start a thread to handle commands.
    // Use a separate thread to avoid synchornisation with the Boss (and both Source and Dest may be on same PC!, so all three in one process),
    // and for consistency with remote secondaries.
    //TODO: Use channels to send/receive messages to that thread?
    if remote_hostname.is_empty() {
        info!("Spawning local thread");
        let (command_sender, command_receiver) = mpsc::channel();
        let (response_sender, response_receiver) = mpsc::channel();
        let thread = std::thread::spawn(
            move || { doer_thread_running_on_boss(command_receiver, response_sender); });
        return Some(Comms::Local { _thread: thread, sender: command_sender, receiver: response_receiver });
    }

    // We first attempt to run a previously-deployed copy of the program on the remote, to save time.
    // If it exists and is a compatible version, we can use that.
    loop {
        match launch_doer_via_ssh(remote_hostname, remote_user) {
            SshDoerLaunchResult::FailedToRunSsh => {

            },
            SshDoerLaunchResult::NotPresentOnRemote | SshDoerLaunchResult::HandshakeIncompatibleVersion => {
                if deploy_to_remote(remote_hostname, remote_user).is_ok() {
                    info!("Successfully deployed, attempting to run again");
                    continue;
                } else {
                    error!("Failed to deploy to remote");
                    return None;
                }
            },
            SshDoerLaunchResult::ExitedBeforeHandshake => {
                error!("Not gonna try doing anything, as we don't know what happened");
                return None;
            }
            SshDoerLaunchResult::Success { stdin, stdout, stderr } => {
                info!("Connection estabilished");
                return Some(Comms::Remote { stdin, stdout, _stderr: stderr });
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

enum SshDoerLaunchResult {
    FailedToRunSsh,
    NotPresentOnRemote,
    ExitedBeforeHandshake,
    HandshakeIncompatibleVersion,
    Success {
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
        stderr: BufReader<ChildStderr>,
    }
}

// Generic thread function for reading from stdout or stderr of the ssh process.
// We need to handle both in the same way - waiting until we receive the magic line indicating that the doer
// copy has started up correctly. And so that each background thread knows when it's time to return control
// of the stream.
enum OutputReaderThreadMsg {
    Line(String),
    Error(std::io::Error),
    StreamClosed,
    HandshakeReceived(String, OutputReaderStream), // Also sends back the stream, so the main thread takes control.
}

#[derive(Clone, Copy)]
enum OutputReaderStreamType {
    Stdout,
    Stderr
}
impl Display for OutputReaderStreamType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OutputReaderStreamType::Stdout => write!(f, "stdout"),
            OutputReaderStreamType::Stderr => write!(f, "stderr"),
        }
    }
}

enum OutputReaderStream {
    Stdout(BufReader<ChildStdout>),
    Stderr(BufReader<ChildStderr>)
}
impl OutputReaderStream {
    fn get_type(&self) -> OutputReaderStreamType {
        match self {
            OutputReaderStream::Stdout(_) => OutputReaderStreamType::Stdout,
            OutputReaderStream::Stderr(_) => OutputReaderStreamType::Stderr,
        }
    }
    fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
        match self {
            OutputReaderStream::Stdout(b) => b.read_line(buf),
            OutputReaderStream::Stderr(b) => b.read_line(buf),
        }
    }
}

fn output_reader_thread_main(mut stream: OutputReaderStream, sender: Sender<(OutputReaderStreamType, OutputReaderThreadMsg)>) {
    let stream_type = stream.get_type();
    loop  {
        let mut l : String = "".to_string();
        // Note we unwrap() the errors on the sender here, as the other end should never have been dropped before this thread exits.
        match stream.read_line(&mut l) {
            Err(e) => {
                sender.send((stream_type, OutputReaderThreadMsg::Error(e))).unwrap();
                return;
            },
            Ok(0) => {
                // end of stream
                sender.send((stream_type, OutputReaderThreadMsg::StreamClosed)).unwrap();
                return;
            }
            Ok(_) => {
                l.pop(); // Remove the trailing newline
                if l.starts_with(HANDSHAKE_MSG) {
                    // remote end has booted up properly and is ready for comms.
                    // finish this thread and return control of the stdout to the main thread, so it can communicate directly
                    sender.send((stream_type, OutputReaderThreadMsg::HandshakeReceived(l, stream))).unwrap();
                    return;
                } else {
                    sender.send((stream_type, OutputReaderThreadMsg::Line(l))).unwrap();
                }
            }
        }
    }
}

fn launch_doer_via_ssh(remote_hostname: &str, remote_user: &str) -> SshDoerLaunchResult {
    info!("launch_doer_via_ssh on '{}'", remote_hostname);

    let user_prefix = if remote_user.is_empty() { "".to_string() } else { remote_user.to_string() + "@" };
    let remote_temp_folder = PathBuf::from("/tmp/rjrssync/");
    let remote_command = format!("cd {} && target/release/rjrssync --doer", remote_temp_folder.display());
    info!("Running remote command: {}", remote_command);
    //TODO: should we run "cmd /C ssh ..." rather than just "ssh", otherwise the line endings get messed up and subsequent log messages are broken?
    let mut ssh = match std::process::Command::new("ssh")
        .arg(user_prefix + remote_hostname)
        .arg(remote_command)
        .stdin(Stdio::piped()) //TODO: even though we're piping this, it still seems able to accept password input somehow?? Using /dev/tty?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn() {
        Ok(c) => c,
        Err(e) => {
            error!("Error launching ssh: {}", e);
            return SshDoerLaunchResult::FailedToRunSsh;
        },
    };

    // Note that some of the output from ssh (errors etc.) comes on stderr, so we need to display both stdout and stderr
    // to the user, before handshake is estabilished.

    let ssh_stdin = ssh.stdin.take().unwrap(); // stdin should always be available as we piped it
    let ssh_stdout = ssh.stdout.take().unwrap(); // stdin should always be available as we piped it
    let ssh_stderr = ssh.stderr.take().unwrap(); // stdin should always be available as we piped it

    let (sender1, receiver): (Sender<(OutputReaderStreamType, OutputReaderThreadMsg)>, Receiver<(OutputReaderStreamType, OutputReaderThreadMsg)>) = mpsc::channel();
    let sender2 = sender1.clone();
    // std::thread::spawn(move || {
    //     std::io::copy(&mut stdin(), &mut ssh_stdin).expect("stdin error");
    //  });

    std::thread::spawn(move || output_reader_thread_main(OutputReaderStream::Stdout(BufReader::new(ssh_stdout)), sender1));
    std::thread::spawn(move || output_reader_thread_main(OutputReaderStream::Stderr(BufReader::new(ssh_stderr)), sender2));

    info!("Waiting for remote copy to send version handshake");
    //TODO: wait until ssh either exits (e.g. due to an error), or we see a special print from our doer program
    // that says it's up and running. Perhaps this replaces the "Magic"? as we won't need that anymore? We could include
    // the version number in the special print too, so we can replace that too?
    // then we can transition into command mode, where we talk in binary?
    //TOOD: the ssh process could not exit but also never print anything useful, e.g. if it hangs or is waiting for input that never comes.
    // In this case the user would need to kill it, so need Ctrl+C to work.
    //TODO: can/should/how can we forward Ctrl+C to ssh?
    //TODO: can we make it so that the remote copy is killed once teh ssh session is dropped, e.g. if the boss copy is killed.
    // do we need to close all the streams that we have open too??
    let mut not_present_on_remote = false;
    let mut handshook_stdout : Option<BufReader<ChildStdout>> = None;
    let mut handshook_stderr : Option<BufReader<ChildStderr>> = None;
    loop {
        match receiver.recv() {
            Err(RecvError) => {
                info!("Receiver error - all threads done, process exited?");
                // Wait for the process to exit, for tidyness?
                let result = ssh.wait();
                info!("Process exited with {:?}", result);
                if not_present_on_remote {
                    return SshDoerLaunchResult::NotPresentOnRemote;
                } else {
                    return SshDoerLaunchResult::ExitedBeforeHandshake;
                }
            }
            Ok((stream_type, OutputReaderThreadMsg::Line(l))) => {
                info!("Line received from {}: {}", stream_type, l);
                if l.contains("No such file or directory") {
                    not_present_on_remote = true;
                }
            },
            Ok((stream_type, OutputReaderThreadMsg::HandshakeReceived(line, s))) => {
                info!("HandshakeReceived from {}: {}", stream_type, line);
                // Need to wait for both stdout and stderr to pass the handshake
                match s {
                    OutputReaderStream::Stdout(b) => {
                        handshook_stdout = Some(b);
                    },
                    OutputReaderStream::Stderr(b) => {
                        handshook_stderr = Some(b);
                    },
                }

                let remote_version = line.split_at(HANDSHAKE_MSG.len()).1;
                if remote_version != VERSION.to_string() {
                    warn!("Remote server has incompatible version ({} vs local version {})", remote_version, VERSION);
                    return SshDoerLaunchResult::HandshakeIncompatibleVersion;
                }

                //TODO: check version. Check both stdout and stderr?
                if handshook_stdout.is_some() && handshook_stderr.is_some() {
                    return SshDoerLaunchResult::Success { stdin: ssh_stdin, stdout: handshook_stdout.unwrap(), stderr: handshook_stderr.unwrap() };
                }

            },
            Ok((stream_type, OutputReaderThreadMsg::Error(e))) => {
                info!("Error from {}: {}", stream_type, e);
                // Wait for the process to exit, for tidyness?
               // ssh.wait();
            },
            Ok((stream_type, OutputReaderThreadMsg::StreamClosed)) => {
                info!("StreamClosed {}", stream_type);
                // Wait for the process to exit, for tidyness?
               // ssh.wait();
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

fn deploy_to_remote(remote_hostname: &str, remote_user: &str) -> Result<(), ()> {
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
            return Err(());
        }
    };
    info!("Writing embedded source to temp dir: {}", temp_dir.path().display());
    for file in EmbeddedSource::iter() {
        let temp_path = temp_dir.path().join("rjrssync").join(&*file); // Add an extra "rjrssync" folder with a fixed name (as opposed to the temp dir, whose name varies), to work around SCP weirdness below.

        if let Err(e) = std::fs::create_dir_all(temp_path.parent().unwrap()) {
            error!("Error creating folders for temp file: {}", e);
            return Err(());
        }

        let mut f = match std::fs::File::create(&temp_path) {
            Ok(x) => x,
            Err(e) => {
                error!("Error creating temp file {}: {}", temp_path.display(), e);
                return Err(());
            }
        };

        if let Err(e) = f.write_all(&EmbeddedSource::get(&file).unwrap().data) {
            error!("Error writing temp file: {}", e);
            return Err(());
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
    match std::process::Command::new("cmd")
        .arg("/C")
        .arg("scp")
        .arg("-r")
        .arg(source_spec)
        .arg(remote_spec)
        .status() {
        Err(e) => {
            error!("Error launching scp: {}", e);
            return Err(());
        },
        Ok(s) if s.code() == Some(0) => {
            // good!
        }
        Ok(s) => {
            error!("Error copying source code. Exit status from scp: {}", s);
            return Err(());
        },
    };

    // Build the program remotely (using the cargo on the remote system)
    // Note that we could merge this ssh command with the one to run the program once it's built (in launch_doer_via_ssh),
    // but this would make error reporting slightly more difficult as the command in launch_doer_via_ssh is more tricky as
    // we are parsing the stdout, but the command here we can wait for it to finish easily.
    let remote_command = format!("cd {} && cargo build --release", remote_temp_folder.display());
    info!("Running remote command: {}", remote_command);
     // Note that we run "cmd /C ssh ..." rather than just "ssh", otherwise the line endings get messed up and subsequent log messages are broken.
     match std::process::Command::new("cmd")
        .arg("/C")
        .arg("ssh")
        .arg(user_prefix + remote_hostname)
        .arg(remote_command)
        .status() {
        Err(e) => {
            error!("Error launching ssh: {}", e);
            return Err(());
        },
        Ok(s) if s.code() == Some(0) => {
            // good!
        }
        Ok(s) => {
            error!("Error building on remote. Exit status from ssh: {}", s);
            return Err(());
        },
    };

    return Ok(());
}
