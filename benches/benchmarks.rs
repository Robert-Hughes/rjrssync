use std::{time::{Instant, Duration}, path::{Path, PathBuf}, io::Write, process::Command};

use ascii_table::AsciiTable;
use clap::Parser;
use fs_extra::dir::CopyOptions;
use indicatif::HumanBytes;

#[path = "../tests/test_utils.rs"]
mod test_utils;

use test_utils::get_unique_remote_temp_folder;
use test_utils::RemotePlatform;

#[derive(Debug, Clone)]
enum Target {
    Local(PathBuf),
    Remote {
        is_windows: bool,
        user_and_host: String,
        folder: String,
    }
}

#[derive(clap::Parser)]
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

fn set_up_src_folders(src_folder: &Path, skip_if_exists: bool) {
    if src_folder.exists() && skip_if_exists {
        println!("Skipping setup. Beware this may be stale!");
        return;
    }

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
    // stack overflow.
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

fn main () {
    let args = CliArgs::parse();

    // Create a temporary folder which we will run all our benchmarks in
    let temp_dir = std::env::temp_dir().join("rjrssync-benchmarks");
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    set_up_src_folders(&temp_dir.join("src"), args.skip_setup);
    
    let mut results : AllResults = vec![];
    
    let local_name = if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    };

    let src_target = Target::Local(temp_dir.clone());
    
    if !args.only_remote {
        results.push((TargetDesc { source: local_name, dest: local_name }, run_benchmarks_for_target(&args, src_target.clone(), Target::Local(temp_dir.join("dest")))));
    }
        
    if !args.only_remote && !args.only_local {
        #[cfg(windows)]
        {
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
                    results.push((TargetDesc { source: local_name, dest: r"\\wsl$\..." }, run_benchmarks_for_target(&args, src_target.clone(), Target::Local(wsl_tmp_path.join(r"rjrssync-benchmark-dest")))));
                }
            }
        }

        #[cfg(unix)]
        {
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
            let dest = PathBuf::from(mnt_temp).join("rjrssync-benchmarks").join("dest");
            results.push((TargetDesc { source: local_name, dest: "/mnt/..." }, run_benchmarks_for_target(&args, src_target.clone(), Target::Local(dest))));
        }
    }
    
    if !args.only_local {
        results.push((TargetDesc { source: local_name, dest: "Remote Windows" }, run_benchmarks_for_target(&args, 
            src_target.clone(), 
            Target::Remote { 
                is_windows: true, 
                user_and_host: RemotePlatform::get_windows().user_and_host.clone(), 
                folder: get_unique_remote_temp_folder(RemotePlatform::get_windows()) })));
        
        results.push((TargetDesc { source: local_name, dest: "Remote Linux" }, run_benchmarks_for_target(&args, 
            src_target.clone(), 
            Target::Remote { 
                is_windows: false, 
                user_and_host: RemotePlatform::get_linux().user_and_host.clone(), 
                folder: get_unique_remote_temp_folder(RemotePlatform::get_linux()) })));
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
                source: target_desc.source,
                dest: target_desc.dest,
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
    source: &'static str,
    dest: &'static str,
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

fn run_benchmarks_for_target(args: &CliArgs, src_target: Target, dest_target: Target) -> TargetResults {
    println!("Src target: {:?}, dest target: {:?}", src_target, dest_target);
    let mut results : TargetResults = vec![];

    if args.programs.contains(&String::from("rjrssync")) {
        let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
        results.push(("rjrssync", run_benchmarks_using_program(args, rjrssync_path, &["$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }
   
    if args.programs.contains(&String::from("rsync")) && !matches!(dest_target, Target::Remote{ is_windows, .. } if is_windows) { // rsync is Linux -> Linux only
        #[cfg(unix)]
        // Note trailing slash on the src is important for rsync!
        results.push(("rsync", run_benchmarks_using_program(args, "rsync", &["--archive", "--delete", "$SRC/", "$DEST"], src_target.clone(), dest_target.clone())));
    }

    if args.programs.contains(&String::from("scp")) {
        results.push(("scp", run_benchmarks_using_program(args, "scp", &["-r", "-q", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }
   
    if args.programs.contains(&String::from("cp")) && matches!(dest_target, Target::Local(..)) { // cp is local only
        #[cfg(unix)]
        results.push(("cp", run_benchmarks_using_program(args, "cp", &["-r", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }

    if args.programs.contains(&String::from("xcopy")) && matches!(dest_target, Target::Local(..)) { // xcopy is local only
        #[cfg(windows)]
        results.push(("xcopy", run_benchmarks_using_program(args, "xcopy", &["/i", "/s", "/q", "/y", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }
   
    if args.programs.contains(&String::from("robocopy")) && matches!(dest_target, Target::Local(..)) { // robocopy is local only
        #[cfg(windows)]
        results.push(("robocopy", run_benchmarks_using_program(args, "robocopy", &["/MIR", "/nfl", "/NJH", "/NJS", "/nc", "/ns", "/np", "/ndl", "$SRC", "$DEST"], src_target.clone(), dest_target.clone())));
    }

    if args.programs.contains(&String::from("apis")) && matches!(dest_target, Target::Local(..)) { // APIs are local only
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
                d.to_string_lossy().to_string() + &std::path::MAIN_SEPARATOR.to_string()
            }
            _ => unimplemented!()
        };

        // Delete any old dest folder from other subjects
        let dest_prefix = match &dest_target {
            Target::Local(d) => {
                if Path::new(&d).exists() {
                    std::fs::remove_dir_all(&d).expect("Failed to delete old dest folder");
                }
                std::fs::create_dir(&d).expect("Failed to create dest dir");
                d.to_string_lossy().to_string() + &std::path::MAIN_SEPARATOR.to_string()
            }
            Target::Remote { is_windows, user_and_host, folder } => {
                if *is_windows {
                    // Use run_process_with_live_output to avoid messing up terminal line endings
                    let _ = test_utils::run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&user_and_host).arg(format!("rmdir /Q /S {folder}")), false, false, true);
                    // This one can fail, if the folder doesn't exist

                    let result = test_utils::run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&user_and_host).arg(format!("mkdir {folder}")), false, false, true);
                    assert!(result.exit_status.success());
                } else {
                    let result = test_utils::run_process_with_live_output_impl(std::process::Command::new("ssh").arg(&user_and_host).arg(format!("rm -rf '{folder}' && mkdir -p '{folder}'")), false, false, true);
                    assert!(result.exit_status.success());
                }
                let remote_sep = if *is_windows { "\\" } else { "/" };
                user_and_host.clone() + ":" + &folder + remote_sep
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
        let s = run(Path::new(&src_prefix).join("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
        println!("      {id} example-repo everything copied: {:?}", s);
        everything_copied_results.push(s);

        // Sync again - this should be a no-op, but still needs to check that everything is up-to-date
        // Programs like scp will always copy everything, so there's no point running this part of the test
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo nothing copied...");
            let s = run(Path::new(&src_prefix).join("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo nothing copied: {:?}", s);
            nothing_copied_results.push(s);
        }

        // Make some small changes, e.g. check out a new version
        // Programs like scp will always copy everything, so there's no point running this part of the test
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo some copied...");
            let s = run(Path::new(&src_prefix).join("src").join("example-repo-slight-change").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo some copied: {:?}", s);
            some_copied_results.push(s);
        }

        // Make some large changes, (a big folder was renamed, so many things need deleting and then copying)
        // Programs like scp will always copy everything, so there's no point running this part of the test
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo delete and copy...");
            let s = run(Path::new(&src_prefix).join("src").join("example-repo-large-change").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo delete and copy: {:?}", s);
            delete_and_copy_results.push(s);
        }

        // Sync a single large file
        println!("      {id} example-repo single large file...");
        let s = run(Path::new(&src_prefix).join("src").join("large-file").to_string_lossy().to_string(), dest_prefix.clone() + "large-file");
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