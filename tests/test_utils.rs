// This file contains test utilities which is used by both the usage_tests binary
// and the benchmarks binary.

use std::{process::{Stdio, Command}, sync::{mpsc::{Sender, Receiver, self, SendError}, Mutex, MutexGuard}, thread, fmt::{Display, self}, io::{BufReader, BufRead, stdout}};
use network_interface::NetworkInterface;
use network_interface::NetworkInterfaceConfig;
use network_interface::V4IfAddr;
use network_interface::Addr::V4;
use lazy_static::{lazy_static};
use rand::{thread_rng, distributions::DistString};

pub struct ProcessOutput {
    pub exit_status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub peak_memory_usage: Option<usize>,
}

/// Runs a child processes and waits for it to exit. The stdout and stderr of the child process
/// are captured and forwarded to our own.
/// Simply letting the child process inherit out stdout/stderr seems to cause problems with line endings getting messed
/// up and losing output, and unwanted clearing of the screen.
/// This is mostly a copy-paste of the same function from boss_launch.rs, but we don't have a good way to share the code
/// and this version is slightly different, more suitable for tests (e.g. simpler error checking, logging printed with println).
pub fn run_process_with_live_output(c: &mut std::process::Command) -> ProcessOutput {
    run_process_with_live_output_impl(c, false, false, false)
}

pub fn assert_process_with_live_output(c: &mut std::process::Command) {
    let r = run_process_with_live_output_impl(c, false, false, false);
    assert!(r.exit_status.success());
}

/// Runs a child processes and waits for it to exit. The stdout and stderr of the child process
/// are captured and forwarded to our own.
/// Simply letting the child process inherit out stdout/stderr seems to cause problems with line endings getting messed
/// up and losing output, and unwanted clearing of the screen.
/// This is mostly a copy-paste of the same function from boss_launch.rs, but we don't have a good way to share the code
/// and this version is slightly different, more suitable for tests (e.g. simpler error checking, logging printed with println).
pub fn run_process_with_live_output_impl(c: &mut std::process::Command,
    no_stdout: bool, no_stderr: bool, quiet: bool) -> ProcessOutput {
    if !quiet {
        println!("Running {:?} {:?}...", c.get_program(), c.get_args());
    }

    // Setting stdin to null seems to fix issues with running all the tests in parallel (cargo test),
    // where some ssh processes get stuck waiting for input (pressing Enter in the command prompt a few times
    // seems to unstick it). It only seems to happen when running in parallel though strangely.
    let mut child = c.stdin(Stdio::null());
    if no_stdout {
        child = child.stdout(Stdio::null())
    } else {
        child = child.stdout(Stdio::piped())
    }
    if no_stderr {
        child = child.stderr(Stdio::null())
    } else {
        child = child.stderr(Stdio::piped())
    }

    let mut child = child
        .spawn()
        .expect(&format!("Failed to launch child process {:?} {:?}", c.get_program(), c.get_args()));


    // Spawn a background thread for each stdout and stderr, to process messages we get from the child
    // and forward them to the main thread. This is easier than some kind of async IO stuff.
    let (sender1, receiver): (
        Sender<OutputReaderThreadMsg2>,
        Receiver<OutputReaderThreadMsg2>,
    ) = mpsc::channel();
    let sender2 = sender1.clone();

    if !no_stdout {
        let child_stdout = child.stdout.take().unwrap();
        let thread_builder = thread::Builder::new().name("child_stdout_reader".to_string());
        thread_builder.spawn(move || {
            output_reader_thread_main2(child_stdout, OutputReaderStreamType::Stdout, sender1)
        }).unwrap();
    } else {
        drop(sender1);
    }

    if !no_stderr {
        let thread_builder = thread::Builder::new().name("child_stderr_reader".to_string());
        let child_stderr = child.stderr.take().unwrap();
        thread_builder.spawn(move || {
            output_reader_thread_main2(child_stderr, OutputReaderStreamType::Stderr, sender2)
        }).unwrap();
    } else {
        drop(sender2);
    }

    let mut captured_stdout = String::new();
    let mut captured_stderr = String::new();
    loop {
        match receiver.recv() {
            Ok(OutputReaderThreadMsg2::Line(stream_type, l)) => {
                // Print output for test debugging. Note that we need to use println, not write directly to stdout, so that
                // cargo's testing framework captures the output correctly.
                match stream_type {
                    OutputReaderStreamType::Stdout => {
                        if !quiet {
                            println!("{}", l);
                        }
                        captured_stdout += &(l + "\n");
                    }
                    OutputReaderStreamType::Stderr => {
                        if !quiet {
                            eprintln!("{}", l);
                        }
                        captured_stderr += &(l + "\n");
                    }
                }
            }
            Ok(OutputReaderThreadMsg2::Error(stream_type, e)) => {
                panic!("Error reading from {}: {}", stream_type, e);
            }
            Ok(OutputReaderThreadMsg2::StreamClosed(stream_type)) => {
                if !quiet {
                  //  println!("Child process {} closed", stream_type);
                }
            }
            Err(_) => {
                // Both senders have been dropped, i.e. both background threads exited
                if !quiet {
                 //   println!("Both reader threads done, child process must have exited. Waiting for process.");
                }

                // Wait for the process to exit, to get the exit code
                let result = match child.wait() {
                    Ok(r) => r,
                    Err(e) => panic!("Error waiting for child process: {}", e),
                };
                if !quiet {
                    println!("Exit status: {:?}", result);
                }

                // Collect peak memory usage stats before we close the handle
                let peak_memory_usage = get_peak_memory_usage(&child);

                return ProcessOutput { exit_status: result, stdout: captured_stdout, stderr: captured_stderr, peak_memory_usage };
            }
        }
    }
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

fn get_peak_memory_usage(_process: &std::process::Child) -> Option<usize> {
    #[cfg(windows)]
    unsafe {
        use std::os::windows::prelude::{AsRawHandle};
        let mut counters : winapi::um::psapi::PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        let handle = _process.as_raw_handle();
        if winapi::um::psapi::GetProcessMemoryInfo(handle, &mut counters,
            std::mem::size_of::<winapi::um::psapi::PROCESS_MEMORY_COUNTERS>() as u32) == 0
        {
            panic!("Win32 API failed!");
        }
        // I think this only accounts for physical memory, not paged memory, but hopefully that's fine
        Some(counters.PeakWorkingSetSize)
    }
    #[cfg(unix)]
    // On Linux there doesn't seem to be a good way of implementing this, as the /proc/X/status
    // file doesn't contain memory usage during process shutdown: https://unix.stackexchange.com/questions/500212/memory-usage-info-in-proc-pid-status-missing-when-program-is-about-to-terminate
    // and we don't have a good way of querying it before this happens.
    None
}

// Some tests and benchmarks rely on accessing "remote" hosts to test
// remote deploying and syncing. Therefore they require the test environment
// to be set up (e.g. firewalls configured, remote hosts configured), and
// a Windows and Linux remote hostname are required.
// One way of achieving this is to use WSL.

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RemotePlatform {
    pub user_and_host: String,
    pub test_folder: String,
    pub path_separator: char,
    pub is_windows: bool,
    /// Path to the deployed rjrssync on the remote platform (e.g. %TEMP%\rjrssync\rjrssync.exe).
    /// This needs to be kept in sync with where rjrssync actually deploys itself.
    pub rjrssync_path: String,
}

pub struct RemotePlatforms {
    pub windows: RemotePlatform,
    pub linux: RemotePlatform,
}

impl RemotePlatforms {
    // Multiple tests can't use the same remote platform at the same time, as this can cause
    // issues like trying to deploy rjrssync whilst it's already running.
    // Therefore we lock access to the remote platforms in a mutex. It's important that we
    // lock *all* the remote platforms in the *same* mutex, to prevent deadlocks
    // (one test locks Windows then Linux, the other Linux then Windows!)
    pub fn lock() -> MutexGuard<'static, RemotePlatforms> {
        REMOTE_PLATFORMS.lock().expect("Failed to lock mutex")
    }
}

// Determine the remote config just once using lazy_static, as it might be a bit expensive
// to run every time we need these platforms, as it runs some ssh commands etc.
// Don't use these directly, use RemotePlatforms::get instead.
lazy_static! {
    pub static ref REMOTE_PLATFORMS: Mutex<RemotePlatforms> = Mutex::new(RemotePlatforms {
        windows: create_remote_windows_platform(),
        linux: create_remote_linux_platform(),
    });
}

/// Gets the remote host configuration to use for remote Windows tests.
/// This can come from environment variables specified by the user, or if not specified,
/// a default is returned assuming a WSL setup.
fn create_remote_windows_platform() -> RemotePlatform {
    let user_and_host = match std::env::var("RJRSSYNC_TEST_REMOTE_USER_AND_HOST_WINDOWS") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => {
            if cfg!(windows) {
                // We want to simply connect to the current OS, but using localhost or 127.0.0.1 won't
                // work if SSH on WSL is also listening on the same port, as that takes precedence.
                // Instead we need to find another IP to refer to the current OS.
                let host = NetworkInterface::show().expect("Error getting network interfaces").into_iter()
                    .filter_map(|i| i.addr.and_then(|a| if let V4(V4IfAddr { ip, .. }) = a { Some(ip.to_string()) } else { None }))
                    .filter(|a| a != "127.0.0.1").nth(0).expect("No appropriate network interfaces");

                // We specify the user explicitly so that we can connect to this remote platform
                // even from a different one (not running the local device). e.g. in the test_remote_launch_impl
                // test, where we connect to one remote platform from another.
                let output = std::process::Command::new("cmd.exe").arg("/c").arg("echo %USERNAME%").output().expect("Failed to query windows username");
                assert!(output.status.success());
                let username = String::from_utf8(output.stdout).expect("Unable to decode utf-8").trim().to_string();

                format!("{username}@{host}")
            } else if cfg!(unix) {
                // Figure out the IP address of the external host windows system from /etc/resolv.conf,
                // by looking for the line "nameserver XYZ.XYZ.XYZ.XYZ"
                let windows_ip = std::fs::read_to_string("/etc/resolv.conf").expect("Failed to read /etc/resolv.conf")
                    .lines().filter_map(|l| l.split("nameserver ").last()).last().expect("Couldn't find nameserver in /etc/resolv.conf").to_string();

                // Get windows username
                // Note the full path to cmd.exe need to be used when running on GitHub actions through the tmate console (cmd.exe is not enough)
                let output = std::process::Command::new("/mnt/c/Windows/system32/cmd.exe").arg("/c").arg("echo %USERNAME%").output().expect("Failed to query windows username");
                assert!(output.status.success());
                let username = String::from_utf8(output.stdout).expect("Unable to decode utf-8").trim().to_string();

                format!("{username}@{windows_ip}")
            } else {
                panic!("Not implemented for this OS" );
            }
        }
        _ => panic!("Unexpected error"),
    };
    println!("Windows remote user and host: {user_and_host}");

    let test_folder = match std::env::var("RJRSSYNC_TEST_REMOTE_TEST_FOLDER_WINDOWS") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => {
            // Figure out the remote temp dir, based on the remote environment variable %TEMP%
            // Use run_process_with_live_output to avoid messing up terminal line endings
            let output = run_process_with_live_output(std::process::Command::new("ssh").arg(&user_and_host).arg("echo %TEMP%\\rjrssync-tests"));
            assert!(output.exit_status.success());
            output.stdout.trim().to_string()
        }
        _ => panic!("Unexpected error"),
    };
    println!("Windows remote test folder: {test_folder}");

    // Confirm that we can connect to this remote host, to help debugging the test environment
    confirm_remote_test_environment(&user_and_host, &test_folder, "Windows");

    RemotePlatform { user_and_host, test_folder, path_separator: '\\', is_windows: true,
        rjrssync_path: "%TEMP%\\rjrssync\\rjrssync.exe".to_string() }
}

/// Gets the remote host configuration to use for remote Linux tests.
/// This can come from environment variables specified by the user, or if not specified,
/// a default is returned assuming a WSL setup.
fn create_remote_linux_platform() -> RemotePlatform {
    let user_and_host = match std::env::var("RJRSSYNC_TEST_REMOTE_USER_AND_HOST_LINUX") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => {
            if cfg!(windows) {
                // We want to connect to the WSL instance which we assume is running, which can be done
                // by simply using localhost or 127.0.0.1. If both WSL SSH and windows SSH are both listening,
                // then WSL takes precedence.
                // The username is more complicated, as the WSL username might differ from Windows username
                // Running wsl.exe messes up line endings while it is running, so this lock prevents it messing
                // up other tests running at the same time.
                let _lock = stdout().lock();
                let output = run_process_with_live_output(std::process::Command::new("wsl").arg("echo").arg("$USER"));
                assert!(output.exit_status.success());
                let username = output.stdout.trim().to_string();

                format!("{username}@127.0.0.1")
            } else if cfg!(unix) {
                // Simply connect to the current OS, with the current user
                let host = "127.0.0.1";

                // We specify the user explicitly so that we can connect to this remote platform
                // even from a different one (not running the local device). e.g. in the test_remote_launch_impl
                // test, where we connect to one remote platform from another.
                let output = run_process_with_live_output(std::process::Command::new("bash").arg("-c").arg("echo $USER"));
                assert!(output.exit_status.success());
                let username = output.stdout.trim().to_string();

                format!("{username}@{host}")
            } else {
                panic!("Not implemented for this OS" );
            }
        }
        _ => panic!("Unexpected error"),
    };
    println!("Linux remote user and host: {user_and_host}");

    let test_folder = match std::env::var("RJRSSYNC_TEST_REMOTE_TEST_FOLDER_LINUX") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => "/tmp/rjrssync-tests".to_string(),
        _ => panic!("Unexpected error"),
    };
    println!("Linux remote test folder: {test_folder}");

    // Confirm that we can connect to this remote host, to help debugging the test environment
    confirm_remote_test_environment(&user_and_host, &test_folder, "Linux");

    RemotePlatform { user_and_host, test_folder, path_separator: '/', is_windows: false,
        rjrssync_path: "/var/tmp/rjrssync/rjrssync".to_string() }
}

fn confirm_remote_test_environment(remote_user_and_host: &str, remote_folder: &str, expected_os: &str) {
    // Confirm that we can connect to this remote host, to help debugging the test environment
    // And make sure that the folder specified exists, otherwise we'll run into other issues later on
    let test_command = match expected_os {
        "Windows" => format!("echo Remote host is working && ver && cd {remote_folder}"),
        "Linux" => format!("echo Remote host is working && uname -a && cd {remote_folder}"),
        _ => panic!("Unexpected OS"),
    };

    println!("Checking connection to {} with ssh command '{}'", remote_user_and_host, test_command);
    // Use run_process_with_live_output to avoid messing up terminal line endings
    let output = run_process_with_live_output(std::process::Command::new("ssh").arg(remote_user_and_host).arg(test_command));
    println!("ssh exit code: {}", output.exit_status);
    println!("ssh stdout:");
    println!("{}", output.stdout);
    println!("ssh stderr:");
    println!("{}", output.stderr);

    assert!(output.exit_status.success());
    assert!(output.stdout.contains(expected_os));
}

pub struct RemoteTempDir<'a> {
    remote_platform: &'a RemotePlatform,
    pub folder: String,
}
impl<'a> Drop for RemoteTempDir<'a> {
    /// Delete when dropped to clean up after ourselves.
    fn drop(&mut self) {
        delete_remote_folder(&self.folder, self.remote_platform);
    }
}

/// Creates and returns the path to an empty temporary folder on the given remote platform.
/// We can't use TempDir or similar as this is for a remote platform, not the local one.
/// We need to use separate folders for each test so that each test is run in a clean environment.
/// Returns a struct wrapping the remote folder, so that it can be deleted when dropped to clean up
/// after ourselves
pub fn get_unique_remote_temp_folder(remote_platform: &RemotePlatform) -> RemoteTempDir {
    // For now we make a random number and hope that it's unique!
    let mut rng = thread_rng();
    let folder = format!("{}{}{}", remote_platform.test_folder, remote_platform.path_separator, &rand::distributions::Alphanumeric.sample_string(&mut rng, 8));

    // Create the folder
    assert_process_with_live_output(Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("mkdir {folder}")));

    RemoteTempDir { remote_platform, folder }
}

pub fn delete_remote_file(file: &str, remote_platform: &RemotePlatform) {
    if remote_platform.is_windows {
        // Use run_process_with_live_output to avoid messing up terminal line endings
        let _ = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("del {file}")), false, false, true);
        // This one can fail, if the file doesn't exist
    } else {
        let result = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("rm '{file}'")), false, false, true);
        assert!(result.exit_status.success());
    }
}

pub fn delete_remote_folder(folder: &str, remote_platform: &RemotePlatform) {
    if remote_platform.is_windows {
        // Use run_process_with_live_output to avoid messing up terminal line endings
        let _ = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("rmdir /Q /S {folder}")), false, false, true);
        // This one can fail, if the folder doesn't exist
    } else {
        let result = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("rm -rf '{folder}'")), false, false, true);
        assert!(result.exit_status.success());
    }
}

pub fn delete_and_recreate_remote_folder(folder: &str, remote_platform: &RemotePlatform) {
    if remote_platform.is_windows {
        // Use run_process_with_live_output to avoid messing up terminal line endings
        let _ = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("rmdir /Q /S {folder}")), false, false, true);
        // This one can fail, if the folder doesn't exist

        let result = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("mkdir {folder}")), false, false, true);
        assert!(result.exit_status.success());
    } else {
        let result = run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&remote_platform.user_and_host).arg(format!("rm -rf '{folder}' && mkdir -p '{folder}'")), false, false, true);
        assert!(result.exit_status.success());
    }
}