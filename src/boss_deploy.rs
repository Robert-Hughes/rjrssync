use indicatif::{HumanBytes, ProgressBar};
use log::{debug, info};
use std::ffi::OsStr;
use std::path::{Path};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::{
    fmt::{self, Display},
    io::{BufRead, BufReader},
    process::{Stdio},
    sync::mpsc::{RecvError, SendError},
};
use tempdir::TempDir;
use crate::*;
use embedded_binaries::EmbeddedBinaries;

/// Deploys a pre-built binary of rjrssync to the given remote computer, ready to be executed.
pub fn deploy_to_remote(remote_hostname: &str, remote_user: &str, reason: &str,
    deploy_behaviour: DeployBehaviour, progress_bar: &ProgressBar)
-> Result<(), String>
{
    profile_this!();

    // We're about to (potentially) some output from scp/ssh, so this log message may as well be the same severity,
    // so the user knows what's happening. We log a message as well as changing the progress bar message,
    // so that any messages from ssh that follow can be easily attributed to the deployment.
    info!("Deploying onto '{}'...", &remote_hostname);
    progress_bar.set_message("Deploying...");

    let user_prefix = if remote_user.is_empty() {
        "".to_string()
    } else {
        remote_user.to_string() + "@"
    };

    // Determine if the target system is Windows or Linux, so that we know where to copy our files to,
    // and which pre-built binary to deploy (we can't simply copy the current binary as it might not be compatible with the remote platform)
    // We run a command that prints something reasonably friendly, so we don't pollute the output
    // (we show all output from ssh, in case it contains prompts etc. that are useful/required for the user to see).
    // Note the \n to send a two-line command - it seems Windows ignores this, but Linux runs it.
    // We use the user's existing ssh/scp tool so that their config/settings will be used for
    // logging in to the remote system (as opposed to using an ssh library called from our code).
    let remote_command = format!("echo >/dev/null # >nul & echo Remote system is Windows %PROCESSOR_ARCHITECTURE%\necho Remote system is `uname -a`");
    debug!("Running remote command: {}", remote_command);
    let os_test_output = match run_process_with_live_output("ssh", &[user_prefix.clone() + remote_hostname, remote_command.to_string()]) {
        Err(e) => return Err(format!("Error running ssh: {}", e)),
        Ok(output) if output.exit_status.success() => output.stdout,
        Ok(output) => return Err(format!("Error checking remote OS. Exit status from ssh: {}", output.exit_status)),
    };

    // We could check for "linux" in the string, but there are other Unix systems we might want to support too e.g. Mac,
    // so we fall back to Linux as a default
    let is_windows = os_test_output.contains("Windows");
    let binary_extension = if is_windows { "exe" } else { "" };

    // Create temporary staging folder for upload
    let staging_dir = match TempDir::new("rjrssync-deploy-staging") {
        Ok(x) => x,
        Err(e) => return Err(format!("Error creating temp dir: {}", e)),
    };
    // Add an extra "rjrssync" folder with a fixed name (as opposed to the temp dir, whose name varies),
    // to work around SCP weirdness below.
    let staging_dir = staging_dir.path().join("rjrssync");
    if let Err(e) = std::fs::create_dir_all(&staging_dir) {
        return Err(format!("Error creating staging dir {}: {}", staging_dir.display(), e));
    }

    let binary_filename = staging_dir.join("rjrssync").with_extension(binary_extension);

    // Generate a big binary for this platform into the staging folder, if we can.
    let binary_size = match create_binary_for_target(&os_test_output, &binary_filename) {
        Ok(s) => s,
        Err(e) => return Err(format!("Error generating binary to deploy: {}", e)),
    };

    let (remote_temp, remote_rjrssync_folder) = if is_windows {
        (REMOTE_TEMP_WINDOWS, format!("{REMOTE_TEMP_WINDOWS}\\rjrssync"))
    } else {
        (REMOTE_TEMP_UNIX, format!("{REMOTE_TEMP_UNIX}/rjrssync"))
    };

    // Confirm that the user is happy for us to deploy the binary to the target
    // (we're copying something onto the device that wasn't explicitly requested,
    // so the user should probably be aware).
    let msg = format!("rjrssync needs to be deployed onto remote target {remote_hostname} because {reason}. \
        A pre-built {} binary will be uploaded into the folder '{remote_rjrssync_folder}'",
        HumanBytes(binary_size));
    let resolved_behaviour = match deploy_behaviour {
        DeployBehaviour::Prompt => {
            let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                Some(progress_bar),
                &[
                    ("Deploy", DeployBehaviour::Ok),
                ], false, DeployBehaviour::Error);
            prompt_result.immediate_behaviour
        },
        DeployBehaviour::Force => DeployBehaviour::Ok,
        x => x,
    };
    match resolved_behaviour {
        DeployBehaviour::Prompt | DeployBehaviour::Force => panic!("Should have been alredy resolved!"),
        DeployBehaviour::Error => return Err(format!("{msg}. Will not deploy. See --deploy.")),
        DeployBehaviour::Ok => (), // Continue with deployment
    };

    // Deploy to remote target using scp
    // Note we need to deal with the case where the the remote folder doesn't exist, and the case where it does, so
    // we copy into /tmp (which should always exist), rather than directly to /tmp/rjrssync which may or may not
    let source_spec = staging_dir;
    let remote_spec = format!("{user_prefix}{remote_hostname}:{remote_temp}");
    debug!("Copying {} to {}", source_spec.display(), remote_spec);
    match run_process_with_live_output("scp", &[OsStr::new("-r"), source_spec.as_os_str(), OsStr::new(&remote_spec)]) {
        Err(e) => return Err(format!("Error running scp: {}", e)),
        Ok(s) if s.exit_status.success() => {
            // Good!
        },
        Ok(s) => return Err(format!("Error copying pre-built binary. Exit status from scp: {}", s.exit_status)),
    };

    // Make sure the remote exe is executable (on Linux this is required)
    if !is_windows {
        // Note that we could merge this ssh command with the one to run the program once it's built (in launch_doer_via_ssh),
        // but this would make error reporting slightly more difficult as the command in launch_doer_via_ssh is more tricky as
        // we are parsing the stdout, but for the command here we can wait for it to finish easily.
        let remote_command = format!("cd {remote_rjrssync_folder} && chmod +x rjrssync");
        debug!("Running remote command: {}", remote_command);
        match run_process_with_live_output("ssh", &[user_prefix + remote_hostname, remote_command]) {
            Err(e) => return Err(format!("Error running ssh: {}", e)),
            Ok(s) if s.exit_status.success() => {
                // Good!
            }
            Ok(s) => return Err(format!("Error setting executable bit. Exit status from ssh: {}", s.exit_status)),
        };
    }

    // Log message here to delineate any ssh messages printed above
    info!("Deploy successful!");

    Ok(())
}

struct ProcessOutput {
    exit_status: std::process::ExitStatus,
    stdout: String,
    #[allow(unused)]
    stderr: String,
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

/// Runs a child processes and waits for it to exit. The stdout and stderr of the child process
/// are captured and forwarded to our own, with a prefix to indicate that they're from the child.
// We capture and then forward stdout and stderr, so the user can see what's happening and if there are any errors.
// Simply letting the child process inherit out stdout/stderr seems to cause problems with line endings getting messed
// up and losing output, and unwanted clearing of the screen.
// This is especially important if using --deploy=force on a broken remote, as you don't see any errors from the initial
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

/// Attempts to create an rjrssync binary that can be deployed to a target platform.
///
/// This is quite confusing because of the recursive resource embedding.
/// We need to 'break the chain' to avoid a binary that contains itself (and thus is impossible).
///
/// Within the running executable, we embed a set of binaries for each supported
/// target platform (e.g. Windows x86 and Linux x86). These binaries are "lite" binaries as
/// they do not have any embedded binaries themselves - just the rjrssync code. This is as opposed
/// to our currently executing binary, which is a "big" binary as it contains embedded binaries
/// for the target platforms. A "big" binary therefore contains a set of "lite" binaries.
///
/// When we want to deploy to a target platform, we could just extract and use the "lite" binary,
/// but then this binary wouldn't itself be able to deploy to other targets which might be annoying
/// and confusing, because you would have to keep track of which copies of rjrssync are lite and which
/// are big. Instead, we create a new big binary for the target platform, by embedding all the lite binaries
/// inside the lite binary for the target platform.
///
/// For any platform p, Big_p = Lite_p + Embed(Lite_0, Lite_1, ..., Lite_n)
///
/// If the target platform is the same as the current one though, we can skip most of this and simply
/// copy ourselves directly - no need to recreate what we already have. This means that even
/// a lite binary can be deployed to remote targets as long as they are the same platform.
fn create_binary_for_target(os_test_output: &str, output_binary_filename: &Path) -> Result<u64, String> {
    // The embedded binaries might have different target triples depending on how it was build,
    // e.g. -gnu vs -msvc suffixes, so we need to be somewhat flexible here.
    let compatible_target_triples = if os_test_output.contains("Windows") && os_test_output.contains("AMD64") {
        vec!["x86_64-pc-windows-msvc", "x86_64-pc-windows-gnu"]
    } else if os_test_output.contains("Linux") && os_test_output.contains("x86_64") {
        vec!["x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu"]
    } else if os_test_output.contains("Linux") && os_test_output.contains("aarch64") {
        vec!["aarch64-unknown-linux-musl", "aarch64-unknown-linux-gnu"]
    } else {
        return Err(format!("Unknown target platform: {os_test_output}"));
    };

    // If the target is simply the same as what we are already running on, we can use our current
    // binary - no need to recreate what we already have.
    // Note that the env var TARGET is set (forwarded) by us in build.rs
    if compatible_target_triples.contains(&env!("TARGET")) {
        debug!("Target platform is compatible with native platform - copying current executable");
        let current_exe = std::env::current_exe().map_err(|e| format!("Unable to get path to current exe: {e}"))?;
        let size = std::fs::copy(&current_exe, output_binary_filename).map_err(|e|
            format!("Unable to copy current exe {} to {}: {e}", current_exe.display(), output_binary_filename.display()))?;
        return Ok(size)
    }

    // Get the embedded binaries from the current executable, and find one which is compatible
    // with the target platform.
    let (embedded_binaries, embedded_binaries_data) = get_embedded_binaries()?;
    let target_platform_binary = match embedded_binaries.binaries.into_iter().find(
        |b| compatible_target_triples.contains(&b.target_triple.as_str()))
    {
        Some(b) => b,
        None => return Err(format!(
            "No embedded binary for any compatible target triple ({:?}). Run --list-embedded-binaries to check what is available.",
            compatible_target_triples)),
    };

    // Create new executable for the target platform, by upgrading the chosen lite binary
    // to a big binary.
    debug!("Found embedded binary ({}), extracting and upgrading it to a big binary", target_platform_binary.target_triple);
    let size = create_big_binary(&output_binary_filename, &target_platform_binary.target_triple, target_platform_binary.data, embedded_binaries_data)?;
    debug!("Created big binary at {} ({})", output_binary_filename.display(), HumanBytes(size));
    Ok(size)
}

pub fn get_embedded_binaries() -> Result<(EmbeddedBinaries, Vec<u8>), String> {
    // Raw data
    let embedded_binaries_data = get_embedded_binaries_data()?;

    // Deserialize into structured data (array of binaries for different targets)
    // This isn't very efficient - we're loading every single binary into memory,
    // and then only using one of them. But the binaries should be quite small, so this should be fine.
    let embedded_binaries : EmbeddedBinaries = bincode::deserialize(&embedded_binaries_data).
        map_err(|e| format!("Error deserializing embedded binaries: {e}"))?;
    Ok((embedded_binaries, embedded_binaries_data))
}

// For creating the initial big binary from Cargo, which needs to include all the embedded lite
// binaries.
#[cfg(feature="progenitor")]
include!(concat!(env!("OUT_DIR"), "/embedded_binaries.rs"));

/// Parses the currently running executable file to get the section containing the embedded binaries table.
fn get_embedded_binaries_data() -> Result<Vec<u8>, String> {
    // To make sure that progenitor builds actually include the embedded binaries data and it isn't optimised
    // out, make a proper reference to the data.
    // The reference we add here hopefully shouldn't cause any performance issues.
    // As an alternative, I managed to work around this on MSVC by putting it in a section called ".rsrc1",
    // but couldn't figure anything similar out for Linux.
    #[cfg(feature="progenitor")]
    unsafe { std::ptr::read_volatile(&EMBEDDED_BINARIES_DATA[0]); }

    let current_exe = std::env::current_exe().map_err(|e| format!("Unable to get path to current exe: {e}"))?;
    let exe_data = std::fs::read(current_exe).map_err(|e| format!("Error loading current exe: {e}"))?;
    #[cfg(windows)]
    let embedded_binaries_data = exe_utils::extract_section_from_pe(exe_data, embedded_binaries::SECTION_NAME)?;
    #[cfg(unix)]
    let embedded_binaries_data = exe_utils::extract_section_from_elf(exe_data, embedded_binaries::SECTION_NAME)?;

    Ok(embedded_binaries_data)
}

/// Takes a lite binary for a target, and augments it with the given embedded_binaries_data, turning
/// it into a big binary.
fn create_big_binary(output_binary_filename: &Path, target_triple: &str,
    target_platform_binary: Vec<u8>, embedded_binaries_data: Vec<u8>) -> Result<u64, String>
{
    let new_binary = match target_triple {
        x if x.contains("windows") => exe_utils::add_section_to_pe(target_platform_binary, embedded_binaries::SECTION_NAME, embedded_binaries_data)?,
        x if x.contains("linux") => exe_utils::add_section_to_elf(target_platform_binary, embedded_binaries::SECTION_NAME, embedded_binaries_data)?,
        _ => return Err("No executable generating code for this platform".to_string()),
    };

    let size = new_binary.len() as u64;
    std::fs::write(output_binary_filename, new_binary).map_err(|e| format!("Error saving big binary: {e}"))?;
    Ok(size)
}

