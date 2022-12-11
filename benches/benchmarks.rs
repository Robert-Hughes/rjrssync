use std::{time::Instant, path::{Path, PathBuf}, process::Stdio, io::Write};

use ascii_table::AsciiTable;
use fs_extra::dir::CopyOptions;

fn main () {
    // Change working directory to a temporary folder which we will run all our benchmarks in
    let temp_dir = std::env::temp_dir().join("rjrssync-benchmarks");
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
    std::env::set_current_dir(temp_dir).expect("Failed to set working directory");

    set_up_src_folders();

    let mut result_table: Vec<Vec<String>> = vec![];

   
    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    run_benchmarks_using_program(rjrssync_path, &["$SRC", "$DEST"], &mut result_table);
   
    #[cfg(unix)]
    run_benchmarks_using_program("rsync", &["--archive", "--delete", "$SRC", "$DEST"], &mut result_table);
   
    run_benchmarks_using_program("scp", &["-r", "-q", "$SRC", "$DEST"], &mut result_table);
   
    #[cfg(unix)]
    run_benchmarks_using_program("cp", &["-r", "$SRC", "$DEST"], &mut result_table);
   
    #[cfg(windows)]
    run_benchmarks_using_program("xcopy", &["/i", "/s", "/q", "/y", "$SRC", "$DEST"], &mut result_table);
   
    #[cfg(windows)]
    run_benchmarks_using_program("robocopy", &["/MIR", "/nfl", "/NJH", "/NJS", "/nc", "/ns", "/np", "/ndl", "$SRC", "$DEST"], &mut result_table);

    run_benchmarks("APIs", |src, dest| {
        if !Path::new(dest).exists() {
            std::fs::create_dir_all(dest).expect("Failed to create dest folder");
        }
        fs_extra::dir::copy(src, dest, &CopyOptions { content_only: true, overwrite: true, ..Default::default() })
            .expect("Copy failed");
    }, &mut result_table);

    let mut ascii_table = AsciiTable::default();
    ascii_table.column(0).set_header("Method");
    ascii_table.column(1).set_header("Simple Copy");
    ascii_table.column(2).set_header("No-op");
    ascii_table.column(3).set_header("Small change");
    ascii_table.column(4).set_header("Large file");

    println!();
    println!("Local -> Local");
    ascii_table.print(result_table);

    #[cfg(windows)]
    {
        println!();
        println!(r"Local -> \\wsl$\...");
    }

    #[cfg(unix)]
    {
        println!();
        println!("Local -> /mnt/...");
    }

    println!();
    println!("Local -> Remote Windows");

    println!();
    println!("Local -> Remote Linux");
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

    // Single large file
    std::fs::create_dir_all("src/large-file").expect("Failed to create dir");
    let mut f = std::fs::File::create("src/large-file/large.bin").expect("Failed to create file");
    for i in 0..1000_000 as i32 {
        let buf = [(i % 256) as u8; 1024];
        f.write_all(&buf).expect("Failed to write to file");
    }
}

fn run_benchmarks_using_program(program: &str, args: &[&str], result_table: &mut Vec<Vec<String>>) {
    let id = Path::new(program).file_name().unwrap().to_string_lossy().to_string();
    let f = |src: &'static str, dest: &'static str| {
        let src = src.replace("/", &std::path::MAIN_SEPARATOR.to_string());
        let dest = dest.replace("/", &std::path::MAIN_SEPARATOR.to_string());

        let substitute = |p: &str| PathBuf::from(p.replace("$SRC", &src).replace("$DEST", &dest));
       // println!("{:?}", args.iter().map(|a| substitute(a)).collect::<Vec<PathBuf>>());
        let result = std::process::Command::new(program)
            .args(args.iter().map(|a| substitute(a)))
            .stdout(Stdio::null()) // To hide output from e.g. scp which is very noisy :(
            .stderr(Stdio::null())
            .status().expect("Failed to launch program");
        if program == "robocopy" {
            // robocopy has different exit codes (0 isn't what we want)
            let code = result.code().unwrap();
            // println!("code = {code}");
            assert!(code == 0 || code == 1 || code == 3);
        } else {
            assert!(result.success());
        }
    };
    run_benchmarks(&id, f, result_table);
}

fn run_benchmarks<F>(id: &str, sync_fn: F, result_table: &mut Vec<Vec<String>>) where F : Fn(&'static str, &'static str) {
    println!("Subject: {id}");

    // Delete any old dest folder from other subjects
    if Path::new("dest").exists() {
        std::fs::remove_dir_all("dest").expect("Failed to delete old dest folder");
    }
    std::fs::create_dir("dest").expect("Failed to create dest dir");

    let run = |src, dest| {
        let start = Instant::now();
        sync_fn(src, dest);
        let elapsed = start.elapsed();
        elapsed    
    };

    let mut results = vec![id.to_string()];

    // Sync example-repo to an empty folder, so this is a simple copy
    let elapsed = run("src/example-repo", "dest/example-repo");
    println!("{id} example-repo simple copy: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));
    
    // Sync again - this should be a no-op, but still needs to check that everything is up-to-date
    let elapsed = run("src/example-repo", "dest/example-repo");
    println!("{id} example-repo no-op: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));

    // Make some small changes, e.g. check out a new version
    let elapsed = run("src/example-repo-slight-change", "dest/example-repo");
    println!("{id} example-repo small change: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));

    // Sync a single large file
    let elapsed = run("src/large-file", "dest/large-file");
    println!("{id} example-repo large file: {:?}", elapsed);
    results.push(format!("{:?}", elapsed));

    result_table.push(results);
}