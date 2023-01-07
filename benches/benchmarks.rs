use std::{time::{Instant, Duration}, path::{Path, PathBuf}, io::Write, process::Command, fmt::Display, collections::HashSet};

use ascii_table::AsciiTable;
use clap::Parser;
use fs_extra::dir::CopyOptions;
use indicatif::HumanBytes;

#[path = "../tests/test_utils.rs"]
#[allow(unused)]
mod test_utils;
#[path = "../tests/filesystem_node.rs"]
#[allow(unused)]
mod filesystem_node;

use test_utils::RemotePlatform;

/// Global state
struct Context {
    args: CliArgs,

    local_temp_dir: PathBuf,
    
    /// The set of Targets that we have already set up source folders for,
    /// so that we don't need to do it again.
    src_folders_setup_on_targets: HashSet<Target>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum Target {
    Local(PathBuf),
    Remote {
        platform: RemotePlatform,
        folder: String,
    }
}
impl Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Target::Local(p) => match p {
                x if x.to_string_lossy().starts_with(r"\\wsl$\") => r"\\wsl$\...",
                x if x.to_string_lossy().starts_with("/mnt/") => "/mnt/...",
                _ => if cfg!(windows) {
                    "Windows"
                } else {
                    "Linux"
                },
            },            
            Target::Remote { platform, .. } => if platform.is_windows {
                "Remote Windows"
            } else {
                "Remote Linux"
            }
        };
        write!(f, "{}", name)
    }
}

#[derive(clap::Parser, Clone)]
struct CliArgs {
    /// This is passed to us by "cargo bench", so we need to declare it, but we simply ignore it.
    #[arg(long)]
    bench: bool,

    /// Skips the setup of the files that will be copied in the tests (i.e. cloning stuff from GitHub)
    /// if the file already exist. This speeds up running the benchmark if the files are up to date, but
    /// if they're out of date, this might give misleading results.
    #[arg(long)]
    skip_setup: bool,
    /// Only runs tests for local filesystem destinations, skipping the remote ones.
    #[arg(long)]
    only_local: bool,
    /// Only runs tests for remote filesystem destinations, skipping the local ones.
    #[arg(long)]
    only_remote: bool,
    /// Only runs tests for the given programs (comma-separated list).
    #[arg(long, value_delimiter=',', default_value="rjrssync,rsync,scp,cp,xcopy,robocopy,apis")]
    programs: Vec<String>,
    /// Number of times to repeat each test, to get more accurate results in the presence of noise.
    #[arg(long, short, default_value_t=1)]
    num_samples: u32,
    /// Saves the benchmark results to a JSON file for further processing.
    #[arg(long)]
    json_output: Option<PathBuf>,
}

fn set_up_src_folders(target: &Target, context: &mut Context) {
    // If we've already set up source folders on this target as part of an earlier benchmark
    // then don't repeat it.
    if context.src_folders_setup_on_targets.contains(target) {
        println!("Skipping setup of {target} because it was already set up");
        return;
    }

    match target {
        Target::Local(local_path) => {
            let local_path = local_path.join("src");

            // If the user requested to skip the setup if possible (assuming it's up-to-date from last time),
            // then do so if the folder already exists
            if local_path.exists() && context.args.skip_setup {
                println!("Skipping setup of {target} because of --skip-setup. Beware this may be stale!");
                return;
            }

            set_up_src_folders_impl_local(&local_path);
        }
        Target::Remote { folder, platform, .. } => {
            // If the user requested to skip the setup if possible (assuming it's up-to-date from last time),
            // then do so if the folder already exists
            let remote_path = format!("{folder}{}src", platform.path_separator);
            let r = test_utils::run_process_with_live_output(Command::new("ssh").arg(&platform.user_and_host).arg(format!("stat {remote_path} || dir {remote_path}")));
            if r.exit_status.success() && context.args.skip_setup {
                println!("Skipping setup of {target} because of --skip-setup. Beware this may be stale!");
                return;
            }
        
            set_up_src_folders_impl_remote(platform, &remote_path, context);
        }
    };

    // Remember that we set up this target as a source, so we don't have to repeat
    // it for other benchmark configs
    context.src_folders_setup_on_targets.insert(target.clone());
}

fn set_up_src_folders_impl_local(src_folder: &Path) {
    // Delete any old stuff, so we start from a clean state each time
    if src_folder.exists() {
        std::fs::remove_dir_all(src_folder).expect("Failed to delete old src folder");
    }
    std::fs::create_dir_all(src_folder).expect("Failed to create src dir");

    // Representative example of a directory structure with varied depth, varied file size etc.
    // PowerToys, specific version (so doesn't change in future runs)
    let result = std::process::Command::new("git").arg("clone")
        .arg("--depth").arg("1")
        .arg("--branch").arg("v0.64.0")
        .arg("https://github.com/microsoft/PowerToys.git")
        .arg(src_folder.join("example-repo"))
        .status().expect("Failed to launch git");
    assert!(result.success());

    // Copy the repo then check out a slightly different version, so that only some files have changed
    std::fs::create_dir(src_folder.join("example-repo-slight-change")).expect("Failed to create folder");
    fs_extra::dir::copy(src_folder.join("example-repo"), src_folder.join("example-repo-slight-change"), &CopyOptions { content_only: true, ..Default::default() })
        .expect("Failed to copy dir");
    assert!(std::process::Command::new("git").arg("remote").arg("set-branches").arg("origin").arg("*")
        .current_dir(src_folder.join("example-repo-slight-change"))
        .status().expect("Failed to launch git").success());
    assert!(std::process::Command::new("git").arg("fetch").arg("--depth").arg("1").arg("origin").arg("v0.64.1")
        .current_dir(src_folder.join("example-repo-slight-change"))
        .status().expect("Failed to launch git").success());
    assert!(std::process::Command::new("git").arg("checkout").arg("FETCH_HEAD")
        .current_dir(src_folder.join("example-repo-slight-change"))
        .status().expect("Failed to launch git").success());

    // Delete the .git folders so these aren't synced too.
    std::fs::remove_dir_all(src_folder.join("example-repo/.git")).expect("Failed to delete .git");
    std::fs::remove_dir_all(src_folder.join("example-repo-slight-change/.git")).expect("Failed to delete .git");

    // Delete some particularly deeply-nested folders, which cause scp.exe on windows to crash with a
    // stack overflow. https://github.com/PowerShell/Win32-OpenSSH/issues/1897
    std::fs::remove_dir_all(src_folder.join("example-repo/src/modules/previewpane/MonacoPreviewHandler/monacoSRC/min/vs")).expect("Failed to delete nested folders");
    std::fs::remove_dir_all(src_folder.join("example-repo/src/settings-ui/Settings.UI.UnitTests/BackwardsCompatibility/TestFiles/")).expect("Failed to delete nested folders");
  
    std::fs::remove_dir_all(src_folder.join("example-repo-slight-change/src/modules/previewpane/MonacoPreviewHandler/monacoSRC/min/vs")).expect("Failed to delete nested folders");
    std::fs::remove_dir_all(src_folder.join("example-repo-slight-change/src/settings-ui/Settings.UI.UnitTests/BackwardsCompatibility/TestFiles/")).expect("Failed to delete nested folders");

    // Copy the repo again and make a more significant change (rename src folder), so that many files will need
    // deleting and copying.
    std::fs::create_dir(src_folder.join("example-repo-large-change")).expect("Failed to create folder");
    fs_extra::dir::copy(src_folder.join("example-repo-slight-change"), src_folder.join("example-repo-large-change"), &CopyOptions { content_only: true, ..Default::default() })
        .expect("Failed to copy dir");
    std::fs::rename(src_folder.join("example-repo-large-change/src"), src_folder.join("example-repo-large-change/src2")).expect("Failed to rename");

    // Single large file
    std::fs::create_dir_all(src_folder.join("large-file")).expect("Failed to create dir");
    let mut f = std::fs::File::create(src_folder.join("large-file/large.bin")).expect("Failed to create file");
    for i in 0..1000_000 as i32 {
        let buf = [(i % 256) as u8; 1024];
        f.write_all(&buf).expect("Failed to write to file");
    }
}

fn set_up_src_folders_impl_remote(platform: &RemotePlatform, remote_path: &str, context: &mut Context) {
    let user_and_host = &platform.user_and_host;

    // Delete any old stuff, so we start from a clean state each time.
    // Note that we also make sure that all parent folders are there, hence the weird deletion/recreation here
    test_utils::delete_and_recreate_remote_folder(remote_path, platform);
    test_utils::delete_remote_folder(remote_path, platform);
   
    // First set up the source folders locally, then we copy these to the remote source folder
    // Use the same local folder as for local targets, so we can avoid repeating the setup
    set_up_src_folders(&Target::Local(context.local_temp_dir.clone()), context);

    // Use the test framework's features to deploy it remotely, to avoid problems with scp
    // stack overflow on Windows https://github.com/PowerShell/Win32-OpenSSH/issues/1897
    println!("Deploying source data to {user_and_host}:{remote_path}...");
    let node = filesystem_node::load_filesystem_node_from_disk_local(&context.local_temp_dir.join("src"));
    filesystem_node::save_filesystem_node_to_disk_remote(&node.unwrap(), &format!("{user_and_host}:{remote_path}"));
}

fn main () {
    let args = CliArgs::parse();

    // Set up global state
    let local_temp_dir = std::env::temp_dir().join("rjrssync-benchmarks");
    let mut context = Context {
        args: args.clone(),
        local_temp_dir,
        src_folders_setup_on_targets: HashSet::new(),
    };

    // Create potential targets for use as source or dest

    let local_target = Target::Local(context.local_temp_dir.clone());

    let wsl_target = if cfg!(windows) {
        // Get the WSL distribution name, as we need this to find the path in \\wsl$
        let r = test_utils::run_process_with_live_output(Command::new("wsl").arg("--list").arg("--quiet"));
        if r.exit_status.success() {  // GitHub actions runs an older version of wsl, which doesn't support --list (nor the \\wsl$ path, so skip this)
            // wsl --list has some text encoding problems...
            println!("distro name = {:?}", r.stdout.as_bytes());
            let u16s = unsafe { r.stdout.as_bytes().split_at(r.stdout.len() - 2).0.align_to::<u16>().1 };
            let distro_name = String::from_utf16(u16s).unwrap().trim().to_string();
            println!("distro name = {:?}", distro_name);
            let wsl_tmp_path = PathBuf::from(format!("\\\\wsl$\\{distro_name}\\tmp"));
            println!("WSL tmp path = {:?}", wsl_tmp_path);
            // Older versions of WSL don't have this (e.g. on GitHub actions)
            if PathBuf::from(&wsl_tmp_path).is_dir() { 
                Some(Target::Local(wsl_tmp_path.join("rjrssync-benchmarks")))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let mnt_target = if cfg!(unix) {
        // Figure out the /mnt/... path to the windows temp dir
        // Note the full path to cmd.exe need to be used when running on GitHub actions
        let r = test_utils::run_process_with_live_output(Command::new("/mnt/c/Windows/system32/cmd.exe").arg("/c").arg("echo %TEMP%"));
        assert!(r.exit_status.success());
        let windows_temp = r.stdout.trim();
        // Convert to /mnt/ format using wslpath
        let r = test_utils::run_process_with_live_output(Command::new("wslpath").arg(windows_temp));
        assert!(r.exit_status.success());
        let mnt_temp = r.stdout.trim();
        // Use a sub-folder
        let folder = PathBuf::from(mnt_temp).join("rjrssync-benchmarks");
        Some(Target::Local(folder))
    } else {
        None
    };

    let remote_windows_target = Target::Remote { 
        platform: RemotePlatform::get_windows().clone(), 
        folder: RemotePlatform::get_windows().test_folder.clone() + "\\" + "rjrssync-benchmarks",
    };

    let remote_linux_target = Target::Remote { 
        platform: RemotePlatform::get_linux().clone(), 
        folder: RemotePlatform::get_linux().test_folder.clone() + "/" + "rjrssync-benchmarks",  
    };


    let mut results : AllResults = vec![];
    
    let mut run_targets = |src: &Target, dest: &Target| {
        results.push((
            TargetDesc { source: src.to_string(), dest: dest.to_string() }, 
            run_benchmarks_for_target(&args, src, dest, &mut context)
        ));
    };

    if !args.only_remote {
        run_targets(&local_target, &local_target);
    }
        
    if !args.only_remote && !args.only_local {
        if let Some(w) = wsl_target {
            run_targets(&local_target, &w);
        }
        if let Some(m) = mnt_target {
            run_targets(&local_target, &m);
        }
    }
    
    if !args.only_local {
        run_targets(&local_target, &remote_windows_target);
        run_targets(&local_target, &remote_linux_target);
        run_targets(&remote_linux_target, &remote_windows_target);
        run_targets(&remote_windows_target, &remote_linux_target);
    }

    println!();
    println!("Each cell shows <min> - <max> over {} sample(s) for: time | local memory (if available) | remote memory (if available)", args.num_samples);

    for (target_desc, target_results) in &results {
        println!();
        println!("{} -> {}", target_desc.source, target_desc.dest);

        let mut ascii_table = AsciiTable::default();
        ascii_table.set_max_width(300);
        ascii_table.column(0).set_header("Test case");
        let case = ["Everything copied", "Nothing copied", "Some copied", "Delete and copy", "Single large file"];
        let mut table_data: Vec<Vec<String>> = case.iter().map(|c| vec![c.to_string()]).collect();
        for (program_idx, (program_name, program_results)) in target_results.iter().enumerate() {
            ascii_table.column(program_idx + 1).set_header(*program_name);
            for (case_name, case_results) in program_results.iter() {
                // Make statistics to summarise into table cell
                let min_time = case_results.iter().map(|s| s.time).min();
                let max_time = case_results.iter().map(|s| s.time).max();
                let min_memory_local = case_results.iter().filter_map(|s| s.peak_memory.local).min();
                let max_memory_local = case_results.iter().filter_map(|s| s.peak_memory.local).max();
                let min_memory_remote = case_results.iter().filter_map(|s| s.peak_memory.remote).min();
                let max_memory_remote = case_results.iter().filter_map(|s| s.peak_memory.remote).max();
                let summary_text = if let (Some(min_time), Some(max_time)) = (min_time, max_time) {
                    let mut s = format!("{:7} - {:7}", format_duration(min_time), format_duration(max_time));
                    if let (Some(min_memory_local), Some(max_memory_local)) = (min_memory_local, max_memory_local) {
                        s += &format!("| {:10} - {:10}", HumanBytes(min_memory_local as u64).to_string(), HumanBytes(max_memory_local as u64).to_string());
                    }
                    if let (Some(min_memory_remote), Some(max_memory_remote)) = (min_memory_remote, max_memory_remote) {
                        s += &format!("| {:10} - {:10}", HumanBytes(min_memory_remote as u64).to_string(), HumanBytes(max_memory_remote as u64).to_string());
                    }
                    s
                } else {
                    format!("Skipped") 
                };

                table_data[case.iter().position(|x| x == case_name).unwrap()].push(summary_text);
            }
        }

        ascii_table.print(table_data);    
    }

    if let Some(json_filename) = args.json_output {
        println!("Saving benchmark results to {}...",json_filename.display());
        let mut json_file = std::fs::File::create(json_filename).expect("Failed to create JSON file");

        let json_value = json::JsonValue::Array(results.iter().map(|(target_desc, target_results)| {
            json::object! {
                source: target_desc.source.clone(),
                dest: target_desc.dest.clone(),
                results: target_results.iter().map(|(program_name, program_results)| {
                    json::object! {
                        program: *program_name,
                        results: program_results.iter().map(|(case_name, case_results)| {
                            json::object! {
                                case: *case_name,
                                results: case_results.iter().map(|sample| {
                                    json::object! {
                                        time: sample.time.as_millis() as u64,
                                        peak_memory_local: sample.peak_memory.local,
                                        peak_memory_remote: sample.peak_memory.remote,
                                    }
                                }).collect::<Vec<json::JsonValue>>(),
                            }
                        }).collect::<Vec<json::JsonValue>>(),
                    }
                }).collect::<Vec<json::JsonValue>>(),
            }
        }).collect::<Vec<json::JsonValue>>());

        write!(json_file, "{}", json_value.dump()).unwrap();
    }
}

struct TargetDesc {
    source: String,
    dest: String,
}
type AllResults = Vec<(TargetDesc, TargetResults)>;
type TargetResults = Vec<(&'static str, ProgramResults)>;
type ProgramResults = Vec<(&'static str, CaseResults)>;
type CaseResults = Vec<Sample>;

#[derive(Debug)]
struct Sample {
    time: Duration,
    peak_memory: PeakMemoryUsage,
}

fn run_benchmarks_for_target(args: &CliArgs, src_target: &Target, dest_target: &Target, context: &mut Context) -> TargetResults {
    println!("Src target: {:?}, dest target: {:?}", src_target, dest_target);

    // Set up test data on the source target if it's not there already
    set_up_src_folders(src_target, context);
    
    let mut results : TargetResults = vec![];

    let both_local = matches!(src_target, Target::Local(..)) && matches!(dest_target, Target::Local(..));
    let both_remote = matches!(src_target, Target::Remote{..}) && matches!(dest_target, Target::Remote{..});

    if args.programs.contains(&String::from("rjrssync")) {
        let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
        results.push(("rjrssync", run_benchmarks_using_program(args, rjrssync_path, &["$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }
   
    if args.programs.contains(&String::from("rsync")) && !matches!(dest_target, Target::Remote{ platform, .. } if platform.is_windows) { // rsync is Linux -> Linux only
        #[cfg(unix)]
        // Note trailing slash on the src is important for rsync!
        results.push(("rsync", run_benchmarks_using_program(args, "rsync", &["--archive", "--delete", "$SRC/", "$DEST"], src_target.clone(), dest_target.clone())));
    }

    if args.programs.contains(&String::from("scp")) && !both_remote { // scp has problems with two remotes (it hangs :O)
        results.push(("scp", run_benchmarks_using_program(args, "scp", &["-r", "-q", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }
   
    if args.programs.contains(&String::from("cp")) && both_local { // cp is local only
        #[cfg(unix)]
        results.push(("cp", run_benchmarks_using_program(args, "cp", &["-r", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }

    if args.programs.contains(&String::from("xcopy")) && both_local { // xcopy is local only
        #[cfg(windows)]
        results.push(("xcopy", run_benchmarks_using_program(args, "xcopy", &["/i", "/s", "/q", "/y", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }
   
    if args.programs.contains(&String::from("robocopy")) && both_local { // robocopy is local only
        #[cfg(windows)]
        results.push(("robocopy", run_benchmarks_using_program(args, "robocopy", &["/MIR", "/nfl", "/NJH", "/NJS", "/nc", "/ns", "/np", "/ndl", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }

    if args.programs.contains(&String::from("apis")) && both_local { // APIs are local only
        results.push(("apis", run_benchmarks(args, "APIs", |src, dest| -> PeakMemoryUsage {
            if !Path::new(&dest).exists() {
                std::fs::create_dir_all(&dest).expect("Failed to create dest folder");
            }
            fs_extra::dir::copy(src, dest, &CopyOptions { content_only: true, overwrite: true, ..Default::default() })
                .expect("Copy failed");
            PeakMemoryUsage { local: None, remote: None } // No measurement of peak memory usage as this is in-process
        }, src_target.clone(), dest_target.clone())));
    }

    results
}

#[derive(Debug)]
struct PeakMemoryUsage {
    local: Option<usize>,
    remote: Option<usize>,
}

fn run_benchmarks_using_program(cli_args: &CliArgs, program: &str, program_args: &[&str], 
    src_target: Target, dest_target: Target) -> ProgramResults {
    let id = Path::new(program).file_name().unwrap().to_string_lossy().to_string();
    let f = |src: String, dest: String| -> PeakMemoryUsage {
        let substitute = |p: &str| PathBuf::from(p.replace("$SRC", &src).replace("$DEST", &dest));
        let mut cmd = std::process::Command::new(program);
        let result = cmd
            .env("RJRSSYNC_TEST_DUMP_MEMORY_USAGE", "1") // To enable memory instrumentation when running rjrssync
            .args(program_args.iter().map(|a| substitute(a)));
        let hide_stdout = program == "scp"; // scp spams its stdout, and we can't turn this off, so we hide it.
        let result = test_utils::run_process_with_live_output_impl(result, hide_stdout, false, true);
        let success = if program == "robocopy" {
            // robocopy has different exit codes (0 isn't what we want)
            let code = result.exit_status.code().unwrap();
            // println!("code = {code}");
            code == 0 || code == 1 || code == 3
        } else {
            result.exit_status.success()
        };
        if !success {
            // Dump the stdout and stderr to help debug (we only show them on failure,
            // to keep the output concise and easy to follow benchmark progress)
            println!("Stdout:\n{}", result.stdout);
            println!("Stderr:\n{}", result.stderr);
            assert!("Test program failed! See above logs.".is_empty());
        }

        // Because reporting of memory usage is tricky (we can't do it well on Linux, nor for the remote
        // part of processes on any OS), we have our own instrumentation built into rjrssync. We use this 
        // when possible, otherwise use the memory usage from the process we launched (which only works on
        // Windows, and doesn't include remote usage)
        if program.contains("rjrssync") {
            // For rjrssync, parse the output to get the instrumented memory usage for both boss (local) and doer (remote, if relevant for this test)
            PeakMemoryUsage { 
                local: Some(result.stdout.lines().filter(|l| l.contains("Boss peak memory usage")).next().expect("Couldn't find line")
                    .rsplit_once(':').expect("Failed to parse line").1.trim()
                    .parse::<usize>().expect("Failed to parse number")),
                remote: match &dest_target {
                    Target::Local(_) => None,
                    Target::Remote { .. } => Some(result.stderr.lines().filter(|l| l.contains("Doer peak memory usage")).next().expect("Couldn't find line")
                        .rsplit_once(':').expect("Failed to parse line").1.trim()
                        .parse::<usize>().expect("Failed to parse number")),
                } 
            }
        } else {
            // For other programs, use the value reported by run_process_with_live_output_impl, which has some limitations
            PeakMemoryUsage { local: result.peak_memory_usage, remote: None }
        }
    };
    run_benchmarks(cli_args, &id, f, src_target, dest_target.clone())
}

fn run_benchmarks<F>(cli_args: &CliArgs, id: &str, sync_fn: F, 
    src_target: Target, dest_target: Target) -> ProgramResults
    where F : Fn(String, String) -> PeakMemoryUsage
{
    println!("  Subject: {id}");

    let mut everything_copied_results : CaseResults = vec![];
    let mut nothing_copied_results : CaseResults = vec![];
    let mut some_copied_results : CaseResults = vec![];
    let mut delete_and_copy_results : CaseResults = vec![];
    let mut single_large_file_results : CaseResults = vec![];

    for sample_idx in 0..cli_args.num_samples {
        println!("    Sample {sample_idx}/{}", cli_args.num_samples);

        let src_prefix = match &src_target {
            Target::Local(d) => {
                let d = d.join("src");
                d.to_string_lossy().to_string() + &std::path::MAIN_SEPARATOR.to_string()
            }
            Target::Remote { platform, folder } => {
                let folder = format!("{folder}{}src", platform.path_separator);
                platform.user_and_host.clone() + ":" + &folder + &platform.path_separator.to_string()
            }
        };

        // Delete any old dest folder from other subjects
        let dest_prefix = match &dest_target {
            Target::Local(d) => {
                let d = d.join("dest");
                if Path::new(&d).exists() {
                    std::fs::remove_dir_all(&d).expect("Failed to delete old dest folder");
                }
                std::fs::create_dir(&d).expect("Failed to create dest dir");
                d.to_string_lossy().to_string() + &std::path::MAIN_SEPARATOR.to_string()
            }
            Target::Remote { platform, folder } => {
                let folder = format!("{folder}{}dest", platform.path_separator);
                test_utils::delete_and_recreate_remote_folder(&folder, platform);
                platform.user_and_host.clone() + ":" + &folder + &platform.path_separator.to_string()
            }
        };

        let run = |src, dest| -> Sample {
            let start = Instant::now();
            let peak_memory = sync_fn(src, dest);
            let time = start.elapsed();
            Sample { time, peak_memory }    
        };

        // Sync example-repo to an empty folder, so this means everything is copied
        println!("      {id} example-repo everything copied...");
        let s = run(src_prefix.clone() + "example-repo", dest_prefix.clone() + "example-repo");
        println!("      {id} example-repo everything copied: {:?}", s);
        everything_copied_results.push(s);

        // Sync again - this should be a no-op, but still needs to check that everything is up-to-date
        // Programs like scp will always copy everything, so there's no point running this part of the test
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo nothing copied...");
            let s = run(src_prefix.clone() + "example-repo", dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo nothing copied: {:?}", s);
            nothing_copied_results.push(s);
        }

        // Make some small changes, e.g. check out a new version
        // Programs like scp will always copy everything, so there's no point running this part of the test
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo some copied...");
            let s = run(src_prefix.clone() + "example-repo-slight-change", dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo some copied: {:?}", s);
            some_copied_results.push(s);
        }

        // Make some large changes, (a big folder was renamed, so many things need deleting and then copying)
        // Programs like scp will always copy everything, so there's no point running this part of the test
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo delete and copy...");
            let s = run(src_prefix.clone() + "example-repo-large-change", dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo delete and copy: {:?}", s);
            delete_and_copy_results.push(s);
        }

        // Sync a single large file
        println!("      {id} example-repo single large file...");
        let s = run(src_prefix.clone() + "large-file", dest_prefix.clone() + "large-file");
        println!("      {id} example-repo single large file: {:?}", s);
        single_large_file_results.push(s);
    }

    vec![
        ("Everything copied", everything_copied_results),
        ("Nothing copied", nothing_copied_results),
        ("Some copied", some_copied_results),
        ("Delete and copy", delete_and_copy_results),
        ("Single large file", single_large_file_results),
    ]
}

fn format_duration(d: Duration) -> String {
    if d.as_secs_f32() < 1.0 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{:.2}s", d.as_secs_f32())
    }
}