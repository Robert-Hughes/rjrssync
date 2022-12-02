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
    run_expect_success(&src_folder, &empty_folder(), 3);
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
    run_expect_success(&src_folder, &dest_folder, 3);
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
    run_expect_success(&src_folder, &dest_folder, 1);
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
    run_expect_success(&src_folder, &dest_folder, 1);
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
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src.txt", Some(src)), // Source should always be unchanged
            ("$TEMP/dest1/dest2/dest3/dest.txt", Some(src)), // Dest should be identical to source
        ],
        ..Default::default()
    });
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
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 2 file(s), created 2 folder(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src_folder)), // Source should always be unchanged
            ("$TEMP/dest", Some(&expected_dest_folder)),
        ],
        ..Default::default()
    });
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
            ("$TEMP/dest", Some(&dest_folder)), // Dest should be unchanged too as it failed
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
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap()
        ],
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
