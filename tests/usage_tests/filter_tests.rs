use std::time::SystemTime;

use crate::test_framework::*;
use crate::folder;
use map_macro::map;
use regex::Regex;

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

/// Checks that the regex must match the full path, not just part of it.
#[test]
fn test_filters_partial_match() {
    let src_folder = folder! {
        // This file would be matched by the filter "build", if it only checked for partial matches
        "mybuilder.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "build" => folder! {
            "sc1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        }
    };
    // Because of the filter, not everything will get copied. mybuilder.txt will though,
    // because it isn't a complete match for the filter.
    let expected_dest_folder = folder! {
        "mybuilder.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };

    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src_folder),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--filter".to_string(),
            "-build".to_string(),
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src_folder)), // Source should always be unchanged
            ("$TEMP/dest", Some(&expected_dest_folder)),
        ],
        ..Default::default()
    }.with_expected_actions(copied_files_and_folders(1, 1)));
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
        expected_exit_code: 12,
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
            Regex::new(&regex::escape("regex parse error")).unwrap(),
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

// "Tag" these tests as they require remote platforms (GitHub Actions differentiates these)
mod remote {

use super::*;

/// Tests that filters operate on normalized (forward slash) relative paths on both doers,
/// no matter the platform (Windows vs Linux).
/// This also tests that (de-)serialization of the Filters struct works correctly.
#[test]
fn test_filter_normalized_paths() {
    // Run this test between Linux and Windows remotes, so we cover both platforms, no matter what the native
    // platform is.
    let src_folder = folder! {
        "Folder" => folder! {
            "CopyMe" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
            "EXCLUDE" => folder! {
                "exclude1" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
                "exclude2" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
            }
        }
    };
    let dest_folder = folder! {
        "Folder" => folder! {
            "EXCLUDE" => folder! {
                "exclude3" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
                "exclude4" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
            }
        }
    };
    // Dest should have the CopyMe file added, but the EXCLUDE folder should remain untouched, as it was ignored
    // for both the source (nothing new added) and dest sides (nothing removed)
    let expected_dest = folder! {
        "Folder" => folder! {
            "CopyMe" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
            "EXCLUDE" => folder! {
                "exclude3" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
                "exclude4" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
            }
        }
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$REMOTE_WINDOWS_TEMP/src", &src_folder),
            ("$REMOTE_LINUX_TEMP/dest", &dest_folder),
        ],
        args: vec![
            "$REMOTE_WINDOWS_TEMP/src".to_string(),
            "$REMOTE_LINUX_TEMP/dest".to_string(),
            "--filter".to_string(),
            "-Folder/EXCLUDE".to_string(), // note we use a forward slash - this should match the EXCLUDE subfolder on both Linux and Windows
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$REMOTE_WINDOWS_TEMP/src", Some(&src_folder)), // Source should always be unchanged
            ("$REMOTE_LINUX_TEMP/dest", Some(&expected_dest)),
        ],
        ..Default::default()
    }.with_expected_actions(copied_files(1)));
}

}

