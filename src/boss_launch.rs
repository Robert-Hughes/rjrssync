use aes_gcm::aead::{OsRng};
use aes_gcm::{Aes128Gcm, KeyInit, Key};
use log::{debug, error, info, log, trace};
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::LineWriter;
use std::net::{TcpStream};
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;
use std::{
    fmt::{self, Display},
    io::{BufRead, BufReader, Write},
    process::{ChildStderr, ChildStdin, ChildStdout, Stdio},
    sync::mpsc::{RecvError, SendError},
    thread::JoinHandle,
};
use tempdir::TempDir;

use crate::*;
use crate::encrypted_comms::AsyncEncryptedComms;

/// Abstraction of two-way communication channel between this boss and a doer, which might be
/// remote (communicating over an encrypted TCP connection) or local (communicating via a channel to a background thread).
#[allow(clippy::large_enum_variant)]
pub enum Comms {
    Local {
        debug_name: String, // To identify this Comms against others for debugging, when there are several
        thread: JoinHandle<Result<(), String>>,
        sender: memory_bound_channel::Sender<Command>,
        receiver: memory_bound_channel::Receiver<Response>,
    },
    Remote {
        debug_name: String, // To identify this Comms against others for debugging, when there are several
        ssh_process: std::process::Child,
        // Once the network socket is set up, we don't need to communicate over stdin/stdout any more,
        // but we keep these around anyway in case.
        stdin: LineWriter<ChildStdin>,
        stdout: BufReader<ChildStdout>,
        stderr_reading_thread: JoinHandle<()>,

        encrypted_comms: AsyncEncryptedComms<Command, Response>,
    },
}
impl Comms {
    pub fn get_sender(&self) -> &memory_bound_channel::Sender<Command> {
        match self {
            Comms::Local { sender, .. } => &sender,
            Comms::Remote { encrypted_comms, .. } => &encrypted_comms.sender,
        }
    }

    pub fn get_receiver(&self) -> &memory_bound_channel::Receiver<Response> {
        match self {
            Comms::Local { receiver, .. } => &receiver,
            Comms::Remote { encrypted_comms, .. } => &encrypted_comms.receiver,
        }
    }

    /// This will block if there is not enough capacity in the channel, so
    /// that we don't use up infinite memory if the doer is being slow.
    pub fn send_command(&self, c: Command) -> Result<(), String> {
        trace!("Sending command {:?} to {}", c, &self);
        self.get_sender().send(c).map_err(|_| format!("Lost communication with {}", &self))
    }

    /// Blocks until a response is received, if none if buffered in the channel.
    pub fn receive_response(&self) -> Result<Response, String> {
        trace!("Waiting for response from {}", &self);
        self.get_receiver().recv().map_err(|_| format!("Lost communication with {}", &self))
    }

    /// Never blocks, will return None if the channel is empty.
    pub fn try_receive_response(&self) -> Result<Option<Response>, String>  {
        trace!("Checking for response from {}", &self);
        match self.get_receiver().try_recv() {
            Ok(r) => Ok(Some(r)),
            Err(crossbeam::channel::TryRecvError::Empty) => Ok(None),
            Err(crossbeam::channel::TryRecvError::Disconnected) => Err(format!("Lost communication with {}", &self))
        }
    }

    // Tell the other end (thread or process over network) to shutdown once we're finished.
    // They should exit anyway due to a disconnection (of their channel or stdin), but this
    // gives a cleaner exit without errors.
    pub fn shutdown(self) {
        match self {
            Comms::Local { .. } => {
                // There's not much we can do about an error here, other than log it, which send_command already does, so we ignore any error.
                let _ = self.send_command(Command::Shutdown);
                // Join threads so that they're properly cleaned up including the profiling data
                if let Comms::Local { thread, .. } = self { // Always true, just need to extract the fields
                    if let Err(e) = thread.join().expect("Failed to join local doer thread") {
                        error!("Local doer thread exited with error: {e}");
                    }
                }
            }
            Comms::Remote { ref debug_name, .. } => {
                let _debug_name = debug_name.clone();
                // Synchronise the profiling clocks between local and remote profiling.
                // Do this by sending a special Command which the doer responds to immediately with its local 
                // profiling timer. We then compare that value with our own profiling clock to work out the offset.
                let profiling_offset = if cfg!(feature="profiling") {
                    // Do this a couple of times and take the average
                    let mut samples = vec![];
                    for i in 0..5 {
                        let start = PROFILING_START.elapsed();
                        self.send_command(Command::ProfilingTimeSync).expect("Failed to send profiling time sync");
                        match self.receive_response() {
                            Ok(Response::ProfilingTimeSync(remote_timestamp)) => {
                                let end = PROFILING_START.elapsed();
                                trace!("Profiling sync: start: {:?}, end: {:?}, diff: {:?}, remote: {:?}", start, end, end-start, remote_timestamp);
                                if i >= 2 { // Skip the first two to make sure the doer is ready to go (e.g. code cached)
                                    // Take the average of our local start/end timestamps, assuming that the round trip is symmetrical.
                                    let sample = (start + end) / 2 - remote_timestamp;
                                    trace!("Profiling sync sample: {:?}", sample);
                                    samples.push(sample);
                                }
                            },
                            x => panic!("Unexpected response (expected ProfilingTimeSync): {:?}", x),
                        }
                    }
                    let average = samples.iter().sum::<std::time::Duration>() / samples.len() as u32;
                    trace!("Profiling sync average: {:?}", average);
                    average
                } else {
                    Duration::new(0, 0)
                };

                // There's not much we can do about an error here, other than log it, which send_command already does, so we ignore any error.
                let _ = self.send_command(Command::Shutdown);

                // Shutdown the comms cleanly, potentially getting profiling data at the same time
                if let Comms::Remote { encrypted_comms, mut ssh_process, stdin, stdout, stderr_reading_thread, .. } = self { // This is always true, we just need a way of getting the fields
                    // Wait for remote doers to send back any profiling data, if enabled
                    match encrypted_comms.receiver.recv() {
                        Ok(Response::ProfilingData(x)) => add_remote_profiling(x, _debug_name, profiling_offset),
                        x => error!("Unexpected response as final message (expected ProfilingData): {:?}", x),
                    }

                    encrypted_comms.shutdown();

                    // Wait for the ssh process to cleanly shutdown.
                    // We don't strictly need to do this for most cases, but it's nice to have a clean shutdown.
                    // We do however need to do this when the doer is printing its memory usage, to make sure that we receive it
                    // before closing down ourself.
                    drop(stdin);
                    drop(stdout);
                    debug!("Waiting for stderr_reading_thread");
                    stderr_reading_thread.join().expect("Failed to join stderr_reading_thread");
                    debug!("Waiting for ssh child process");
                    let result = ssh_process.wait();
                    debug!("ssh child process wait result = {:?}", result);
                }
            }
        }
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

// Sets up communications with the given computer, which may be either remote or local (if remote_hostname is empty).
pub fn setup_comms(
    remote_hostname: &str,
    remote_user: &str,
    remote_port_for_comms: Option<u16>,
    debug_name: String,
    force_redeploy: bool,
    needs_deploy_behaviour: NeedsDeployBehaviour,
) -> Option<Comms> {
    profile_this!(format!("setup_comms {}", debug_name));
    debug!(
        "setup_comms with hostname '{}' and username '{}'. debug_name = {}",
        remote_hostname, remote_user, debug_name
    );

    // If the target is local, then start a thread to handle commands.
    // Use a separate thread to avoid synchronisation with the Boss (and both Source and Dest may be on same PC, so all three in one process),
    // and for consistency with remote doers.
    if remote_hostname.is_empty() {
        debug!("Spawning local thread for {} doer", debug_name);
        let debug_name = "Local ".to_string() + &debug_name + " doer";
        let (command_sender, command_receiver) = memory_bound_channel::new(BOSS_DOER_CHANNEL_MEMORY_CAPACITY);
        let (response_sender, response_receiver) = memory_bound_channel::new(BOSS_DOER_CHANNEL_MEMORY_CAPACITY);
        let thread_builder = thread::Builder::new().name(debug_name.clone());
        let thread = thread_builder.spawn(move || {
            doer_thread_running_on_boss(command_receiver, response_sender)
        }).unwrap();
        return Some(Comms::Local {
            debug_name,
            thread,
            sender: command_sender,
            receiver: response_receiver,
        });
    }

    // We first attempt to run a previously-deployed copy of the program on the remote, to save time.
    // If it exists and is a compatible version, we can use that. Otherwise we deploy a new version
    // and try again
    let mut deploy_reason = match force_redeploy {
        true => Some("--force-redeploy was set"),
        false => None
    };
    for attempt in 0..2 {
        if let Some(r) = deploy_reason {
            if deploy_to_remote(remote_hostname, remote_user, r, needs_deploy_behaviour).is_ok() {
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
            SshDoerLaunchResult::NotPresentOnRemote if attempt == 0 => {
                deploy_reason = Some("rjrssync is not present on the remote target"); // Will attempt to deploy on next loop iteration
            }
            SshDoerLaunchResult::HandshakeIncompatibleVersion if attempt == 0 => {
                deploy_reason = Some("the rjrssync version present on the remote target is not compatible"); // Will attempt to deploy on next loop iteration
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
                actual_port,
            } => {
                // Start a background thread to print out log messages from the remote doer,
                // which it can send over its stderr.
                let debug_name_clone = debug_name.clone();
                let stderr_reading_thread = std::thread::spawn(move || remote_doer_logging_thread(stderr, debug_name_clone));

                // Connect to the network port that the doer should be listening on
                let addr = (remote_hostname, actual_port);
                debug!("Connecting to doer over network at {:?}", addr);
                let tcp_connection = {
                    profile_this!("Connecting");
                    match TcpStream::connect(addr) {
                        Ok(t) => {
                            debug!("Connected! {:?}", t);
                            t
                        }
                        Err(e) => {
                            error!("Failed to connect to network port: {}", e);
                            return None;
                        }
                    }
                };

                let debug_comms_name = "Remote ".to_string() + &debug_name;
                return Some(Comms::Remote {
                    debug_name: debug_comms_name.clone(),
                    ssh_process,
                    stdin,
                    stdout,
                    stderr_reading_thread,
                    encrypted_comms: AsyncEncryptedComms::new(
                        tcp_connection,
                        secret_key,
                        0, // Nonce counters must be different, so sender and receiver don't reuse
                        1,
                        ("boss", &debug_comms_name)
                    )
                });
            }
        };
    }
    panic!("Unreachable code");
}

fn remote_doer_logging_thread(mut stderr: BufReader<ChildStderr>, debug_name: String) {
    loop {
        let mut l: String = "".to_string();
        match stderr.read_line(&mut l) {
            Ok(0) => break, // end of stream
            Ok(_) => {
                l.pop(); // Remove the trailing newline
                // Use a custom target to indicate this is from a remote doer in the log output
                // Preserve the log level of the remote messages if possible
                match &l.splitn(4, ' ').collect::<Vec<&str>>()[..] {
                    [timestamp, level_str, target, msg] => {
                        let target = format!("remote {debug_name}: {target}");
                        match log::Level::from_str(level_str) {
                            Ok(level) => log!(target: &target, level, "{} {}", timestamp, msg),
                            Err(_) => debug!(target: &target, "{}", l),
                        }
                    }
                    _ => debug!(target: &format!("remote {debug_name}"), "{}", l),
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
    /// for example rjrssync couldn't launch correctly because it bind to a free port.
    ExitedUnexpectedly,
    /// Failed to send/receive some data on stdin/stdout commuicating with the doer.
    CommunicationError,
    /// rjrssync launched successfully on the remote computer, but it reported a version number that
    /// isn't compatible with our version.
    HandshakeIncompatibleVersion,
    /// rjrssync launched successfully on the remote computer, is a compatible version, and is now
    /// listening for an incoming network connection on the actual_port. It has been provided
    /// with a secret shared key for encryption, which is stored here too.
    /// The fields here can be used to communicate with the remote rjrssync via its stdin/stdout,
    /// but we use the network connection for the main data transfer because it is faster (see README.md).
    Success {
        ssh_process: std::process::Child,
        stdin: LineWriter<ChildStdin>,
        stdout: BufReader<ChildStdout>,
        stderr: BufReader<ChildStderr>,
        secret_key: Key<Aes128Gcm>,
        actual_port: u16
    },
}

// Sent from the threads reading stdout and stderr of ssh back to the main thread.
enum OutputReaderThreadMsg {
    Line(String),
    Error(std::io::Error),
    StreamClosed,
    HandshakeStarted(String),
    HandshakeCompleted(String, OutputReaderStream), // Also sends back the stream, so the main thread can take back control
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
                } else if l.starts_with(HANDSHAKE_COMPLETED_MSG) {
                    // Remote end has booted up properly and is ready for network connection.
                    // Finish this thread and return control of the stdout to the main thread, so it can communicate directly
                    sender.send((
                        stream_type,
                        OutputReaderThreadMsg::HandshakeCompleted(l, stream),
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
fn launch_doer_via_ssh(remote_hostname: &str, remote_user: &str, remote_port_for_comms: Option<u16>) -> SshDoerLaunchResult {
    profile_this!();
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

    // Forward any specific port request to the remote doer
    let port_arg = match remote_port_for_comms {
        Some(p) => format!(" --port {p}"),
        None => "".to_string()
    };

    // Forward memory dumping flag to the remote doer
    let memory_dump_arg = match std::env::var("RJRSSYNC_TEST_DUMP_MEMORY_USAGE") {
        Ok(_) => format!(" --dump-memory-usage"),
        Err(_) => "".to_string()
    };

    // Note we don't cd, so that relative paths for the path specified by the user on the remote
    // will be correct (relative to their ssh default dir, e.g. home dir)
    let doer_args = format!("--doer {} {} {}", log_arg, port_arg, memory_dump_arg);
    // Try launching using both Unix and Windows paths, as we don't know what the remote system is
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
    let mut handshake_start_timer = Some(start_timer("Waiting for handshake start"));

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
    let mut _handshaking_timer = None;
    loop {
        match receiver.recv() {
            Ok((stream_type, OutputReaderThreadMsg::Line(l))) => {
                // Show ssh output to the user, as this might be useful/necessary
                info!("ssh {}: {}", stream_type, l);
                // Check for both the Linux (bash) and Windows (cmd) errors
                if l.contains("No such file or directory") ||
                    l.contains("The system cannot find the path specified") ||
                    l.contains("is not recognized as an internal or external command") {
                    debug!("rjrssync not present on remote computer");
                    // Note the stdin of the ssh will be dropped and this will tidy everything up nicely
                    return SshDoerLaunchResult::NotPresentOnRemote;
                }
            }
            Ok((stream_type, OutputReaderThreadMsg::HandshakeStarted(line))) => {
                handshake_start_timer.take();
                _handshaking_timer = Some(start_timer("Handshaking"));
                debug!("Handshake started on {}: {}", stream_type, line);

                let remote_version = line.split_at(HANDSHAKE_STARTED_MSG.len()).1;
                if remote_version != VERSION.to_string() {
                    debug!(
                        "Remote server has incompatible version ({} vs local version {})",
                        remote_version, VERSION
                    );
                    // Note the stdin of the ssh will be dropped and this will tidy everything up nicely
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
            Ok((stream_type, OutputReaderThreadMsg::HandshakeCompleted(line, s))) => {
                debug!("Handshake completed on {}: {}", stream_type, line);
                match s {
                    OutputReaderStream::Stdout(b) => handshook_data.stdout = Some(b),
                    OutputReaderStream::Stderr(b) => handshook_data.stderr = Some(b),
                }

                let actual_port : u16 = match line.split_at(HANDSHAKE_COMPLETED_MSG.len()).1.parse() {
                    Ok(p) => p,
                    Err(e) => {
                        error!("Failed to parse port number from line '{}': {e}", line);
                        return SshDoerLaunchResult::CommunicationError;
                    } 
                };

                // Need to wait for both stdout and stderr to pass the handshake
                if let HandshookStdoutAndStderr { stdout: Some(stdout), stderr: Some(stderr), secret_key: Some(secret_key) } = handshook_data {
                    return SshDoerLaunchResult::Success {
                        ssh_process,
                        stdin: ssh_stdin,
                        stdout,
                        stderr,
                        secret_key,
                        actual_port,
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

// This embeds the source code of the program into the executable, so it can be deployed remotely and built on other platforms.
// We only include the src/ folder using this crate, and the Cargo.toml/lock files are included separately.
// This is because the RustEmbed crate doesn't handle well including the whole repository and then filtering out the
// bits we don't want (e.g. target/, .git/), because it walks the entire directory structure before filtering, which means
// that it looks through all the .git and target folders first, which is slow and error prone (intermittent errors possibly
// due to files being deleted partway through the build).
#[derive(RustEmbed)]
#[folder = "src/"]
#[prefix = "src/"] // So that these files are placed in a src/ subfolder when extracted
#[exclude = "bin/*"] // No need to copy testing code
struct EmbeddedSrcFolder;

const EMBEDDED_CARGO_TOML : &'static str = include_str!("../Cargo.toml");
const EMBEDDED_CARGO_LOCK : &[u8] = include_bytes!("../Cargo.lock");

/// Deploys the source code of rjrssync to the given remote computer and builds it, ready to be executed.
fn deploy_to_remote(remote_hostname: &str, remote_user: &str, reason: &str, needs_deploy_behaviour: NeedsDeployBehaviour) -> Result<(), ()> {
    // We're about to show a bunch of output from scp/ssh, so this log message may as well be the same severity,
    // so the user knows what's happening.
    info!("Deploying onto '{}'", &remote_hostname); 
    profile_this!();

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
    // Add cargo.lock/toml to the list of files we're about to extract.
    // These aren't included in the src/ folder (see EmbeddedSrcFolder comments for reason)
    let mut all_files_iter : Vec<(Cow<'static, str>, Cow<[u8]>)> = EmbeddedSrcFolder::iter().map(
        |p| (p.clone(), EmbeddedSrcFolder::get(&p).unwrap().data)
    ).collect();
    // Cargo.toml has some special processing to remove lines that aren't relevant for remotely deployed
    // copies
    let mut processed_cargo_toml: Vec<u8> = vec![];
    let mut in_non_remote_block = false;
    for line in EMBEDDED_CARGO_TOML.lines() {
        match line {
            "#if NonRemote" => in_non_remote_block = true,
            "#end" => in_non_remote_block = false,
            l => if !in_non_remote_block {
                processed_cargo_toml.extend_from_slice(l.as_bytes());
                processed_cargo_toml.push(b'\n');
            }
        }
    }
    all_files_iter.push((Cow::from("Cargo.toml"), Cow::from(processed_cargo_toml)));
    all_files_iter.push((Cow::from("Cargo.lock"), Cow::from(EMBEDDED_CARGO_LOCK)));
    for (path, contents) in all_files_iter {
        // Add an extra "rjrssync" folder with a fixed name (as opposed to the temp dir, whose name varies), to work around SCP weirdness below.
        let local_temp_path = local_temp_dir.path().join("rjrssync").join(&*path);

        if let Err(e) = std::fs::create_dir_all(local_temp_path.parent().unwrap()) {
            error!("Error creating folders for local temp file '{}': {}", local_temp_path.display(), e);
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

        if let Err(e) = f.write_all(&contents) {
            error!("Error writing local temp file: {}", e);
            return Err(());
        }
    }

    // Determine if the target system is Windows or Linux, so that we know where to copy our files to
    // We run a command that doesn't print out anything on both Windows and Linux, so we don't pollute the output
    // (we show all output from ssh, in case it contains prompts etc. that are useful/required for the user to see).
    // Note the \n to send a two-line command - it seems Windows ignores this, but Linux runs it.
    let remote_command = format!("echo >/dev/null # >nul & echo This is a Windows system\necho This is a Linux system");
    debug!("Running remote command: {}", remote_command);
    let os_test_output = match run_process_with_live_output("ssh", &[user_prefix.clone() + remote_hostname, remote_command.to_string()]) {
        Err(e) => {
            error!("Error running ssh: {}", e);
            return Err(());
        }
        Ok(output) if output.exit_status.success() => output.stdout,
        Ok(output) => {
            error!("Error checking remote OS. Exit status from ssh: {}", output.exit_status);
            return Err(());
        }
    };

    // We could check for "linux" in the string, but there are other Unix systems we might want to supoprt e.g. Mac,
    // so we fall back to Linux as a default
    let is_windows = os_test_output.contains("Windows");

    let (remote_temp, remote_rjrssync_folder) = if is_windows {
        (REMOTE_TEMP_WINDOWS, format!("{REMOTE_TEMP_WINDOWS}\\rjrssync"))
    } else {
        (REMOTE_TEMP_UNIX, format!("{REMOTE_TEMP_UNIX}/rjrssync"))
    };

    // Confirm that the user is happy for us to deploy and build the code on the target. This might take a while
    // and might download cargo packages, so the user should probably be aware.
    let msg = format!("rjrssync needs to be deployed onto remote target {remote_hostname} because {reason}. \
        This will require downloading some cargo packages and building the program on the remote target. \
        It will be deployed into the folder '{remote_rjrssync_folder}'");
    let resolved_behaviour = match needs_deploy_behaviour {
        NeedsDeployBehaviour::Prompt => {
            let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                None, 
                &[
                    ("Deploy", NeedsDeployBehaviour::Deploy),
                ], false, NeedsDeployBehaviour::Error);
            prompt_result.immediate_behaviour
        },
        x => x,
    };
    match resolved_behaviour {
        NeedsDeployBehaviour::Prompt => panic!("Should have been alredy resolved!"),
        NeedsDeployBehaviour::Error => { 
            error!("{msg}. Will not deploy. See --needs-deploy.");
            return Err(());
        }
        NeedsDeployBehaviour::Deploy => (), // Continue with deployment
    };

    // Deploy to remote target using scp
    // Note we need to deal with the case where the the remote folder doesn't exist, and the case where it does, so
    // we copy into /tmp (which should always exist), rather than directly to /tmp/rjrssync which may or may not
    let source_spec = local_temp_dir.path().join("rjrssync");
    let remote_spec = format!("{user_prefix}{remote_hostname}:{remote_temp}");
    debug!("Copying {} to {}", source_spec.display(), remote_spec);
    match run_process_with_live_output("scp", &[OsStr::new("-r"), source_spec.as_os_str(), OsStr::new(&remote_spec)]) {
        Err(e) => {
            error!("Error running scp: {}", e);
            return Err(());
        }
        Ok(s) if s.exit_status.success() => {
            // Good!
        }
        Ok(s) => {
            error!("Error copying source code. Exit status from scp: {}", s.exit_status);
            return Err(());
        }
    };

    // Build the program remotely (using the cargo on the remote system)
    // Note that we could merge this ssh command with the one to run the program once it's built (in launch_doer_via_ssh),
    // but this would make error reporting slightly more difficult as the command in launch_doer_via_ssh is more tricky as
    // we are parsing the stdout, but for the command here we can wait for it to finish easily.
    let cargo_command = format!("cargo build --release{}",
        if cfg!(feature="profiling") {
            " --features=profiling" // If this is a profiling build, then turn on profiling on the remote side too
        } else {
            ""
        });
    let remote_command = if is_windows {
        format!("cd /d {remote_rjrssync_folder} && {cargo_command}")
    } else {
        // Attempt to load .profile first, as cargo might not be on the PATH otherwise.
        // Still continue even if this fails, as it might not be available on this system.
        // Note that the previous attempt to do this (using $SHELL -lc '...') wasn't as good as it runs bashrc,
        // which is only meant for interative shells, but .profile is meant for scripts too.
        format!("source ~/.profile; cd {remote_rjrssync_folder} && {cargo_command}")
    };
    debug!("Running remote command: {}", remote_command);
    match run_process_with_live_output("ssh", &[user_prefix + remote_hostname, remote_command]) {
        Err(e) => {
            error!("Error running ssh: {}", e);
            return Err(());
        }
        Ok(s) if s.exit_status.success() => {
            // Good!
        }
        Ok(s) => {
            error!("Error building on remote. Exit status from ssh: {}", s.exit_status);
            return Err(());
        }
    };

    Ok(())
}

struct ProcessOutput {
    exit_status: std::process::ExitStatus,
    stdout: String,
    #[allow(unused)]
    stderr: String,
}

/// Runs a child processes and waits for it to exit. The stdout and stderr of the child process
/// are captured and forwarded to our own, with a prefix to indicate that they're from the child.
// We capture and then forward stdout and stderr, so the user can see what's happening and if there are any errors.
// Simply letting the child process inherit out stdout/stderr seems to cause problems with line endings getting messed
// up and losing output, and unwanted clearing of the screen.
// This is especially important if using --force-redeploy on a broken remote, as you don't see any errors from the initial
// attempt to connect either
fn run_process_with_live_output<I, S>(program: &str, args: I) -> Result<ProcessOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>
{
    let mut c = std::process::Command::new(program);
    let c = c.args(args);
    debug!("Running {:?} {:?}...", c.get_program(), c.get_args());

    let mut child = match c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("Error launching {}: {}", program, e)),
    };

    // unwrap is fine here, as the streams should always be available as we piped them all
    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    // Spawn a background thread for each stdout and stderr, to process messages we get from the child
    // and forward them to the main thread. This is easier than some kind of async IO stuff.
    let (sender1, receiver): (
        Sender<OutputReaderThreadMsg2>,
        Receiver<OutputReaderThreadMsg2>,
    ) = mpsc::channel();
    let sender2 = sender1.clone();
    let thread_builder = thread::Builder::new().name("child_stdout_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main2(child_stdout, OutputReaderStreamType::Stdout, sender1)
    }).unwrap();
    let thread_builder = thread::Builder::new().name("child_stderr_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main2(child_stderr, OutputReaderStreamType::Stderr, sender2)
    }).unwrap();

    let mut captured_stdout = String::new();
    let mut captured_stderr = String::new();
    loop {
        match receiver.recv() {
            Ok(OutputReaderThreadMsg2::Line(stream_type, l)) => {
                // Show output to the user, as this might be useful/necessary
                info!("{} {}: {}", program, stream_type, l);
                match stream_type {
                    OutputReaderStreamType::Stdout => captured_stdout += &(l + "\n"),
                    OutputReaderStreamType::Stderr => captured_stderr += &(l + "\n"),
                }
            }
            Ok(OutputReaderThreadMsg2::Error(stream_type, e)) => {
                return Err(format!("Error reading from {} {}: {}", program, stream_type, e));
            }
            Ok(OutputReaderThreadMsg2::StreamClosed(stream_type)) => {
                debug!("{} {} closed", program, stream_type);
            }
            Err(RecvError) => {
                // Both senders have been dropped, i.e. both background threads exited
                debug!("Both reader threads done, child process must have exited. Waiting for process.");
                // Wait for the process to exit, to get the exit code
                let result = match child.wait() {
                    Ok(r) => r,
                    Err(e) => return Err(format!("Error waiting for {}: {}", program, e)),
                };
                return Ok (ProcessOutput { exit_status: result, stdout: captured_stdout, stderr: captured_stderr });
            }
        }
    }
}

// Sent from the threads reading stdout and stderr of a child process back to the main thread.
enum OutputReaderThreadMsg2 {
    Line(OutputReaderStreamType, String),
    Error(OutputReaderStreamType, std::io::Error),
    StreamClosed(OutputReaderStreamType),
}

fn output_reader_thread_main2<S>(
    stream: S,
    stream_type: OutputReaderStreamType,
    sender: Sender<OutputReaderThreadMsg2>,
) -> Result<(), SendError<OutputReaderThreadMsg2>>
where S : std::io::Read {
    let mut reader = BufReader::new(stream);
    loop {
        let mut l: String = "".to_string();
        // Note we ignore errors on the sender here, as the other end should never have been dropped while it still cares
        // about our messages, but may have dropped if they abandon the child process, letting it finish itself.
        match reader.read_line(&mut l) {
            Err(e) => {
                sender.send(OutputReaderThreadMsg2::Error(stream_type, e))?;
                return Ok(());
            }
            Ok(0) => {
                // end of stream
                sender.send(OutputReaderThreadMsg2::StreamClosed(stream_type))?;
                return Ok(());
            }
            Ok(_) => {
                l.pop(); // Remove the trailing newline
                // A line of other content, for example a prompt or error from ssh itself
                sender.send(OutputReaderThreadMsg2::Line(stream_type, l))?;
            }
        }
    }
}

