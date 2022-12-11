use std::{time::Instant, path::{Path, PathBuf}, io::Write};

use ascii_table::AsciiTable;
use fs_extra::dir::CopyOptions;

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

fn main () {
    // Change working directory to a temporary folder which we will run all our benchmarks in
    let temp_dir = std::env::temp_dir().join("rjrssync-benchmarks");
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
    std::env::set_current_dir(&temp_dir).expect("Failed to set working directory");

    set_up_src_folders();

    
    let mut results = vec![];
    
    results.push(("Local -> Local", run_benchmarks_for_target(Target::Local(temp_dir.join("dest")))));
    
    #[cfg(windows)]
    results.push((r"Local -> \\wsl$\...", run_benchmarks_for_target(Target::Local(PathBuf::from(r"\\wsl$\\Ubuntu\\tmp\\rjrssync-benchmark-dest\\")))));
    
    #[cfg(unix)]
    results.push(("Local -> /mnt/...", run_benchmarks_for_target(Target::Local(PathBuf::from("/mnt/t/Temp/rjrssync-benchmarks/dest")))));
    
    results.push(("Local -> Remote Windows", run_benchmarks_for_target(
        Target::Remote { is_windows: true, user_and_host: test_utils::REMOTE_WINDOWS_CONFIG.0.clone(), folder: test_utils::REMOTE_WINDOWS_CONFIG.1.clone() + "\\benchmark-dest" })));
    
    results.push(("Local -> Remote Linux", run_benchmarks_for_target(
        Target::Remote { is_windows: false, user_and_host: test_utils::REMOTE_LINUX_CONFIG.0.clone(), folder: test_utils::REMOTE_LINUX_CONFIG.1.clone() + "/benchmark-dest" })));

    let mut ascii_table = AsciiTable::default();
    ascii_table.column(0).set_header("Method");
    ascii_table.column(1).set_header("Everything copied");
    ascii_table.column(2).set_header("Nothing copied");
    ascii_table.column(3).set_header("Some copied");
    ascii_table.column(4).set_header("Single large file");

    for (table_name, table_data) in results {
        println!();
        println!("{}", table_name);
        ascii_table.print(table_data);    
    }
}

fn set_up_src_folders() {
    if Path::new("src").exists() && std::env::var("RJRSSYNC_BENCHMARKS_SKIP_SETUP").is_ok() {
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

    // Single large file
    std::fs::create_dir_all("src/large-file").expect("Failed to create dir");
    let mut f = std::fs::File::create("src/large-file/large.bin").expect("Failed to create file");
    for i in 0..1000_000 as i32 {
        let buf = [(i % 256) as u8; 1024];
        f.write_all(&buf).expect("Failed to write to file");
    }
}

fn run_benchmarks_for_target(target: Target) -> Vec<Vec<String>> {
    println!("Target: {:?}", target);
    let mut result_table = vec![];

    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    run_benchmarks_using_program(rjrssync_path, &["$SRC", "$DEST"], target.clone(), &mut result_table);
   
    if !matches!(target, Target::Remote{ is_windows, .. } if is_windows) { // rsync is Linux -> Linux only
        #[cfg(unix)]
        // Note trailing slash on the src is important for rsync!
        run_benchmarks_using_program("rsync", &["--archive", "--delete", "$SRC/", "$DEST"], target.clone(), &mut result_table);
    }

    run_benchmarks_using_program("scp", &["-r", "-q", "$SRC", "$DEST"], target.clone(), &mut result_table);
   
    if matches!(target, Target::Local(..)) { // cp is local only
        #[cfg(unix)]
        run_benchmarks_using_program("cp", &["-r", "$SRC", "$DEST"], target.clone(), &mut result_table);
    }

    if matches!(target, Target::Local(..)) { // xcopy is local only
        #[cfg(windows)]
        run_benchmarks_using_program("xcopy", &["/i", "/s", "/q", "/y", "$SRC", "$DEST"], target.clone(), &mut result_table);
    }
   
    if matches!(target, Target::Local(..)) { // robocopy is local only
        #[cfg(windows)]
        run_benchmarks_using_program("robocopy", &["/MIR", "/nfl", "/NJH", "/NJS", "/nc", "/ns", "/np", "/ndl", "$SRC", "$DEST"], target.clone(), &mut result_table);
    }

    if matches!(target, Target::Local(..)) { // APIs are local only
            run_benchmarks("APIs", |src, dest| {
            if !Path::new(&dest).exists() {
                std::fs::create_dir_all(&dest).expect("Failed to create dest folder");
            }
            fs_extra::dir::copy(src, dest, &CopyOptions { content_only: true, overwrite: true, ..Default::default() })
                .expect("Copy failed");
        }, target.clone(), &mut result_table);
    }

    result_table
}

fn run_benchmarks_using_program(program: &str, args: &[&str], target: Target, result_table: &mut Vec<Vec<String>>) {
    let id = Path::new(program).file_name().unwrap().to_string_lossy().to_string();
    let f = |src: String, dest: String| {
        let substitute = |p: &str| PathBuf::from(p.replace("$SRC", &src).replace("$DEST", &dest));
       // println!("{:?}", args.iter().map(|a| substitute(a)).collect::<Vec<PathBuf>>());
        let mut cmd = std::process::Command::new(program);
        let result = cmd
            .args(args.iter().map(|a| substitute(a)));
        let hide_stdout = program == "scp"; // scp spams its stdout, and we can't turn this off, so we hide it.
        let result = test_utils::run_process_with_live_output_impl(result, hide_stdout, false);
        if program == "robocopy" {
            // robocopy has different exit codes (0 isn't what we want)
            let code = result.exit_status.code().unwrap();
            // println!("code = {code}");
            assert!(code == 0 || code == 1 || code == 3);
        } else {
            assert!(result.exit_status.success());
        }
    };
    run_benchmarks(&id, f, target, result_table);
}

fn run_benchmarks<F>(id: &str, sync_fn: F, target: Target, result_table: &mut Vec<Vec<String>>) where F : Fn(String, String) {
    println!("  Subject: {id}");

    // Delete any old dest folder from other subjects
    let dest_prefix = match target {
        Target::Local(d) => {
            if Path::new(&d).exists() {
                std::fs::remove_dir_all(&d).expect("Failed to delete old dest folder");
            }
            std::fs::create_dir(&d).expect("Failed to create dest dir");
            d.to_string_lossy().to_string() + &std::path::MAIN_SEPARATOR.to_string()
        }
        Target::Remote { is_windows, user_and_host, folder } => {
            if is_windows {
                // Use run_process_with_live_output to avoid messing up terminal line endings
                let _ = test_utils::run_process_with_live_output(std::process::Command::new("ssh").arg(&user_and_host).arg(format!("rmdir /Q /S {folder}")));
                // This one can fail, if the folder doesn't exist

                let result = test_utils::run_process_with_live_output(std::process::Command::new("ssh").arg(&user_and_host).arg(format!("mkdir {folder}")));
                assert!(result.exit_status.success());
            } else {
                let result = test_utils::run_process_with_live_output(std::process::Command::new("ssh").arg(&user_and_host).arg(format!("rm -rf '{folder}' && mkdir -p '{folder}'")));
                assert!(result.exit_status.success());
            }
            let remote_sep = if is_windows { "\\" } else { "/" };
            user_and_host + ":" + &folder + remote_sep
        }
    };

    let run = |src, dest| {
        let start = Instant::now();
        sync_fn(src, dest);
        let elapsed = start.elapsed();
        elapsed    
    };

    let mut results = vec![id.to_string()];

    // Sync example-repo to an empty folder, so this means everything is copied
    println!("    {id} example-repo everything copied...");
    let elapsed = run(Path::new("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
    println!("    {id} example-repo everything copied: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));
    
    // Sync again - this should be a no-op, but still needs to check that everything is up-to-date
    println!("    {id} example-repo nothing copied...");
    let elapsed = run(Path::new("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
    println!("    {id} example-repo nothing copied: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));

    // Make some small changes, e.g. check out a new version
    println!("    {id} example-repo some copied...");
    let elapsed = run(Path::new("src").join("example-repo-slight-change").to_string_lossy().to_string(), dest_prefix.clone() + "example-repo");
    println!("    {id} example-repo some copied: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));

    // Sync a single large file
    println!("    {id} example-repo single large file...");
    let elapsed = run(Path::new("src").join("example-repo").to_string_lossy().to_string(), dest_prefix.clone() + "large-file");
    println!("    {id} example-repo single large file: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));

    result_table.push(results);
}