use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

use crate::test_framework::*;
use crate::folder;
use map_macro::map;
use regex::Regex;
use crate::filesystem_node::*;

/// Simple folder -> folder sync
#[test]
fn test_simple_folder_sync() {
    let src_folder = folder! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder! {
            "sc" => file("contents3"),
        }
    };
    run_expect_success(&src_folder, &empty_folder(), copied_files_and_folders(3, 1));
}

/// Some files and a folder (with contents) in the destination need deleting.
#[test]
fn test_remove_dest_stuff() {
    let src_folder = folder! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder! {
            "sc" => file("contents3"),
        }
    };
    let dest_folder = folder! {
        "remove me" => file("contents1"),
        "remove me too" => file("contents2"),
        "remove this whole folder" => folder! {
            "sc" => file("contents3"),
            "sc2" => file("contents3"),
            "remove this whole folder" => folder! {
                "sc" => file("contents3"),
            }
        }
    };
    run_expect_success(&src_folder, &dest_folder, NumActions { copied_files: 3, created_folders: 1, copied_symlinks: 0,
        deleted_files: 5, deleted_folders: 2, deleted_symlinks: 0 });
}

/// A file exists but has an old timestamp so needs updating.
#[test]
fn test_update_file() {
    let src_folder = folder! {
        "file" => file_with_modified("contents1", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    let dest_folder = folder! {
        "file" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src_folder, &dest_folder, copied_files(1));
}

/// Most files have the same timestamp so don't need updating, but one does.
#[test]
fn test_skip_unchanged() {
    let src_folder = folder! {
        "file1" => file_with_modified("contentsNEW", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "file2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "file3" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
    };
    let dest_folder = folder! {
        "file1" => file_with_modified("contentsOLD", SystemTime::UNIX_EPOCH),
        "file2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "file3" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
    };
    // Check that exactly one file was copied (the other two should have been skipped)
    run_expect_success(&src_folder, &dest_folder, copied_files(1));
}

/// The destination is inside several folders that don't exist yet - they should be created.
#[test]
fn test_dest_ancestors_dont_exist() {
    let src = &file("contents");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src.txt", &src),
        ],
        args: vec![
            "$TEMP/src.txt".to_string(),
            "$TEMP/dest1/dest2/dest3/dest.txt".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: copied_files(1).into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src.txt", Some(src)), // Source should always be unchanged
            ("$TEMP/dest1/dest2/dest3/dest.txt", Some(src)), // Dest should be identical to source
        ],
        ..Default::default()
    });
}

/// Tests that src and dest can use relative paths.
#[test]
fn test_relative_paths() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src_folder),
        ],
        args: vec![
            "src".to_string(),
            "dest".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: copied_files_and_folders(1, 1).into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src_folder)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src_folder)), // Dest should be same as source
        ],
        ..Default::default()
    });
}

/// Tests that the --spec option works instead of specifying SRC and DEST directly.
#[test]
fn test_spec_file() {
    let spec_file = file(r#"
        syncs:
        - src: src1/
          dest: dest1/
        - src: src2/
          dest: dest2/
    "#);
    let src1 = folder! {
        "c1" => file("contents1"),
    };
    let src2 = folder! {
        "c2" => file("contents2"),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/spec.yaml", &spec_file),
            ("$TEMP/src1", &src1),
            ("$TEMP/src2", &src2),
        ],
        args: vec![
            "--spec".to_string(),
            "$TEMP/spec.yaml".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("src1/ => dest1/")).unwrap()),
            (1, Regex::new(&regex::escape("src2/ => dest2/")).unwrap()),
            (2, Regex::new(&regex::escape("Copied 1 file(s)")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/dest1", Some(&src1)),
            ("$TEMP/dest2", Some(&src2)),
        ],
        ..Default::default()
    });
}

/// Syncing a large file that therefore needs splitting into chunks
#[test]
fn test_large_file() {
    let src_folder = folder! {
        "file" => file(&"so much big!".repeat(1000*1000*10)), // Roughly 100MB
    };
    run_expect_success(&src_folder, &empty_folder(), copied_files(1));
}

/// Checks that the --dry-run flag means that no changes are made, and that information about
/// what _would_ happen is printed.
#[test]
fn dry_run() {
    let slash = regex::escape(&std::path::MAIN_SEPARATOR.to_string());
    let src = folder! {
        "file" => file("contents"),
        "folder" => folder! {
            "c1" => file("contents1"),
        },
        "symlink" => symlink_file("bob")
    };
    let dest = folder! {
        "file2" => file("contents"),
        "folder2" => folder! {
            "c12" => file("contents1"),
        },
        "symlink2" => symlink_file("bob")
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dry-run".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&format!(r"Would delete dest file .*/dest{slash}folder2{slash}c12")).unwrap()),
            (1, Regex::new(&format!(r"Would delete dest symlink .*/dest{slash}symlink2")).unwrap()),
            (1, Regex::new(&format!(r"Would delete dest folder .*/dest{slash}folder2")).unwrap()),
            (1, Regex::new(&format!(r"Would delete dest file .*/dest{slash}file2")).unwrap()),
            (1, Regex::new(&format!(r"Would copy source file .*/src{slash}file' => dest file .*/dest{slash}file")).unwrap()),
            (1, Regex::new(&format!(r"Would create dest folder .*/dest{slash}folder")).unwrap()),
            (1, Regex::new(&format!(r"Would create dest symlink .*/dest{slash}symlink")).unwrap()),
            (1, Regex::new(&format!(r"Would copy source file .*/src{slash}folder{slash}c1' => dest file .*/dest{slash}folder{slash}c1")).unwrap()),
            (1, Regex::new(&regex::escape("Would delete 2 file(s) totalling 17B, 1 folder(s) and 1 symlink(s)")).unwrap()),
            (1, Regex::new(&regex::escape("Would copy 2 file(s) totalling 17B, would create 1 folder(s) and would copy 1 symlink(s)")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&dest)), // Dest should be unchanged too
        ],
        ..Default::default()
    });
}

/// Checks that the --dry-run flag means that no changes are made, and that information about
/// what _would_ happen is printed. Also checks when dest ancestor folders are missing, that
/// they are not created.
#[test]
fn dry_run_root_ancestors() {
    let slash = regex::escape(&std::path::MAIN_SEPARATOR.to_string());
    let src = folder! {
        "file" => file("contents"),
        "folder" => folder! {
            "c1" => file("contents1"),
        },
        "symlink" => symlink_file("bob")
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            // Place the dest inside some non-existent folders, to check that root ancestors are not
            // created in dry-run mode
            "$TEMP/dest1/dest2/dest3/dest".to_string(),
            "--dry-run".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&format!(r"Would create dest root folder .*/dest1/dest2/dest3/dest")).unwrap()),
            (1, Regex::new(&format!(r"Would copy source file .*/src{slash}file' => dest file .*/dest{slash}file")).unwrap()),
            (1, Regex::new(&format!(r"Would create dest folder .*/dest{slash}folder")).unwrap()),
            (1, Regex::new(&format!(r"Would create dest symlink .*/dest{slash}symlink")).unwrap()),
            (1, Regex::new(&format!(r"Would copy source file .*/src{slash}folder{slash}c1' => dest file .*/dest{slash}folder{slash}c1")).unwrap()),
            (1, Regex::new(&regex::escape("Would copy 2 file(s) totalling 17B, would create 2 folder(s) and would copy 1 symlink(s)")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest1", None), // Dest should be unchanged, with no ancestors created
        ],
        ..Default::default()
    });
}

/// Checks what happens when a file's size changes between the querying phase and the actual sync.
#[test]
fn file_size_change_during_sync() {
    let src_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    src_file.as_file().write_all("original contents".as_bytes()).expect("Failed to write file");

    // Launch a background thread that constantly changes the file's size
    let mut second_handle = src_file.reopen().expect("Failed to reopen temp file");
    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop_signal2 = stop_signal.clone();
    let thread = std::thread::spawn(move || {
        while !stop_signal2.load(Ordering::Relaxed) {
            write!(second_handle, "some more stuff").expect("Failed to write to temp file");
        }
    });

    run(TestDesc {
        args: vec![
            src_file.path().to_string_lossy().to_string(),
            "$TEMP/dest_file.txt".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            (1, Regex::new("Size of .* changed during the sync").unwrap()),
        ],
        expected_filesystem_nodes: vec![
            // Source file is constantly changing, so nothing we can really check here
            // Dest file might have been changed, but it depends at what point the error is caught, so there's nothing we can really check here
        ],
        ..Default::default()
    });

    stop_signal.store(true, Ordering::Relaxed);
    thread.join().expect("Failed to join thread");
}

/// Checks that --stats prints some stats
#[test]
fn stats() {
    let src = folder! {
        "file" => file("contents"),
        "folder" => folder! {
            "c1" => file("contents1"),
        },
        "symlink" => symlink_file("bob")
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--stats".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Source: 2 file(s) totalling 17B, 2 folder(s) and 1 symlink(s)")).unwrap()),
            (1, Regex::new(&regex::escape("Dest: 0 file(s) totalling 0B, 0 folder(s) and 0 symlink(s)")).unwrap()),
            (1, Regex::new("Queried in .* seconds").unwrap()),
            (1, Regex::new("Deleted .* in .* seconds").unwrap()),
            (1, Regex::new("Copied .* in .* seconds").unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)),
            ("$TEMP/dest", Some(&src)),
        ],
        ..Default::default()
    });
}

/// Checks that --quiet doesn't print anything, but does show errors
#[test]
fn quiet() {
    let src = folder! {
        "file" => file("contents"),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--quiet".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (0, Regex::new(".+").unwrap()), // No output at all
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)),
            ("$TEMP/dest", Some(&src)),
        ],
        ..Default::default()
    });

    run(TestDesc {
        args: vec![
            "$TEMP/src-that-doesnt-exist".to_string(),
            "$TEMP/dest".to_string(),
            "--quiet".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            (1, Regex::new("src path .* doesn't exist").unwrap()), // Just an error message
        ],
        ..Default::default()
    });
}

/// Checks that --verbose prints additional messages
#[test]
fn verbose() {
    let src = folder! {
        "file" => file("contents"),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--verbose".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (2, Regex::new("setup_comms").unwrap()),
            (1, Regex::new("Copying source file").unwrap()),
            (2, Regex::new("Shutdown command received").unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)),
            ("$TEMP/dest", Some(&src)),
        ],
        ..Default::default()
    });
}

