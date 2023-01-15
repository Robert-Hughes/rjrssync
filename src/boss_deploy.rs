use exe::{VecPE, PE, ImageSectionHeader, Buffer, SectionCharacteristics};
use log::{debug, error, info};
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Path};
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
//TODO: include build.rs, otherwise can't build remotely!

#[derive(PartialEq, Eq)]
enum DeployMethod {
    Source,
    Binary
}

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

    // Determine if the target system is Windows or Linux, so that we know where to copy our files to
    // We run a command that doesn't print out anything on both Windows and Linux, so we don't pollute the output
    // (we show all output from ssh, in case it contains prompts etc. that are useful/required for the user to see).
    // Note the \n to send a two-line command - it seems Windows ignores this, but Linux runs it.
    //TODO: also detect architecture etc.
    //TODO: This isn't very robust, ideally we could share the logic that rustup-init.sh uses, but
    // that doesn't work on Windows anyway...
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

    // Temp staging folder for upload
    let staging_dir = match TempDir::new("rjrssync-deploy-staging") {
        Ok(x) => x,
        Err(e) => {
            error!("Error creating temp dir: {}", e);
            return Err(());
        }
    };
    // Add an extra "rjrssync" folder with a fixed name (as opposed to the temp dir, whose name varies), to work around SCP weirdness below.
    let staging_dir = staging_dir.path().join("rjrssync");

    // Check if we can deploy a binary, as this will be a lot faster than deploying and building source
    let target_triple = if is_windows { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }; //TODO: too simple!
    debug!("Target triple = {target_triple}");
    // Put the binary in the same place it would be if we built from source, for consistency
    let binary_folder = staging_dir.join("target").join("release");
    if let Err(e) = std::fs::create_dir_all(&binary_folder) {
        error!("Error creating temp dir {}: {}", binary_folder.display(), e);
        return Err(());
    }
    let output_binary = binary_folder.join("rjrssync.exe");
    let deploy_method = match create_binary_for_target(target_triple, &output_binary) {
        Ok(()) => {
            debug!("Deploying binary {}", output_binary.display());
            DeployMethod::Binary
        },
        Err(e) => {
            //TODO: for some errors, do we want to just stop rather than automatically falling back ("hiding" the error)?
            debug!("Can't deploy binary because {}. Deploying source instead.", e);

            // Copy our embedded source tree to the remote, so we can build it there.
            // (we can't simply copy the binary as it might not be compatible with the remote platform)
            // We use the user's existing ssh/scp tool so that their config/settings will be used for
            // logging in to the remote system (as opposed to using an ssh library called from our code).

            // Extract embedded source code to a temporary local folder
            debug!(
                "Extracting embedded source to local temp dir: {}",
                staging_dir.display()
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
                let local_temp_path = staging_dir.join(&*path);

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

            DeployMethod::Source
        }
    };
    
    let (remote_temp, remote_rjrssync_folder) = if is_windows {
        (REMOTE_TEMP_WINDOWS, format!("{REMOTE_TEMP_WINDOWS}\\rjrssync"))
    } else {
        (REMOTE_TEMP_UNIX, format!("{REMOTE_TEMP_UNIX}/rjrssync"))
    };

    //TODO: this message needs updating, if we are deploying a binary
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
    let source_spec = staging_dir;
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

    if deploy_method == DeployMethod::Source {
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
    }

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

/// Attempts to create an rjrssync binary that can be deployed to a target platform.
/// 
/// Binary embedding for quick deployment to new remotes.
/// This is quite confusing because of the recursive resource embedding.
/// We need to 'break the chain' to avoid a binary that contains itself (and thus is impossible).
///
/// Within our embedded resources, we have a set of binaries for each supported
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
/// Big_p = Lite_p + Embed(Lite_0, Lite_1, ..., Lite_n)
/// 
/// If the target platform is the same as the current one though, we can skip most of this and simply
/// copy ourselves directly - no need to recreate what we already have.
fn create_binary_for_target(target_triple: &str, output_binary: &Path) -> Result<(), String> {
    let current_exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Err(format!("Unable to get path to current exe: {e}"))
    };

    // If the target is simply the same as what we are already running on, we can use our current
    // binary - no need to recreate what we already have.
    // Note that the env var TARGET is set (forwarded) by us in build.rs
    if target_triple == env!("TARGET") {
        if let Err(e) = std::fs::copy(&current_exe, output_binary) {
            return Err(format!("Unable to copy current exe {} to {}: {e}", current_exe.display(), output_binary.display()))
        }
        return Ok(());
    }

    //TODO: proper error handling - fallback to source deploy?

    // https://0xrick.github.io/win-internals/pe5/
    // https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#the-rsrc-section
    // https://docs.rs/exe/latest/exe/index.html

    // println!("EMBEDDED_DATA_TEST = {:?}", EMBEDDED_DATA_TEST);

    // Find the embedded lite binary for the target platform
    let current_image = VecPE::from_disk_file(current_exe).expect("Failed to parse EXE");
    let embedded_binary_section = current_image.get_section_by_name(".rsrc1").expect("Missing section");
    let embedded_binary_data = embedded_binary_section.read(&current_image).expect("Failed to read data").clone();
 //  println!("Embedded binary data {} {} {} {}", embedded_binary_data[0], embedded_binary_data[1], embedded_binary_data[3], embedded_binary_data[4]);
   
    
    // Create a new PE image for it
    let mut new_image = VecPE::from_disk_data(embedded_binary_data);

    // Extend it with the embedded lite binaries for all platforms, to turn it into a big binary
    let mut new_section = ImageSectionHeader::default();
    new_section.set_name(Some(".rsrc1")); // The special section name we use - needs to match other places TODO: use constant!
    let mut new_section = new_image.append_section(&new_section).expect("Failed to append section");
    new_section.size_of_raw_data = embedded_binary_data.len() as u32;
    new_section.characteristics = SectionCharacteristics::CNT_INITIALIZED_DATA;
    new_section.virtual_size = 0x1000; //TODO: this is needed for it to work, but not sure what it should be set to!

    new_image.append(embedded_binary_data);

    new_image.pad_to_alignment().expect("Failed to pad");
    new_image.fix_image_size().expect("Failed to fix_image_size");

    new_image.save("T:\\Temp\\rjrssync-inception\\rjrssync.exe").expect("Failed to save!");
    
  //  let src_exe = "D:\\Programming\\Utilities\\rjrssync\\target\\debug\\rjrssync.exe";
  //  let mut image = VecPE::from_disk_file(src_exe).unwrap();
    
 //   println!("Section table = {:?}", image.get_section_table());
  //  println!("Has resources? = {}", image.has_data_directory(exe::ImageDirectoryEntry::Resource));
 //   println!("Sectoin rsrc? = {:?}", image.get_section_by_name(".rsrc"));
 //   println!("Sectoin text? = {:?}", image.get_section_by_name(".text"));
 //   println!("Sectoin example_section_rob? = {:?}", image.get_section_by_name(".rsrc1"));
    //image.add_section(section)
 //   let resources = ResourceDirectoryMut::parse(&mut image).unwrap();

    // Looks like we need to add a new section called ".rsrc"
    // Maybe also need to fill in the data directory entry for Resource, to point to that section.
    // Not sure though, as it looks like this might be for things that need loading into memory, and resources
    // probably shouldn't be always loaded into memory, so maybe we skip this?

    //TODO: add test that remotely deployed binary can then itself also remotely deploy (all binaries
    // are equal, no lite binaries every actually exist on disk)

    // Note that we do need to include the lite binary for the native build, as this will be needed if 
    // the big binary is used to produce a new big binary for a different platform - that new big binary will 
    // need to have the lite binary for the native platform.
    // Technically we could get this by downgrading the big binary to a lite binary before embedding it
    // (by deleting the appended section), but this would be more complicated.

    Err("wssup".to_string())
}

// For creating the initial big binary from Cargo, which needs to include all the embedded lite 
// binaries.
#[cfg(feature="progenitor")]
include!(concat!(env!("OUT_DIR"), "/embedded_binaries.rs"));


// // Put it in a section that won't be optimised out (special name for MSVC, for resources,
// // even though we are not actually using resources!)
// #[cfg(feature="progenitor")]
// #[link_section = ".rsrc1"] 
// // We don't actually use this symbol anywhere - it's only used to get the embedded data into
// // the big binary during the cargo build. When we need this data, we read it directly from the 
// // exe, because this symbol won't be available in the lite binary build (for deploying from an already-deployed
// // binary)
// #[used]
// //static EMBEDDED_DATA_TEST: [u8;16] = [ 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15];
// static EMBEDDED_DATA_TEST: [u8; 9010688] = *include_bytes!(concat!(env!("OUT_DIR"), "/lite/debug/rjrssync.exe"));
// //static EMBEDDED_DATA_TEST2: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lite/debug/rjrssync.exe"));


//TODO: tests for both source deployment and binary deployment
//TODO: source deployment will now be even slower, as the remote will need to build all of the embedded
// lite binaries for different targets!
//TODO: deploying a big binary to "less powerful"/slower targets may be bad because it will take
// ages to copy the big binary there, and the benefits of having a fully-functional rjrssync.exe on
// there may be minimal. Perhaps we do want the option(?) of deploying only a lite binary?
// That might make a lot of this work redundant, as we would no longer need to generate new big binaries
// on-demand, so wouldn't need to do all this section stuff.
// Perhaps instead we focus on making the binary smaller, which would be good anyway?
// One option could be to compress the embedded lite binaries.