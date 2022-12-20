use std::{time::{Instant, Duration}, path::{Path, PathBuf}, io::Write};

use ascii_table::AsciiTable;
use clap::Parser;
use fs_extra::dir::CopyOptions;
use indicatif::HumanBytes;

#[path = "../tests/test_utils.rs"]
mod test_utils;

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
}

fn set_up_src_folders(args: &CliArgs) {
    if Path::new("src").exists() && args.skip_setup {
        println!("Skipping setup. Beware this may be stale!");
        return;
    }

    // Delete any old stuff, so we start from a clean state each time
    if Path::new("src").exists() {
        std::fs::remove_dir_all("src").expect("Failed to delete old src folder");
    }
    std::fs::create_dir_all("src").expect("Failed to create src dir");

    // Representative example of a directory structure with varied depth, varied file size etc.
    // PowerToys, specific version (so doesn't change in future runs)
    let result = std::process::Command::new("git").arg("clone")
        .arg("--depth").arg("1")
        .arg("--branch").arg("v0.64.0")
        .arg("https://github.com/microsoft/PowerToys.git")
        .arg("src/example-repo")
        .status().expect("Failed to launch git");
    assert!(result.success());

    // Copy the repo then check out a slightly different version, so that only some files have changed
    std::fs::create_dir("src/example-repo-slight-change").expect("Failed to create folder");
    fs_extra::dir::copy("src/example-repo", "src/example-repo-slight-change", &CopyOptions { content_only: true, ..Default::default() })
        .expect("Failed to copy dir");
    assert!(std::process::Command::new("git").arg("remote").arg("set-branches").arg("origin").arg("*")
        .current_dir("src/example-repo-slight-change")
        .status().expect("Failed to launch git").success());
    assert!(std::process::Command::new("git").arg("fetch").arg("--depth").arg("1").arg("origin").arg("v0.64.1")
        .current_dir("src/example-repo-slight-change")
        .status().expect("Failed to launch git").success());
    assert!(std::process::Command::new("git").arg("checkout").arg("FETCH_HEAD")
        .current_dir("src/example-repo-slight-change")
        .status().expect("Failed to launch git").success());

    // Delete the .git folders so these aren't synced too.
    std::fs::remove_dir_all("src/example-repo/.git").expect("Failed to delete .git");
    std::fs::remove_dir_all("src/example-repo-slight-change/.git").expect("Failed to delete .git");

    // Delete some particularly deeply-nested folders, which cause scp.exe on windows to crash with a
    // stack overflow.
    std::fs::remove_dir_all("src/example-repo/src/modules/previewpane/MonacoPreviewHandler/monacoSRC/min/vs").expect("Failed to delete nested folders");
    std::fs::remove_dir_all("src/example-repo/src/settings-ui/Settings.UI.UnitTests/BackwardsCompatibility/TestFiles/").expect("Failed to delete nested folders");
  
    std::fs::remove_dir_all("src/example-repo-slight-change/src/modules/previewpane/MonacoPreviewHandler/monacoSRC/min/vs").expect("Failed to delete nested folders");
    std::fs::remove_dir_all("src/example-repo-slight-change/src/settings-ui/Settings.UI.UnitTests/BackwardsCompatibility/TestFiles/").expect("Failed to delete nested folders");

    // Copy the repo again and make a more significant change (rename src folder), so that many files will need
    // deleting and copying.
    std::fs::create_dir("src/example-repo-large-change").expect("Failed to create folder");
    fs_extra::dir::copy("src/example-repo-slight-change", "src/example-repo-large-change", &CopyOptions { content_only: true, ..Default::default() })
        .expect("Failed to copy dir");
    std::fs::rename("src/example-repo-large-change/src", "src/example-repo-large-change/src2").expect("Failed to rename");

    // Single large file
    std::fs::create_dir_all("src/large-file").expect("Failed to create dir");
    let mut f = std::fs::File::create("src/large-file/large.bin").expect("Failed to create file");
    for i in 0..1000_000 as i32 {
        let buf = [(i % 256) as u8; 1024];
        f.write_all(&buf).expect("Failed to write to file");
    }
}

fn main () {
    let args = CliArgs::parse();

    // Change working directory to a temporary folder which we will run all our benchmarks in
    let temp_dir = std::env::temp_dir().join("rjrssync-benchmarks");
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
    std::env::set_current_dir(&temp_dir).expect("Failed to set working directory");

    set_up_src_folders(&args);

    
    let mut results = vec![];
    
    let local_name = if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    };
    
    if !args.only_remote {
        results.push((format!("{local_name} -> {local_name}"), run_benchmarks_for_target(&args, Target::Local(temp_dir.join("dest")))));
    }
        
    if !args.only_remote && !args.only_local {
        #[cfg(windows)]
        results.push((format!(r"{local_name} -> \\wsl$\..."), run_benchmarks_for_target(&args, Target::Local(PathBuf::from(r"\\wsl$\\Ubuntu\\tmp\\rjrssync-benchmark-dest\\")))));

        #[cfg(unix)]
        results.push((format!("{local_name} -> /mnt/..."), run_benchmarks_for_target(&args, Target::Local(PathBuf::from("/mnt/t/Temp/rjrssync-benchmarks/dest")))));
    }
    
    if !args.only_local {
        results.push((format!("{local_name} -> Remote Windows"), run_benchmarks_for_target(&args, 
            Target::Remote { is_windows: true, user_and_host: test_utils::REMOTE_WINDOWS_CONFIG.0.clone(), folder: test_utils::REMOTE_WINDOWS_CONFIG.1.clone() + "\\benchmark-dest" })));
        
        results.push((format!("{local_name} -> Remote Linux"), run_benchmarks_for_target(&args, 
            Target::Remote { is_windows: false, user_and_host: test_utils::REMOTE_LINUX_CONFIG.0.clone(), folder: test_utils::REMOTE_LINUX_CONFIG.1.clone() + "/benchmark-dest" })));
    }

    println!();
    println!("Each cell shows <min> - <max> over {} sample(s) for: time | local memory (if available) | remote memory (if available)", args.num_samples);

    for (table_name, target_results) in results {
        println!();
        println!("{}", table_name);

        let mut ascii_table = AsciiTable::default();
        ascii_table.set_max_width(300);
        ascii_table.column(0).set_header("Test case");
        let mut table_data = vec![vec!["Everything copied"], vec!["Nothing copied"], vec!["Some copied"], vec!["Delete and copy"], vec!["Single large file"]];
        for (i, p) in target_results.iter().enumerate() {
            ascii_table.column(i + 1).set_header(&p.program);
            for (row, result) in p.results.iter().enumerate() {
                table_data[row].push(result);
            }
        }
        
        ascii_table.print(table_data);    
    }
}

struct ProgramResults {
    program: String,
    results: Vec<String>,
}
type TargetResults = Vec<ProgramResults>;

fn run_benchmarks_for_target(args: &CliArgs, target: Target) -> TargetResults {
    println!("Target: {:?}", target);
    let mut results = vec![];

    if args.programs.contains(&String::from("rjrssync")) {
        let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
        results.push(run_benchmarks_using_program(args, rjrssync_path, &["$SRC", "$DEST"], target.clone()));
    }
   
    if args.programs.contains(&String::from("rsync")) && !matches!(target, Target::Remote{ is_windows, .. } if is_windows) { // rsync is Linux -> Linux only
        #[cfg(unix)]
        // Note trailing slash on the src is important for rsync!
        results.push(run_benchmarks_using_program(args, "rsync", &["--archive", "--delete", "$SRC/", "$DEST"], target.clone()));
    }

    if args.programs.contains(&String::from("scp")) {
        results.push(run_benchmarks_using_program(args, "scp", &["-r", "-q", "$SRC", "$DEST"], target.clone()));
    }
   
    if args.programs.contains(&String::from("cp")) && matches!(target, Target::Local(..)) { // cp is local only
        #[cfg(unix)]
        results.push(run_benchmarks_using_program(args, "cp", &["-r", "$SRC", "$DEST"], target.clone()));
    }

    if args.programs.contains(&String::from("xcopy")) && matches!(target, Target::Local(..)) { // xcopy is local only
        #[cfg(windows)]
        results.push(run_benchmarks_using_program(args, "xcopy", &["/i", "/s", "/q", "/y", "$SRC", "$DEST"], target.clone()));
    }
   
    if args.programs.contains(&String::from("robocopy")) && matches!(target, Target::Local(..)) { // robocopy is local only
        #[cfg(windows)]
        results.push(run_benchmarks_using_program(args, "robocopy", &["/MIR", "/nfl", "/NJH", "/NJS", "/nc", "/ns", "/np", "/ndl", "$SRC", "$DEST"], target.clone()));
    }

    if args.programs.contains(&String::from("apis")) && matches!(target, Target::Local(..)) { // APIs are local only
        results.push(run_benchmarks(args, "APIs", |src, dest| -> PeakMemoryUsage {
            if !Path::new(&dest).exists() {
                std::fs::create_dir_all(&dest).expect("Failed to create dest folder");
            }
            fs_extra::dir::copy(src, dest, &CopyOptions { content_only: true, overwrite: true, ..Default::default() })
                .expect("Copy failed");
            PeakMemoryUsage { local: None, remote: None } // No measurement of peak memory usage as this is in-process
        }, target.clone()));
    }

    results
}

#[derive(Debug)]
struct PeakMemoryUsage {
    local: Option<usize>,
    remote: Option<usize>,
}

fn run_benchmarks_using_program(cli_args: &CliArgs, program: &str, program_args: &[&str], target: Target) -> ProgramResults {
    let id = Path::new(program).file_name().unwrap().to_string_lossy().to_string();
    let f = |src: String, dest: String| -> PeakMemoryUsage {
        let substitute = |p: &str| PathBuf::from(p.replace("$SRC", &src).replace("$DEST", &dest));
        let mut cmd = std::process::Command::new(program);
        let result = cmd
            .env("RJRSSYNC_TEST_DUMP_MEMORY_USAGE", "1") // To enable memory instrumentation when running rjrssync
            .args(program_args.iter().map(|a| substitute(a)));
        let hide_stdout = program == "scp"; // scp spams its stdout, and we can't turn this off, so we hide it.
        let result = test_utils::run_process_with_live_output_impl(result, hide_stdout, false, true);
        if program == "robocopy" {
            // robocopy has different exit codes (0 isn't what we want)
            let code = result.exit_status.code().unwrap();
            // println!("code = {code}");
            assert!(code == 0 || code == 1 || code == 3);
        } else {
            assert!(result.exit_status.success());
        }

        // Because reporting of memory usage is tricky (we can't do it well on Linux, nor for the remote
        // part of processes on any OS), we have our own instrumentation built into rjrssync. We use this 
        // when possible, otherwise use the memory usage from the process we launched (which only works on
        // Windows, and doesn't include remote usage)
        if program.contains("rjrssync") {
            // For rjrssync, parse the output to get the instrumented memory usage for both boss (local) and doer (remote, if relevant for this test)
            PeakMemoryUsage { 
                local: Some(result.stdout.lines().filter(|l| l.starts_with("Boss peak memory usage")).next().expect("Couldn't find line")
                    .split_once(':').expect("Failed to parse line").1.trim()
                    .parse::<usize>().expect("Failed to parse number")),
                remote: match &target {
                    Target::Local(_) => None,
                    Target::Remote { .. } => Some(result.stderr.lines().filter(|l| l.starts_with("Doer peak memory usage")).next().expect("Couldn't find line")
                        .split_once(':').expect("Failed to parse line").1.trim()
                        .parse::<usize>().expect("Failed to parse number")),
                } 
            }
        } else {
            // For other programs, use the value reported by run_process_with_live_output_impl, which has some limitations
            PeakMemoryUsage { local: result.peak_memory_usage, remote: None }
        }
    };
    run_benchmarks(cli_args, &id, f, target.clone())
}

fn run_benchmarks<F>(cli_args: &CliArgs, id: &str, sync_fn: F, target: Target) -> ProgramResults
    where F : Fn(String, String) -> PeakMemoryUsage
{
    println!("  Subject: {id}");

    #[derive(Debug)]
    struct Sample {
        time: Duration,
        peak_memory: PeakMemoryUsage,
    }

    let mut samples : Vec<Vec<Option<Sample>>> = vec![];
    for sample_idx in 0..cli_args.num_samples {
        println!("    Sample {sample_idx}");

        // Delete any old dest folder from other subjects
        let dest_prefix = match &target {
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

        let mut sample = vec![];

        // Sync example-repo to an empty folder, so this means everything is copied
        println!("      {id} example-repo everything copied...");
        let s = run(Path::new("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
        println!("      {id} example-repo everything copied: {:?}", s);
        sample.push(Some(s));

        // Sync again - this should be a no-op, but still needs to check that everything is up-to-date
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo nothing copied...");
            let s = run(Path::new("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo nothing copied: {:?}", s);
            sample.push(Some(s));
        } else {
            sample.push(None); // Programs like scp will always copy everything, so there's no point running this part of the test
        }

        // Make some small changes, e.g. check out a new version
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo some copied...");
            let s = run(Path::new("src").join("example-repo-slight-change").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo some copied: {:?}", s);
            sample.push(Some(s));
        } else {
            sample.push(None); // Programs like scp will always copy everything, so there's no point running this part of the test
        }

        // Make some large changes, (a big folder was renamed, so many things need deleting and then copying)
        if id.contains("rjrssync") || id.contains("robocopy") || id.contains("rsync") {
            println!("      {id} example-repo delete and copy...");
            let s = run(Path::new("src").join("example-repo-large-change").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
            println!("      {id} example-repo delete and copy: {:?}", s);
            sample.push(Some(s));
        } else {
            sample.push(None); // Programs like scp will always copy everything, so there's no point running this part of the test
        }

        // Sync a single large file
        println!("      {id} example-repo single large file...");
        let s = run(Path::new("src").join("large-file").to_string_lossy().to_string(), dest_prefix.clone() + "large-file");
        println!("      {id} example-repo single large file: {:?}", s);
        sample.push(Some(s));

        samples.push(sample);
    }

    // Make statistics and add to results table
    let mut results = vec![];
    for c in 0..samples[0].len() {
        let min_time = samples.iter().filter_map(|s| s[c].as_ref()).map(|s| s.time).min();
        let max_time = samples.iter().filter_map(|s| s[c].as_ref()).map(|s| s.time).max();
        let min_memory_local = samples.iter().filter_map(|s| s[c].as_ref()).filter_map(|s| s.peak_memory.local).min();
        let max_memory_local = samples.iter().filter_map(|s| s[c].as_ref()).filter_map(|s| s.peak_memory.local).max();
        let min_memory_remote = samples.iter().filter_map(|s| s[c].as_ref()).filter_map(|s| s.peak_memory.remote).min();
        let max_memory_remote = samples.iter().filter_map(|s| s[c].as_ref()).filter_map(|s| s.peak_memory.remote).max();
        if let (Some(min_time), Some(max_time)) = (min_time, max_time) {
            let mut s = format!("{:7} - {:7}", format_duration(min_time), format_duration(max_time));
            if let (Some(min_memory_local), Some(max_memory_local)) = (min_memory_local, max_memory_local) {
                s += &format!("| {:10} - {:10}", HumanBytes(min_memory_local as u64).to_string(), HumanBytes(max_memory_local as u64).to_string());
            }
            if let (Some(min_memory_remote), Some(max_memory_remote)) = (min_memory_remote, max_memory_remote) {
                s += &format!("| {:10} - {:10}", HumanBytes(min_memory_remote as u64).to_string(), HumanBytes(max_memory_remote as u64).to_string());
            }
            results.push(s);
        } else {
            results.push(format!("Skipped")); 
        }
    }
    ProgramResults { program: format!("{id} (x{})", samples.len()), results }
}

fn format_duration(d: Duration) -> String {
    if d.as_secs_f32() < 1.0 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{:.2}s", d.as_secs_f32())
    }
}