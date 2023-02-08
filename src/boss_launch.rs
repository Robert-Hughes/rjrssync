use aes_gcm::aead::{OsRng};
use aes_gcm::{Aes128Gcm, KeyInit, Key};
use base64::Engine;
use indicatif::ProgressBar;
use log::{debug, error, info, log, trace};
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

use crate::*;
use crate::boss_deploy::deploy_to_remote;
use crate::boss_doer_interface::{Response, Command, HANDSHAKE_STARTED_MSG, HANDSHAKE_COMPLETED_MSG, VERSION};
use crate::encrypted_comms::AsyncEncryptedComms;

pub const REMOTE_TEMP_UNIX: &str = "/var/tmp"; // Use /var/tmp rather than /tmp so it doesn't get wiped on reboot (and thus requiring a re-deploy)
pub const REMOTE_TEMP_WINDOWS: &str = r"%TEMP%";

/// Rough maximum amount of memory we allow to be buffered in our cross-thread communication channels
/// between boss and doer. If this is set too high (or we didn't set a limit at all), then we would
/// buffer unlimited amounts of data in the case that one side of the transfer is faster than the
/// other and this would take up too much memory. If set too small, then we won't buffer enough
/// and this could lead to reduced performance.
pub const BOSS_DOER_CHANNEL_MEMORY_CAPACITY : usize = 100*1024*1024;

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
    deploy_behaviour: DeployBehaviour,
    progress_bar: &ProgressBar,
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
    let attempt_deploy = if deploy_behaviour == DeployBehaviour::Force {
        Some(format!("--deploy=force was set"))
    }
    else {
        match launch_doer_via_ssh(remote_hostname, remote_user, remote_port_for_comms, progress_bar) {
            SshDoerLaunchResult::FailedToRunSsh => {
                None // No point trying again. launch_doer_via_ssh will have logged the error already.
            }
            SshDoerLaunchResult::NotPresentOnRemote => {
                Some(format!("rjrssync is not present on the remote target")) // Attempt to deploy
            }
            SshDoerLaunchResult::HandshakeIncompatibleVersion { expected, actual } => {
                Some(format!("the rjrssync version present on the remote target ({actual}) is not compatible with this version ({expected})")) // Will attempt to deploy
            }
            SshDoerLaunchResult::ExitedUnexpectedly | SshDoerLaunchResult::CommunicationError => {
                None // No point trying again. launch_doer_via_ssh will have logged the error already.
            }
            SshDoerLaunchResult::Success { ssh_process, stdin, stdout, stderr, secret_key, actual_port } =>
                match connect_to_remote_doer(remote_hostname, debug_name, ssh_process, stdin, stdout, stderr, secret_key, actual_port) {
                    Ok(c) => return Some(c),
                    Err(e) => {
                        error!("Failed to connect to remote: {e}");
                        return None;
                    }
                }
        }
    };

    let deploy_reason = attempt_deploy?; // Stop here if decided not to deploy (error that we don't think deploying will help with)

    // New version is needed
    if let Err(e) = deploy_to_remote(remote_hostname, remote_user, &deploy_reason, deploy_behaviour, progress_bar) {
        error!("Failed to deploy to remote: {e}");
        return None;
    }

    debug!("Successfully deployed, attempting to run again");

    // Check again
    match launch_doer_via_ssh(remote_hostname, remote_user, remote_port_for_comms, progress_bar) {
        SshDoerLaunchResult::FailedToRunSsh |
        SshDoerLaunchResult::NotPresentOnRemote |
        SshDoerLaunchResult::HandshakeIncompatibleVersion { .. } |
        SshDoerLaunchResult::ExitedUnexpectedly |
        SshDoerLaunchResult::CommunicationError => {
            // Failed to launch even after deployment. launch_doer_via_ssh will have logged the error already.
            error!("Failed to launch, even after deployment");
            return None;
        }
        SshDoerLaunchResult::Success { ssh_process, stdin, stdout, stderr, secret_key, actual_port } =>
            match connect_to_remote_doer(remote_hostname, debug_name, ssh_process, stdin, stdout, stderr, secret_key, actual_port) {
                Ok(c) => return Some(c),
                Err(e) => {
                    error!("Failed to connect to remote: {e}");
                    return None;
                }
            }
    };
}

fn connect_to_remote_doer(
    remote_hostname: &str,
    debug_name: String,
    ssh_process: std::process::Child,
    stdin: LineWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
    secret_key: Key<Aes128Gcm>,
    actual_port: u16
) -> Result<Comms, String> {
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
            Err(e) => return Err(format!("Failed to connect to network address {:?}: {}", addr, e)),
        }
    };

    let debug_comms_name = "Remote ".to_string() + &debug_name;
    return Ok(Comms::Remote {
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
    HandshakeIncompatibleVersion {
        expected: String,
        actual: String,
    },
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
fn launch_doer_via_ssh(remote_hostname: &str, remote_user: &str,
    remote_port_for_comms: Option<u16>, progress_bar: &ProgressBar,
) -> SshDoerLaunchResult
{
    profile_this!();

    // It's important to set this message each time we enter this function,
    // as we might have just finished deploying and need to overwrite that message with this one.
    progress_bar.set_message("Connecting...");

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
    let windows_command = format!("{}\\rjrssync\\rjrssync.exe {}", REMOTE_TEMP_WINDOWS, doer_args);
    let unix_command = format!("{}/rjrssync/rjrssync {}", REMOTE_TEMP_UNIX, doer_args);
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
                    return SshDoerLaunchResult::HandshakeIncompatibleVersion {
                        expected: VERSION.to_string(), actual: remote_version.to_string() };
                }

                // Generate and send a secret key, so that we can authenticate/encrypt the network connection
                // Only do this once (when stdout has passed the version check, not on stderr too)
                if stream_type == OutputReaderStreamType::Stdout {
                    debug!("Sending secret key");
                    // Note that we generate a new key for each doer, otherwise the nonces would be re-used with the same key
                    let key = Aes128Gcm::generate_key(&mut OsRng);
                    let mut msg = base64::engine::general_purpose::STANDARD.encode(key).as_bytes().to_vec();
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
