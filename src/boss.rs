use clap::Parser;
use env_logger::{fmt::Color, Env};
use log::{debug, error, info, warn, log};
use rust_embed::RustEmbed;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::{
    fmt::{self, Display},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::PathBuf,
    process::{ChildStderr, ChildStdin, ChildStdout, ExitCode, Stdio},
    sync::mpsc::{RecvError, SendError},
    thread::JoinHandle,
};
use tempdir::TempDir;

use crate::boss_sync::*;
use crate::*;

#[derive(clap::Parser)]
struct BossCliArgs {
    /// The source folder, which will be synced to the destination folder.
    /// Optionally contains a username and hostname for specifying remote folders.
    /// Format: [[username@]hostname:]folder
    src: RemoteFolderDesc,
    /// The destination folder, which will be synced from the source folder.
    /// Optionally contains a username and hostname for specifying remote folders.
    /// Format: [[username@]hostname:]folder
    dest: RemoteFolderDesc,
    /// If set, forces redeployment of rjrssync to any remote targets, even if they already have an
    /// up-to-date copy.
    #[arg(long)]
    force_redeploy: bool,
    #[arg(name="exclude", long)]
    exclude_filters: Vec<String>,
    /// [Internal] Launches as a doer process, rather than a boss process.
    /// This shouldn't be needed for regular operation.
    #[arg(long)]
    doer: bool,
}

/// Describes a local or remote folder, parsed from the `src` or `dest` command-line arguments.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct RemoteFolderDesc {
    username: String,
    hostname: String,
    // Note this shouldn't be a PathBuf, because the syntax of this path will be for the remote system,
    // which might be different to the local system.
    folder: String,
}
impl std::str::FromStr for RemoteFolderDesc {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // There's some quirks here with windows paths containing colons for drive letters

        let mut r = RemoteFolderDesc::default();

        // The first colon splits folder from the rest, apart from special case for drive letters
        match s.split_once(':') {
            None => {
                r.folder = s.to_string();
            }
            Some((a, b)) if a.len() == 1 && (b.is_empty() || b.starts_with('\\')) => {
                r.folder = s.to_string();
            }
            Some((user_and_host, folder)) => {
                r.folder = folder.to_string();

                // The first @ splits the user and hostname
                match user_and_host.split_once('@') {
                    None => {
                        r.hostname = user_and_host.to_string();
                    }
                    Some((user, host)) => {
                        r.username = user.to_string();
                        if r.username.is_empty() {
                            return Err("Missing username".to_string());
                        }
                        r.hostname = host.to_string();
                    }
                };
                if r.hostname.is_empty() {
                    return Err("Missing hostname".to_string());
                }
            }
        };

        if r.folder.is_empty() {
            return Err("Folder must be specified".to_string());
        }

        Ok(r)
    }
}

pub fn boss_main() -> ExitCode {
    // Configure logging
    let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or("info"));
    builder.format(|buf, record| {
        let target_color = match record.target() {
            "rjrssync::boss" => Color::Rgb(255, 64, 255),
            "rjrssync::doer" => Color::Cyan,
            "remote doer" => Color::Yellow,
            _ => Color::Green,
        };
        let target_style = buf.style().set_color(target_color).clone();

        let level_color = match record.level() {
            log::Level::Error => Color::Red,
            log::Level::Warn => Color::Yellow,
            _ => Color::Black,
        };
        let level_style = buf.style().set_color(level_color).clone();

        writeln!(
            buf,
            "{:5} | {}: {}",
            level_style.value(record.level()),
            target_style.value(record.target()),
            record.args()
        )
    });
    builder.init();

    debug!("Running as boss");

    let args = BossCliArgs::parse();

    // The src and/or dest may be on another computer. We need to run a copy of rjrssync on the remote
    // computer(s) and set up network commmunication.
    // There are therefore up to three copies of our program involved (although some may actually be the same as each other)
    //   Boss - this copy, which received the command line from the user
    //   Source - runs on the computer specified by the `src` command-line arg, and so if this is the local computer
    //            then this may be the same copy as the Boss. If it's remote then it will be a remote doer process.
    //   Dest - the computer specified by the `dest` command-line arg, and so if this is the local computer
    //          then this may be the same copy as the Boss. If it's remote then it will be a remote doer process.
    //          If Source and Dest are the same computer, they are still separate copies for simplicity.
    //          (It might be more efficient to just have one remote copy, but remember that there could be different users specified
    //           on the Source and Dest, with separate permissions to the folders being synced, so they can't access each others' folders,
    //           in which case we couldn't share a copy. Also might need to make it multithreaded on the other end to handle
    //           doing one command at the same time for each Source and Dest, which might be more complicated.)

    // Launch doers on remote hosts or threads on local targets and estabilish communication (check version etc.)
    let src_comms = match setup_comms(
        &args.src.hostname,
        &args.src.username,
        "src".to_string(),
        args.force_redeploy,
    ) {
        Some(c) => c,
        None => return ExitCode::from(10),
    };
    let dest_comms = match setup_comms(
        &args.dest.hostname,
        &args.dest.username,
        "dest".to_string(),
        args.force_redeploy,
    ) {
        Some(c) => c,
        None => return ExitCode::from(11),
    };

    // Perform the actual file sync
    let sync_result = sync(args.src.folder, args.dest.folder, args.exclude_filters, src_comms, dest_comms);

    match sync_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(12),
    }
}

/// Abstraction of two-way communication channel between this boss and a doer, which might be
/// remote (communicating over ssh) or local (communicating via a channel to a background thread).
pub enum Comms {
    Local {
        debug_name: String, // To identify this Comms against others for debugging, when there are several
        thread: JoinHandle<()>,
        sender: Sender<Command>,
        receiver: Receiver<Response>,
    },
    Remote {
        debug_name: String, // To identify this Comms against others for debugging, when there are several
        ssh_process: std::process::Child,
        // Use bufferred readers/writers to reduce number of underlying system calls, for performance
        stdin: BufWriter<ChildStdin>,
        stdout: BufReader<ChildStdout>,
        stderr_reading_thread: JoinHandle<()>,
    },
}
impl Comms {
    pub fn send_command(&mut self, c: Command) -> Result<(), String> {
        debug!("Sending command {:?} to {}", c, &self);
        let mut res;
        match self {
            Comms::Local { sender, .. } => {
                res = sender.send(c).map_err(|e| e.to_string());
            }
            Comms::Remote { stdin, .. } => {
                res = bincode::serialize_into(stdin.by_ref(), &c).map_err(|e| e.to_string());
                if res.is_ok() {
                    res = stdin.flush().map_err(|e| e.to_string()); // Otherwise could be buffered and we hang!
                }
            }
        }
        if res.is_err() {
            error!("Error sending command: {:?}", res);
        }
        res
    }

    pub fn receive_response(&mut self) -> Result<Response, String> {
        debug!("Waiting for response from {}", &self);

        let r = match self {
            Comms::Local { receiver, .. } => receiver.recv().map_err(|e| e.to_string()),
            Comms::Remote { stdout, .. } => {
                bincode::deserialize_from(stdout.by_ref()).map_err(|e| e.to_string())
            }
        };
        debug!("Received response {:?} from {}", r, &self);
        r
    }
}
impl Display for Comms {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Comms::Local { debug_name, .. } => write!(f, "{}", debug_name),
            Comms::Remote { debug_name, .. } => write!(f, "{}", debug_name),
        }
    }
}
impl Drop for Comms {
    // Tell the other end (thread or process through ssh) to shutdown once we're finished.
    // They should exit anyway due to a disconnection (of their channel or stdin), but this
    // gives a cleaner exit without errors.
    fn drop(&mut self) {
        // There's not much we can do about an error here, other than log it, which send_command already does, so we ignore any error.
        let _ = self.send_command(Command::Shutdown);
    }
}

// Sets up communications with the given computer, which may be either remote or local (if remote_hostname is empty).
fn setup_comms(
    remote_hostname: &str,
    remote_user: &str,
    debug_name: String,
    force_redeploy: bool,
) -> Option<Comms> {
    debug!(
        "setup_comms with hostname '{}' and username '{}'. debug_name = {}",
        remote_hostname, remote_user, debug_name
    );

    // If the target is local, then start a thread to handle commands.
    // Use a separate thread to avoid synchornisation with the Boss (and both Source and Dest may be on same PC, so all three in one process),
    // and for consistency with remote secondaries.
    if remote_hostname.is_empty() {
        debug!("Spawning local thread for {} doer", debug_name);
        let (command_sender, command_receiver) = mpsc::channel();
        let (response_sender, response_receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            doer_thread_running_on_boss(command_receiver, response_sender)
        });
        return Some(Comms::Local {
            debug_name: "Local ".to_string() + &debug_name + " doer",
            thread,
            sender: command_sender,
            receiver: response_receiver,
        });
    }

    // We first attempt to run a previously-deployed copy of the program on the remote, to save time.
    // If it exists and is a compatible version, we can use that. Otherwise we deploy a new version
    // and try again
    let mut deploy = force_redeploy;
    for attempt in 0..2 {
        if deploy {
            if deploy_to_remote(remote_hostname, remote_user).is_ok() {
                debug!("Successfully deployed, attempting to run again");
            } else {
                error!("Failed to deploy to remote");
                return None;
            }
        }

        match launch_doer_via_ssh(remote_hostname, remote_user) {
            SshDoerLaunchResult::FailedToRunSsh => {
                return None; // No point trying again. launch_doer_via_ssh will have logged the error already.
            }
            SshDoerLaunchResult::NotPresentOnRemote
            | SshDoerLaunchResult::HandshakeIncompatibleVersion
                if attempt == 0 =>
            {
                deploy = true; // Will attempt to deploy on next loop iteration
            }
            SshDoerLaunchResult::NotPresentOnRemote
            | SshDoerLaunchResult::HandshakeIncompatibleVersion => {
                // If this happens on the second attempt then something is wrong
                error!("Failed to launch remote doer even after new deployment.");
                return None;
            }
            SshDoerLaunchResult::ExitedUnexpectedly => {
                // No point trying again. launch_doer_via_ssh will have logged the error already.
                return None;
            }
            SshDoerLaunchResult::Success {
                ssh_process,
                stdin,
                stdout,
                mut stderr,
            } => {
                debug!("Connection estabilished");

                // Start a background thread to print out log messages from the remote doer,
                // which it can send over its stderr (we use stdout for our regular communications).
                let stderr_reading_thread = std::thread::spawn(move || {
                    loop {
                        let mut l: String = "".to_string();
                        match stderr.read_line(&mut l) {
                            Ok(0) => break, // end of stream
                            Ok(_) => {
                                l.pop(); // Remove the trailing newline
                                // Use a custom target to indicate this is from a remote doer in the log output
                                // Preserve the log level of the remote messages if possible
                                match l.split_once(' ') {
                                    Some((level_str, msg)) => {
                                        match log::Level::from_str(level_str) {
                                            Ok(level) => log!(target: "remote doer", level, "{}", msg),
                                            Err(_) => debug!(target: "remote doer", "{}", l),
                                        }                                        
                                    }
                                    None => debug!(target: "remote doer", "{}", l),
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });

                return Some(Comms::Remote {
                    debug_name: "Remote ".to_string() + &debug_name + " at " + remote_hostname,
                    ssh_process,
                    stdin: BufWriter::new(stdin),
                    stdout,
                    stderr_reading_thread,
                });
            }
        };
    }
    panic!("Unreachable code");
}

// Result of launch_doer_via_ssh function.
enum SshDoerLaunchResult {
    /// The ssh process couldn't be started, for example because the ssh executable isn't available on the PATH.
    FailedToRunSsh,
    /// We connected to the remote computer, but couldn't launch rjrssync because it didn't exist.
    /// This would be expected if this computer has never been used as a remote target before.
    NotPresentOnRemote,
    /// ssh exited before the handshake with rjrssync took place. This could be due to many reasons,
    /// for example rjrssync couldn't launch correctly.
    ExitedUnexpectedly,
    /// rjrssync launched successfully on the remote computer, but it reported a version number that
    /// isn't compatible with our version.
    HandshakeIncompatibleVersion,
    /// rjrssync launched successfully on the remote computer and is a compatible version.
    /// The fields here can be used to communicate with the remote rjrssync.
    Success {
        ssh_process: std::process::Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
        stderr: BufReader<ChildStderr>,
    },
}

// Sent from the threads reading stdout and stderr of ssh back to the main thread.
enum OutputReaderThreadMsg {
    Line(String),
    Error(std::io::Error),
    StreamClosed,
    HandshakeReceived(String, OutputReaderStream), // Also sends back the stream, so the main thread can take back control
}

#[derive(Clone, Copy)]
enum OutputReaderStreamType {
    Stdout,
    Stderr,
}
impl Display for OutputReaderStreamType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OutputReaderStreamType::Stdout => write!(f, "stdout"),
            OutputReaderStreamType::Stderr => write!(f, "stderr"),
        }
    }
}

// To avoid having to use trait objects, this provides a simple abstraction over either a stdout or stderr.
enum OutputReaderStream {
    Stdout(BufReader<ChildStdout>),
    Stderr(BufReader<ChildStderr>),
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

// Generic thread function for reading from stdout or stderr of the ssh process.
// It sends messages back to the main thread using a channel.
// We need to handle both in the same way - waiting until we receive the handshake line indicating that the doer
// copy has started up correctly. We need a handshake on both threads so that each background thread knows when
// it's time to finish and return control of the stream to the main thread.
fn output_reader_thread_main(
    mut stream: OutputReaderStream,
    sender: Sender<(OutputReaderStreamType, OutputReaderThreadMsg)>,
) -> Result<(), SendError<(OutputReaderStreamType, OutputReaderThreadMsg)>> {
    let stream_type = stream.get_type();
    loop {
        let mut l: String = "".to_string();
        // Note we ignore errors on the sender here, as the other end should never have been dropped while it still cares
        // about our messages, but may have dropped if they abandon the ssh process, letting it finish itself.
        match stream.read_line(&mut l) {
            Err(e) => {
                sender.send((stream_type, OutputReaderThreadMsg::Error(e)))?;
                return Ok(());
            }
            Ok(0) => {
                // end of stream
                sender.send((stream_type, OutputReaderThreadMsg::StreamClosed))?;
                return Ok(());
            }
            Ok(_) => {
                l.pop(); // Remove the trailing newline
                if l.starts_with(HANDSHAKE_MSG) {
                    // remote end has booted up properly and is ready for comms.
                    // finish this thread and return control of the stdout to the main thread, so it can communicate directly
                    //TODO: this isn't the end of the handshake any more! need toe exchange secrets etc.
                    // However we should check the version first before proceeding to secret exchange, and give up
                    // if it's the wrong version (as the secret exchange protocol may have changed!)
                    sender.send((
                        stream_type,
                        OutputReaderThreadMsg::HandshakeReceived(l, stream),
                    ))?;
                    return Ok(());
                } else {
                    // A line of other content, for example a prompt or error from ssh itself
                    sender.send((stream_type, OutputReaderThreadMsg::Line(l)))?;
                }
            }
        }
    }
}

/// Attempts to launch a remote copy of rjrssync on the given remote computer using ssh.
fn launch_doer_via_ssh(remote_hostname: &str, remote_user: &str) -> SshDoerLaunchResult {
    let user_prefix = if remote_user.is_empty() {
        "".to_string()
    } else {
        remote_user.to_string() + "@"
    };
    // Note we don't cd, so that relative paths for the folder specified by the user on the remote
    // will be correct
    let remote_command = format!("{}{}target/release/rjrssync --doer",
        // Forward the RUST_LOG env var, so that our logging levels are in sync
        std::env::var("RUST_LOG").map_or("".to_string(), |e| format!("RUST_LOG={} ", e)),
        REMOTE_TEMP_FOLDER);
    debug!("Running remote command: {}", remote_command);
    // Note we use the user's existing ssh tool so that their config/settings will be used for
    // logging in to the remote system (as opposed to using an ssh library called from our code).
    let mut ssh_process = match std::process::Command::new("ssh")
        .arg(user_prefix + remote_hostname)
        .arg(remote_command)
        // Note that even though we're piping stdin, ssh still seems able to accept answers to prompts about
        // host key verification and password input somehow.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            error!("Error launching ssh: {}", e);
            return SshDoerLaunchResult::FailedToRunSsh;
        }
    };

    // Some of the output from ssh (errors etc.) comes on stderr, so we need to display both stdout and stderr
    // to the user, before the handshake is estabilished.
    debug!("Waiting for remote copy to send version handshake");

    // unwrap is fine here, as the streams should always be available as we piped them all
    let ssh_stdin = ssh_process.stdin.take().unwrap();
    let ssh_stdout = ssh_process.stdout.take().unwrap();
    let ssh_stderr = ssh_process.stderr.take().unwrap();

    // Spawn a background thread for each stdout and stderr, to process messages we get from ssh
    // and forward them to the main thread. This is easier than some kind of async IO stuff.
    let (sender1, receiver): (
        Sender<(OutputReaderStreamType, OutputReaderThreadMsg)>,
        Receiver<(OutputReaderStreamType, OutputReaderThreadMsg)>,
    ) = mpsc::channel();
    let sender2 = sender1.clone();
    std::thread::spawn(move || {
        output_reader_thread_main(
            OutputReaderStream::Stdout(BufReader::new(ssh_stdout)),
            sender1,
        )
    });
    std::thread::spawn(move || {
        output_reader_thread_main(
            OutputReaderStream::Stderr(BufReader::new(ssh_stderr)),
            sender2,
        )
    });

    // Wait for messages from the background threads which are reading stdout and stderr
    #[derive(Default)]
    struct HandshookStdoutAndStderr {
        stdout: Option<BufReader<ChildStdout>>,
        stderr: Option<BufReader<ChildStderr>>,
    }
    let mut handshook_stdout_and_stderr = HandshookStdoutAndStderr::default();
    loop {
        match receiver.recv() {
            Ok((stream_type, OutputReaderThreadMsg::Line(l))) => {
                // Show ssh output to the user, as this might be useful/necessary
                info!("ssh {}: {}", stream_type, l);
                if l.contains("No such file or directory") {
                    warn!("rjrssync not present on remote computer");
                    // Note the stdin of the ssh will be dropped and this will tidy everything up nicely
                    return SshDoerLaunchResult::NotPresentOnRemote;
                }
            }
            Ok((stream_type, OutputReaderThreadMsg::HandshakeReceived(line, s))) => {
                debug!("Handshake received from {}: {}", stream_type, line);
                match s {
                    OutputReaderStream::Stdout(b) => handshook_stdout_and_stderr.stdout = Some(b),
                    OutputReaderStream::Stderr(b) => handshook_stdout_and_stderr.stderr = Some(b),
                }

                let remote_version = line.split_at(HANDSHAKE_MSG.len()).1;
                if remote_version != VERSION.to_string() {
                    warn!(
                        "Remote server has incompatible version ({} vs local version {})",
                        remote_version, VERSION
                    );
                    // Note the stdin of the ssh will be dropped and this will tidy everything up nicely
                    return SshDoerLaunchResult::HandshakeIncompatibleVersion;
                }

                // Need to wait for both stdout and stderr to pass the handshake
                if let HandshookStdoutAndStderr { stdout: Some(stdout), stderr: Some(stderr) } = handshook_stdout_and_stderr {
                    return SshDoerLaunchResult::Success {
                        ssh_process,
                        stdin: ssh_stdin,
                        stdout, 
                        stderr, 
                    };
                };
            }
            Ok((stream_type, OutputReaderThreadMsg::Error(e))) => {
                error!("Error reading from {}: {}", stream_type, e);
            }
            Ok((stream_type, OutputReaderThreadMsg::StreamClosed)) => {
                debug!("ssh {} closed", stream_type);
            }
            Err(RecvError) => {
                // Both senders have been dropped, i.e. both background threads exited, before
                // we got any expected error (e.g. "No such file or directory"), so we don't know
                // why it exited.
                debug!("Both reader threads done, ssh must have exited. Waiting for process.");
                // Wait for the process to exit, for tidyness
                let result = ssh_process.wait();
                error!("ssh exited unexpectedly with {:?}", result);
                return SshDoerLaunchResult::ExitedUnexpectedly;
            }
        }
    }
}

// This embeds the source code of the program into the executable, so it can be deployed remotely and built on other platforms
#[derive(RustEmbed)]
#[folder = "."]
#[include = "src/*"]
#[include = "Cargo.*"]
struct EmbeddedSource;

/// Deploys the source code of rjrssync to the given remote computer and builds it, ready to be executed.
fn deploy_to_remote(remote_hostname: &str, remote_user: &str) -> Result<(), ()> {
    debug!("Deploying onto '{}'", &remote_hostname);

    // Copy our embedded source tree to the remote, so we can build it there.
    // (we can't simply copy the binary as it might not be compatible with the remote platform)
    // We use the user's existing ssh/scp tool so that their config/settings will be used for
    // logging in to the remote system (as opposed to using an ssh library called from our code).

    // Extract embedded source code to a temporary local folder
    let local_temp_dir = match TempDir::new("rjrssync") {
        Ok(x) => x,
        Err(e) => {
            error!("Error creating temp dir: {}", e);
            return Err(());
        }
    };
    debug!(
        "Extracting embedded source to local temp dir: {}",
        local_temp_dir.path().display()
    );
    for file in EmbeddedSource::iter() {
        // Add an extra "rjrssync" folder with a fixed name (as opposed to the temp dir, whose name varies), to work around SCP weirdness below.
        let local_temp_path = local_temp_dir.path().join("rjrssync").join(&*file);

        if let Err(e) = std::fs::create_dir_all(local_temp_path.parent().unwrap()) {
            error!("Error creating folders for local temp file: {}", e);
            return Err(());
        }

        let mut f = match std::fs::File::create(&local_temp_path) {
            Ok(x) => x,
            Err(e) => {
                error!(
                    "Error creating local temp file {}: {}",
                    local_temp_path.display(),
                    e
                );
                return Err(());
            }
        };

        if let Err(e) = f.write_all(&EmbeddedSource::get(&file).unwrap().data) {
            error!("Error writing local temp file: {}", e);
            return Err(());
        }
    }

    // Deploy to remote target using scp
    // Note we need to deal with the case where the the remote folder doesn't exist, and the case where it does, so
    // we copy into /tmp (which should always exist), rather than directly to /tmp/rjrssync which may or may not
    let remote_temp_folder = PathBuf::from(REMOTE_TEMP_FOLDER);
    let user_prefix = if remote_user.is_empty() {
        "".to_string()
    } else {
        remote_user.to_string() + "@"
    };
    let source_spec = local_temp_dir.path().join("rjrssync");
    let remote_spec = user_prefix.clone()
        + remote_hostname
        + ":"
        + remote_temp_folder.parent().unwrap().to_str().unwrap();
    debug!("Copying {} to {}", source_spec.display(), remote_spec);
    // Note that we run "cmd /C scp ..." rather than just "scp", otherwise the line endings get messed up and subsequent log messages are broken.
    match std::process::Command::new("cmd")
        .arg("/C")
        .arg("scp")
        .arg("-r")
        .arg(source_spec)
        .arg(remote_spec)
        .status()
    {
        Err(e) => {
            error!("Error launching scp: {}", e);
            return Err(());
        }
        Ok(s) if s.code() == Some(0) => {
            // Good!
        }
        Ok(s) => {
            error!("Error copying source code. Exit status from scp: {}", s);
            return Err(());
        }
    };

    // Build the program remotely (using the cargo on the remote system)
    // Note that we could merge this ssh command with the one to run the program once it's built (in launch_doer_via_ssh),
    // but this would make error reporting slightly more difficult as the command in launch_doer_via_ssh is more tricky as
    // we are parsing the stdout, but for the command here we can wait for it to finish easily.
    let remote_command = format!(
        "cd {} && cargo build --release",
        remote_temp_folder.display()
    );
    debug!("Running remote command: {}", remote_command);
    // Note that we run "cmd /C ssh ..." rather than just "ssh", otherwise the line endings get messed up and subsequent log messages are broken.
    match std::process::Command::new("cmd")
        .arg("/C")
        .arg("ssh")
        .arg(user_prefix + remote_hostname)
        .arg(remote_command)
        .status()
    {
        Err(e) => {
            error!("Error launching ssh: {}", e);
            return Err(());
        }
        Ok(s) if s.code() == Some(0) => {
            // Good!
        }
        Ok(s) => {
            error!("Error building on remote. Exit status from ssh: {}", s);
            return Err(());
        }
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn parse_remote_folder_desc() {
        // There's some quirks here with windows paths containing colons for drive letters

        assert_eq!(
            RemoteFolderDesc::from_str(""),
            Err("Folder must be specified".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("f"),
            Ok(RemoteFolderDesc {
                folder: "f".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str("h:f"),
            Ok(RemoteFolderDesc {
                folder: "f".to_string(),
                hostname: "h".to_string(),
                username: "".to_string()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str("hh:"),
            Err("Folder must be specified".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str(":f"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str(":"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("@"),
            Ok(RemoteFolderDesc {
                folder: "@".to_string(),
                ..Default::default()
            })
        );

        assert_eq!(
            RemoteFolderDesc::from_str("u@h:f"),
            Ok(RemoteFolderDesc {
                folder: "f".to_string(),
                hostname: "h".to_string(),
                username: "u".to_string()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str("@h:f"),
            Err("Missing username".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("u@h:"),
            Err("Folder must be specified".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("u@:f"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("@:f"),
            Err("Missing username".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("u@:"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemoteFolderDesc::from_str("@h:"),
            Err("Missing username".to_string())
        );

        assert_eq!(
            RemoteFolderDesc::from_str("u@f"),
            Ok(RemoteFolderDesc {
                folder: "u@f".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str("@f"),
            Ok(RemoteFolderDesc {
                folder: "@f".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str("u@"),
            Ok(RemoteFolderDesc {
                folder: "u@".to_string(),
                ..Default::default()
            })
        );

        assert_eq!(
            RemoteFolderDesc::from_str("u:u@u:u@h:f:f:f@f"),
            Ok(RemoteFolderDesc {
                folder: "u@u:u@h:f:f:f@f".to_string(),
                hostname: "u".to_string(),
                username: "".to_string()
            })
        );

        assert_eq!(
            RemoteFolderDesc::from_str(r"C:\Path\On\Windows"),
            Ok(RemoteFolderDesc {
                folder: r"C:\Path\On\Windows".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"C:"),
            Ok(RemoteFolderDesc {
                folder: r"C:".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"C:\"),
            Ok(RemoteFolderDesc {
                folder: r"C:\".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"C:folder"),
            Ok(RemoteFolderDesc {
                folder: r"folder".to_string(),
                hostname: "C".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"C:\folder"),
            Ok(RemoteFolderDesc {
                folder: r"C:\folder".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"CC:folder"),
            Ok(RemoteFolderDesc {
                folder: r"folder".to_string(),
                hostname: "CC".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"CC:\folder"),
            Ok(RemoteFolderDesc {
                folder: r"\folder".to_string(),
                hostname: "CC".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"s:C:\folder"),
            Ok(RemoteFolderDesc {
                folder: r"C:\folder".to_string(),
                hostname: "s".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str(r"u@s:C:\folder"),
            Ok(RemoteFolderDesc {
                folder: r"C:\folder".to_string(),
                hostname: "s".to_string(),
                username: "u".to_string()
            })
        );

        assert_eq!(
            RemoteFolderDesc::from_str(r"\\network\share\windows"),
            Ok(RemoteFolderDesc {
                folder: r"\\network\share\windows".to_string(),
                ..Default::default()
            })
        );

        assert_eq!(
            RemoteFolderDesc::from_str("/unix/absolute"),
            Ok(RemoteFolderDesc {
                folder: "/unix/absolute".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemoteFolderDesc::from_str("username@server:/unix/absolute"),
            Ok(RemoteFolderDesc {
                folder: "/unix/absolute".to_string(),
                hostname: "server".to_string(),
                username: "username".to_string()
            })
        );
    }
}
