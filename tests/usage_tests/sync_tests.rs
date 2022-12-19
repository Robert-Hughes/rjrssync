use std::time::Duration;
use std::time::SystemTime;

use crate::test_framework::*;
use crate::folder;
use map_macro::map;
use regex::Regex;

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
        expected_filesystem_nodes: vec![
            ("$TEMP/src.txt", Some(src)), // Source should always be unchanged
            ("$TEMP/dest1/dest2/dest3/dest.txt", Some(src)), // Dest should be identical to source
        ],
        ..Default::default()
    }.with_expected_actions(copied_files(1)));
}

#[test]
fn test_filters() {
    let src_folder = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "c3" => folder! {
            "sc1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
            "sc2" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        }
    };
    // Because of the filter, not everything will get copied
    let expected_dest_folder = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c3" => folder! {
            "sc2" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        }
    };

    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src_folder),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--filter".to_string(),
            "+c3.*".to_string(),
            "--filter".to_string(),
            "+c1".to_string(),
            "--filter".to_string(),
            "-.*/sc1".to_string(),
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src_folder)), // Source should always be unchanged
            ("$TEMP/dest", Some(&expected_dest_folder)),
        ],
        ..Default::default()
    }.with_expected_actions(copied_files_and_folders(2, 2)));
}

#[test]
fn test_invalid_filter_prefix() {
    let src = &file("contents");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--filter".to_string(),
            "BLARG".to_string(),
        ],
        expected_exit_code: 18,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Invalid filter 'BLARG'")).unwrap(),
        ],
        ..Default::default()
    });
}

#[test]
fn test_invalid_filter_regex() {
    let src = &empty_folder(); // Note that we need a folder, not a file, as files don't ever get walked and so we would never check the filter!
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--filter".to_string(),
            "+[[INVALID REGEX".to_string(),
       ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Invalid regex for filter")).unwrap(),
        ],
        ..Default::default()
    });
}



/// A folder that needs deleting on the destination has files which have been excluded, and so the folder can't be deleted.
#[test]
fn test_remove_dest_folder_with_excluded_files() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    let dest_folder = folder! {
        "This folder would be removed" => folder! {
            "EXCLUDED" => file("But it can't because this file has been excluded from the sync"),
        }
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src_folder),
            ("$TEMP/dest", &dest_folder),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--filter".to_string(),
            "-.*/EXCLUDED".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            // Check for both Linux and Windows error messages
            Regex::new("(The directory is not empty)|(Directory not empty)").unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src_folder)), // Source should always be unchanged
            // ("$TEMP/dest", Some(&dest_folder)), // Dest should be unchanged too as it failed.
            // Now that we run stuff asynchronously, the c1 file may actually have been copied anyway, before we see
            // the error and stop. Therefore the dest may have been changed, or it may not - both are valid.
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
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src_folder)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src_folder)), // Dest should be same as source
        ],
        ..Default::default()
    }.with_expected_actions(copied_files_and_folders(1, 1)));
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
            Regex::new(&regex::escape("src1/ => dest1/")).unwrap(),
            Regex::new(&regex::escape("src2/ => dest2/")).unwrap(),
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap(),
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

/// Syncing three files which already exists on the dest, but the dest has newer modified
/// dates. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "skip" and then "overwrite", then "skip" again.
#[test]
fn test_dest_file_newer_prompt_skip_then_overwrite() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "c3" => file_with_modified("contents6", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c3" => file_with_modified("contents7", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip (just this occurence)"),
            String::from("Overwrite (just this occurence)"),
            String::from("Skip (just this occurence)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new("Dest file .*c1.* is newer than src file .*c1.*").unwrap(),
            Regex::new("Dest file .*c2.* is newer than src file .*c2.*").unwrap(),
            // Note that we need this last check, to make sure that the second prompt response only affects one file, not all remaining files
            Regex::new("Dest file .*c3.* is newer than src file .*c3.*").unwrap(), 
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap(), // 1 file copied the other skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&folder! { // First and last file skipped, the other overwritten
                "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
                "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
                "c3" => file_with_modified("contents7", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            })),
        ],
        ..Default::default()
    });
}

/// Syncing two files which already exists on the dest, but the dest has newer modified
/// dates. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "skip all" on the first prompt, so both files should be skipped.
#[test]
fn test_dest_file_newer_prompt_skip_all() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip (all occurences)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new("Dest file .*c1.* is newer than src file .*c1.*").unwrap(),
            Regex::new(&regex::escape("Nothing to do")).unwrap(), // Both files skipped
        ],
        unexpected_output_messages: vec![
            Regex::new("Dest file .*c2.* is newer than src file .*c2.*").unwrap(), // We'll never be prompted about c2, because we choose to "skip all"
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing two files which already exists on the dest, but the dest has newer modified
/// dates. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "overwrite all" on the first prompt, so both files should be overwritten.
#[test]
fn test_dest_file_newer_prompt_overwrite_all() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Overwrite (all occurences)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new("Dest file .*c1.* is newer than src file .*c1.*").unwrap(),
            Regex::new(&regex::escape("Copied 2 file(s)")).unwrap(), // Both files copied
        ],
        unexpected_output_messages: vec![
            Regex::new("Dest file .*c2.* is newer than src file .*c2.*").unwrap(), // We'll never be prompted about c2, because we choose to "overwrite all"
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // All files copied, so same as src
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose to cancel the prompt, so the sync should stop.
#[test]
fn test_dest_file_newer_prompt_cancel() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("<CANCEL>"),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            Regex::new("Dest file .*c1.* is newer than src file .*c1.*").unwrap(),
            Regex::new(&regex::escape("Will not overwrite")).unwrap(), // Cancelled
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to produce an error.
#[test]
fn test_dest_file_newer_error() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "error".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Will not overwrite")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to skip.
#[test]
fn test_dest_file_newer_skip() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "skip".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Nothing to do")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to overwrite.
#[test]
fn test_dest_file_newer_overwrite() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "overwrite".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // Same as source
        ],
        ..Default::default()
    });
}
