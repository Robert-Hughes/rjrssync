use aes_gcm::aead::{OsRng};
use aes_gcm::{Aes128Gcm, KeyInit, Key};
use log::{debug, error, info, warn, log, trace};
use rust_embed::RustEmbed;
use std::io::LineWriter;
use std::net::{TcpStream};
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::{
    fmt::{self, Display},
    io::{BufRead, BufReader, Write},
    process::{ChildStderr, ChildStdin, ChildStdout, Stdio},
    sync::mpsc::{RecvError, SendError},
    thread::JoinHandle,
};
use tempdir::TempDir;

use crate::*;

/// Abstraction of two-way communication channel between this boss and a doer, which might be
/// remote (communicating over an encrypted TCP connection) or local (communicating via a channel to a background thread).
#[allow(clippy::large_enum_variant)]
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
        // Once the network socket is set up, we don't need to communicate over stdin/stdout any more,
        // but we keep these around anyway in case.
        stdin: LineWriter<ChildStdin>,
        stdout: BufReader<ChildStdout>,
        stderr_reading_thread: JoinHandle<()>,

        tcp_connection: TcpStream,
        cipher: Aes128Gcm,
        sending_nonce_counter: u64,
        receiving_nonce_counter: u64,
    },
}
impl Comms {
    pub fn send_command(&mut self, c: Command) -> Result<(), String> {
        trace!("Sending command {:?} to {}", c, &self);
        let res =
            match self {
                Comms::Local { sender, .. } => {
                    sender.send(c).map_err(|e| "Error sending on channel: ".to_string() + &e.to_string())
                }
                Comms::Remote { tcp_connection, cipher, sending_nonce_counter, .. } => {
                    encrypted_comms::send(c, tcp_connection, cipher, sending_nonce_counter, 0)
                }
            };
        if let Err(ref e) = &res {
            error!("Error sending command: {:?}", e);
        }
        res
    }

    pub fn receive_response(&mut self) -> Result<Response, String> {
        trace!("Waiting for response from {}", &self);
        let res = match self {
            Comms::Local { receiver, .. } => {
                receiver.recv().map_err(|e| "Error receiving from channel: ".to_string() + &e.to_string())
            }
            Comms::Remote { tcp_connection, cipher, receiving_nonce_counter, .. } => {
                encrypted_comms::receive(tcp_connection, cipher, receiving_nonce_counter, 1)
            }
        };
        match &res {
            Err(ref e) => error!("{}", e),
            Ok(ref r) => trace!("Received response {:?} from {}", r, &self),
        }
        res
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
    // Tell the other end (thread or process over network) to shutdown once we're finished.
    // They should exit anyway due to a disconnection (of their channel or stdin), but this
    // gives a cleaner exit without errors.
    fn drop(&mut self) {
        // There's not much we can do about an error here, other than log it, which send_command already does, so we ignore any error.
        let _ = self.send_command(Command::Shutdown);
    }
}

// Sets up communications with the given computer, which may be either remote or local (if remote_hostname is empty).
pub fn setup_comms(
    remote_hostname: &str,
    remote_user: &str,
    remote_port_for_comms: u16,
    debug_name: String,
    force_redeploy: bool,
) -> Option<Comms> {
    debug!(
        "setup_comms with hostname '{}' and username '{}'. debug_name = {}",
        remote_hostname, remote_user, debug_name
    );

    // If the target is local, then start a thread to handle commands.
    // Use a separate thread to avoid synchornisation with the Boss (and both Source and Dest may be on same PC, so all three in one process),
    // and for consistency with remote doers.
    if remote_hostname.is_empty() {
        debug!("Spawning local thread for {} doer", debug_name);
        let (command_sender, command_receiver) = mpsc::channel();
        let (response_sender, response_receiver) = mpsc::channel();
        let thread_builder = thread::Builder::new().name(debug_name.clone());
        let thread = thread_builder.spawn(move || {
            doer_thread_running_on_boss(command_receiver, response_sender)
        }).unwrap();
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

        match launch_doer_via_ssh(remote_hostname, remote_user, remote_port_for_comms) {
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
            SshDoerLaunchResult::ExitedUnexpectedly | SshDoerLaunchResult::CommunicationError => {
                // No point trying again. launch_doer_via_ssh will have logged the error already.
                return None;
            }
            SshDoerLaunchResult::Success {
                ssh_process,
                stdin,
                stdout,
                stderr,
                secret_key,
            } => {
                // Start a background thread to print out log messages from the remote doer,
                // which it can send over its stderr.
                let stderr_reading_thread = std::thread::spawn(move || remote_doer_logging_thread(stderr));

                // Connect to the network port that the doer should be listening on
                let addr = (remote_hostname, remote_port_for_comms);
                debug!("Connecting to doer over network at {:?}", addr);
                //TODO: this has a delay ~1 sec even when connecting to localhost, apparently because it first tries connecting to the
                // IPv6 local address, and it has to wait for this to fail before trying IPv4. Maybe can skip this?
                let tcp_connection = match TcpStream::connect(addr) {
                    Ok(t) => {
                        debug!("Connected! {:?}", t);
                        t
                    }
                    Err(e) => {
                        error!("Failed to connect to network port: {}", e);
                        return None;
                    }
                };

                return Some(Comms::Remote {
                    debug_name: "Remote ".to_string() + &debug_name + " at " + remote_hostname,
                    ssh_process,
                    stdin,
                    stdout,
                    stderr_reading_thread,
                    tcp_connection,
                    cipher: Aes128Gcm::new(&secret_key),
                    sending_nonce_counter: 0, // Nonce counters must be different, so sender and receiver don't reuse
                    receiving_nonce_counter: 1,
                });
            }
        };
    }
    panic!("Unreachable code");
}

fn remote_doer_logging_thread(mut stderr: BufReader<ChildStderr>) {
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
    /// Failed to send/receive some data on stdin/stdout commuicating with the doer.
    CommunicationError,
    /// rjrssync launched successfully on the remote computer, but it reported a version number that
    /// isn't compatible with our version.
    HandshakeIncompatibleVersion,
    /// rjrssync launched successfully on the remote computer, is a compatible version, and is now
    /// listening for an incoming network connection on the requested port. It has been provided
    /// with a secret shared key for encryption, which is stored here too.
    /// The fields here can be used to communicate with the remote rjrssync via its stdin/stdout,
    /// but we use the network connection for the main data transfer because it is faster (see README.md).
    Success {
        ssh_process: std::process::Child,
        stdin: LineWriter<ChildStdin>,
        stdout: BufReader<ChildStdout>,
        stderr: BufReader<ChildStderr>,
        secret_key: Key<Aes128Gcm>,
    },
}

// Sent from the threads reading stdout and stderr of ssh back to the main thread.
enum OutputReaderThreadMsg {
    Line(String),
    Error(std::io::Error),
    StreamClosed,
    HandshakeStarted(String),
    HandshakeCompleted(OutputReaderStream), // Also sends back the stream, so the main thread can take back control
}

#[derive(Clone, Copy, PartialEq)]
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
// We need to handle both in the same way - waiting until we complete the handshake indicating that the doer
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
                if l.starts_with(HANDSHAKE_STARTED_MSG) {
                    // Check the version first before passing the secret, so that we can stop
                    // if it's the wrong version (as the secret exchange protocol may have changed!)
                    sender.send((
                        stream_type,
                        OutputReaderThreadMsg::HandshakeStarted(l),
                    ))?;
                } else if l == HANDSHAKE_COMPLETED_MSG {
                    // Remote end has booted up properly and is ready for network connection.
                    // Finish this thread and return control of the stdout to the main thread, so it can communicate directly
                    sender.send((
                        stream_type,
                        OutputReaderThreadMsg::HandshakeCompleted(stream),
                    ))?;
                    return Ok(());
                }
                else {
                    // A line of other content, for example a prompt or error from ssh itself
                    sender.send((stream_type, OutputReaderThreadMsg::Line(l)))?;
                }
            }
        }
    }
}

/// Attempts to launch a remote copy of rjrssync on the given remote computer using ssh.
/// Additionally checks that the remote doer is a compatible version, and is now
/// listening for an incoming network connection on the requested port. It is also provided
/// with a randomly generated secret shared key for encryption, which is returned to the caller
/// for setting up encrypted communication over the network connection.
fn launch_doer_via_ssh(remote_hostname: &str, remote_user: &str, remote_port_for_comms: u16) -> SshDoerLaunchResult {
    let user_prefix = if remote_user.is_empty() {
        "".to_string()
    } else {
        remote_user.to_string() + "@"
    };

    // Forward our logging configuration to the remote doer, so that our logging levels are in sync.
    // Note that forwarding it as an env var is more complicated on Windows (no "ENV=VALUE cmd" syntax), so
    // we use a command-line arg instead.
    // We need to forward both RUST_LOG and also any command-line setting (--quiet/--verbose flags).
    // Unfortunately there isn't a way to reconstruct a log filter string from the current config,
    // so we have to do it manually.
    let log_arg = if let Ok(l) = std::env::var("RUST_LOG") {
        format!(" --log-filter {} ", l)
    } else {
        format!(" --log-filter {} ", log::max_level()) // max_level will be affected by --quiet/--verbose
    };

    // Note we don't cd, so that relative paths for the path specified by the user on the remote
    // will be correct (relative to their ssh default dir, e.g. home dir)
    let doer_args = format!("--doer {} --port {}", log_arg, remote_port_for_comms);
    // Try launching using both Unix and Windows paths, as we don't know what the remote system is
    // uname and ver are used to check the OS before attempting to run using that path, but we pipe
    // their result to /dev/null (or equivalent) so they don't appear in the output.
    // We run a command that doesn't print out anything on both Windows and Linux, so we don't pollute the output
    // (we show all output from ssh, in case it contains prompts etc. that are useful/required for the user to see).
    // Note the \n to send a two-line command - it seems Windows ignores this, but Linux runs it.
    let windows_command = format!("{}rjrssync\\target\\release\\rjrssync.exe {}", REMOTE_TEMP_WINDOWS, doer_args);
    let unix_command = format!("{}rjrssync/target/release/rjrssync {}", REMOTE_TEMP_UNIX, doer_args);
    let remote_command = format!("echo >/dev/null # >nul & {windows_command}\n{unix_command}");
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
    let mut ssh_stdin = LineWriter::new(ssh_process.stdin.take().unwrap());
    let ssh_stdout = ssh_process.stdout.take().unwrap();
    let ssh_stderr = ssh_process.stderr.take().unwrap();

    // Spawn a background thread for each stdout and stderr, to process messages we get from ssh
    // and forward them to the main thread. This is easier than some kind of async IO stuff.
    #[allow(clippy::type_complexity)]
    let (sender1, receiver): (
        Sender<(OutputReaderStreamType, OutputReaderThreadMsg)>,
        Receiver<(OutputReaderStreamType, OutputReaderThreadMsg)>,
    ) = mpsc::channel();
    let sender2 = sender1.clone();
    let thread_builder = thread::Builder::new().name("ssh_stdout_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main(
            OutputReaderStream::Stdout(BufReader::new(ssh_stdout)),
            sender1,
        )
    }).unwrap();
    let thread_builder = thread::Builder::new().name("ssh_stderr_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main(
            OutputReaderStream::Stderr(BufReader::new(ssh_stderr)),
            sender2,
        )
    }).unwrap();

    // Wait for messages from the background threads which are reading stdout and stderr
    #[derive(Default)]
    struct HandshookStdoutAndStderr {
        stdout: Option<BufReader<ChildStdout>>,
        stderr: Option<BufReader<ChildStderr>>,
        secret_key: Option<Key<Aes128Gcm>>,
    }
    let mut handshook_data = HandshookStdoutAndStderr::default();
    loop {
        match receiver.recv() {
            Ok((stream_type, OutputReaderThreadMsg::Line(l))) => {
                // Show ssh output to the user, as this might be useful/necessary
                info!("ssh {}: {}", stream_type, l);
                // Check for both the Linux (bash) and Windows (cmd) errors
                if l.contains("No such file or directory") || 
                    l.contains("The system cannot find the path specified") ||
                    l.contains("is not recognized as an internal or external command") {
                    warn!("rjrssync not present on remote computer");
                    // Note the stdin of the ssh will be dropped and this will tidy everything up nicely
                    return SshDoerLaunchResult::NotPresentOnRemote;
                }
            }
            Ok((stream_type, OutputReaderThreadMsg::HandshakeStarted(line))) => {
                debug!("Handshake started on {}: {}", stream_type, line);

                let remote_version = line.split_at(HANDSHAKE_STARTED_MSG.len()).1;
                if remote_version != VERSION.to_string() {
                    warn!(
                        "Remote server has incompatible version ({} vs local version {})",
                        remote_version, VERSION
                    );
                    // Note the stdin of the ssh will be dropped and this will tidy everything up nicely
                    //TODO: i'm not so sure - we seem to be leaving 'orphaned' doers running on the remote side!
                    // Remote process not closing when stdin is closed because we're now waiting for tcp connection, whereas before we were reading from stdin so when it was closed, we errored and exited
                    return SshDoerLaunchResult::HandshakeIncompatibleVersion;
                }

                // Generate and send a secret key, so that we can authenticate/encrypt the network connection
                // Only do this once (when stdout has passed the version check, not on stderr too)
                if stream_type == OutputReaderStreamType::Stdout {
                    debug!("Sending secret key");
                    // Note that we generate a new key for each doer, otherwise the nonces would be re-used with the same key
                    let key = Aes128Gcm::generate_key(&mut OsRng);
                    let mut msg = base64::encode(key).as_bytes().to_vec();
                    msg.push(b'\n');
                    if let Err(e) = ssh_stdin.write_all(&msg) {
                        error!("Failed to send secret: {}", e);
                        return SshDoerLaunchResult::CommunicationError;
                    }
                    handshook_data.secret_key = Some(key); // Remember the key - we'll need it too!
                }
            }
            Ok((stream_type, OutputReaderThreadMsg::HandshakeCompleted(s))) => {
                debug!("Handshake completed on {}", stream_type);
                match s {
                    OutputReaderStream::Stdout(b) => handshook_data.stdout = Some(b),
                    OutputReaderStream::Stderr(b) => handshook_data.stderr = Some(b),
                }

                // Need to wait for both stdout and stderr to pass the handshake
                if let HandshookStdoutAndStderr { stdout: Some(stdout), stderr: Some(stderr), secret_key: Some(secret_key) } = handshook_data {
                    return SshDoerLaunchResult::Success {
                        ssh_process,
                        stdin: ssh_stdin,
                        stdout,
                        stderr,
                        secret_key,
                    };
                };
            }
            Ok((stream_type, OutputReaderThreadMsg::Error(e))) => {
                error!("Error reading from {}: {}", stream_type, e);
                return SshDoerLaunchResult::CommunicationError;
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

    let user_prefix = if remote_user.is_empty() {
        "".to_string()
    } else {
        remote_user.to_string() + "@"
    };

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

    // Determine if the target system is Windows or Linux, so that we know where to copy our files to
    let remote_command = "uname || ver"; // uname for Linux, ver for Windows
    debug!("Running remote command: {}", remote_command);
    let os_test_output = match std::process::Command::new("ssh")
        .arg(user_prefix.clone() + remote_hostname)
        .arg(remote_command)
        .output()
    {
        Err(e) => {
            error!("Error launching ssh: {}", e);
            return Err(());
        }
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        Ok(output) => {
            //TODO: if this fails, we don't print the stdout or stderr, so the user won't see why.
            // This is especially important if using --force-redeploy on a broken remote, as you don't see any errors from the initial attempt to connect either
            error!("Error checking remote OS. Exit status from ssh: {}", output.status);
            return Err(());
        }
    };
    // We could check for "linux" in the string, but there are other Unix systems we might want to supoprt e.g. Mac,
    // so we fall back to Linux as a default
    let is_windows = os_test_output.contains("Windows");

    // Deploy to remote target using scp
    // Note we need to deal with the case where the the remote folder doesn't exist, and the case where it does, so
    // we copy into /tmp (which should always exist), rather than directly to /tmp/rjrssync which may or may not
    // We leave stdout and stderr to inherit, so the user can see what's happening and if there are any errors
    let source_spec = local_temp_dir.path().join("rjrssync");
    let remote_temp = if is_windows {
        REMOTE_TEMP_WINDOWS
    } else {
        REMOTE_TEMP_UNIX
    };
    let remote_spec = format!("{user_prefix}{remote_hostname}:{remote_temp}");
    debug!("Copying {} to {}", source_spec.display(), remote_spec);
    match std::process::Command::new("scp")
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
    // We leave stdout and stderr to inherit, so the user can see what's happening and if there are any errors
    let cargo_command = "cargo build --release";
    let remote_command = if is_windows {
        format!("cd /d {REMOTE_TEMP_WINDOWS}\\rjrssync && {cargo_command}")
    } else {
        // We use "$SHELL -lc" to run a login shell, as cargo might not be on the PATH otherwise.
        format!("$SHELL -lc 'cd {REMOTE_TEMP_UNIX}/rjrssync && {cargo_command}'")
    };
    debug!("Running remote command: {}", remote_command);
    match std::process::Command::new("ssh")
        .arg("-t") // This fixes issues with line endings getting messed up after ssh exits 
        //TODO: but it seems to mess up line endings etc. when running remote_tests with --nocapture! on windows at least, and breaks the tests entirely!
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
