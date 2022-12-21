use std::time::SystemTime;

use regex::Regex;

use map_macro::map;
use crate::{test_framework::{run, TestDesc, empty_folder, folder, NumActions, file_with_modified}, folder};

/// Tests that rjrssync can be launched on a remote platform, and communication is estabilished.
/// There is no proper sync performed (just syncing an empty folder), but this checks that
/// the ssh/scp/cargo commands and TCP connection works.
fn test_remote_launch_impl(remote_platform_temp_variable: &str) {
    // First run with --force-redeploy, to check that the remote deploying and building works,
    // even when the remote already has rjrssync set up.
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &empty_folder())
        ],
        args: vec![
            "--force-redeploy".to_string(),
            "$TEMP/src".to_string(),
            format!("{remote_platform_temp_variable}/dest")
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new("Compiling rjrssync").unwrap(),
        ],
        ..Default::default()
    });

    // Then run without --force-redeploy, and it should use the existing copy
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &empty_folder())
        ],
        args: vec![
            "$TEMP/src".to_string(),
            format!("{remote_platform_temp_variable}/dest")
        ],
        expected_exit_code: 0,
        unexpected_output_messages: vec![
            Regex::new("Compiling rjrssync").unwrap(),
        ],
        ..Default::default()
    });
}

#[test]
fn test_remote_launch_windows() {
    test_remote_launch_impl("$REMOTE_WINDOWS_TEMP");
}

#[test]
fn test_remote_launch_linux() {
    test_remote_launch_impl("$REMOTE_LINUX_TEMP");
}

/// Checks that a directory structure can be synced between OSes, and between local and remote.
/// This includes syncing between the same remote (as both src and dest).
/// This checks for example that paths are normalized to have the same slashes, so that we can interop 
/// correctly.
#[test]
fn test_cross_platform() {
    // Test some new files, some files that need deleting, and some files that can stay as they are
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "c3" => folder! {
            "sc" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        },
        "same" => folder! { // Make sure that the file whichd doesn't need copying is inside a folder, so that slashes are checked
            "same" => file_with_modified("same", SystemTime::UNIX_EPOCH),
        },
    };
    let dest = folder! {
        "remove me" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "remove me too" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "remove this whole folder" => folder! {
            "sc" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
            "sc2" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
            "remove this whole folder" => folder! {
                "sc" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
            }
        },
        "same" => folder! {
            "same" => file_with_modified("same", SystemTime::UNIX_EPOCH),
        },
    };

    // Check every combination of local and remote for source and dest
    for src_path in ["$TEMP/src", "$REMOTE_WINDOWS_TEMP/src", "$REMOTE_LINUX_TEMP/src"] {
        for dest_path in ["$TEMP/dest", "$REMOTE_WINDOWS_TEMP/dest", "$REMOTE_LINUX_TEMP/dest"] {
            run(TestDesc {
                setup_filesystem_nodes: vec![
                    (src_path, &src),
                    (dest_path, &dest),
                ],
                args: vec![
                    src_path.to_string(),
                    dest_path.to_string(),
                ],
                expected_exit_code: 0,
                expected_filesystem_nodes: vec![
                    (src_path, Some(&src)), // Source should always be unchanged
                    (dest_path, Some(&src)), // Dest should be identical to source
                ],
                ..Default::default()
            }.with_expected_actions(NumActions {
                copied_files: 3,
                created_folders: 1,
                copied_symlinks: 0,
                deleted_files: 5,
                deleted_folders: 2,
                deleted_symlinks: 0,
            }));
        }
    }
}
