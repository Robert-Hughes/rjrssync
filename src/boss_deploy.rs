use log::{debug, error, info};
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::{
    fmt::{self, Display},
    io::{BufRead, BufReader, Write},
    process::{Stdio},
    sync::mpsc::{RecvError, SendError},
};
use tempdir::TempDir;

use crate::*;

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
pub fn deploy_to_remote(remote_hostname: &str, remote_user: &str, reason: &str, needs_deploy_behaviour: NeedsDeployBehaviour) -> Result<(), ()> {
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

