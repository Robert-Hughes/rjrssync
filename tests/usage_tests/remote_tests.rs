use std::time::SystemTime;

use regex::Regex;

use map_macro::map;
use crate::{test_framework::{run, TestDesc, empty_folder, folder, NumActions, file_with_modified, copied_files}, folder};

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
            "--needs-deploy".to_string(),
            "deploy".to_string(), // Skip the confirmation prompt for deploying
            "$TEMP/src".to_string(),
            format!("{remote_platform_temp_variable}/dest"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Finished release [optimized] target")).unwrap()),
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
        expected_output_messages: vec![
            (0, Regex::new(&regex::escape("Finished release [optimized] target")).unwrap()),
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
                    "--needs-deploy".to_string(),
                    "deploy".to_string(), // Skip the confirmation prompt for deploying
                ],
                expected_exit_code: 0,
                expected_output_messages: NumActions {
                    copied_files: 3,
                    created_folders: 1,
                    copied_symlinks: 0,
                    deleted_files: 5,
                    deleted_folders: 2,
                    deleted_symlinks: 0,
                }.into(),
                expected_filesystem_nodes: vec![
                    (src_path, Some(&src)), // Source should always be unchanged
                    (dest_path, Some(&src)), // Dest should be identical to source
                ],
                ..Default::default()
            });
        }
    }
}

/// Deploy is needed due to --force-redeploy. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "cancel" on the prompt, so the sync should be stopped.
#[test]
fn needs_deploy_prompt_cancel() {
    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    let dest = file_with_modified("replace me!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$REMOTE_WINDOWS_TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--force-redeploy".to_string(),
            "--needs-deploy".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("1:.*:Cancel sync"),
        ],
        expected_exit_code: 11,
        expected_output_messages: vec![
            // We actaully get this message twice - once for the prompt and once in the error message after the prompt is cancelled
            (2, Regex::new("rjrssync needs to be deployed").unwrap()),
            (1, Regex::new(&regex::escape("Will not deploy")).unwrap()), // skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Deploy is needed due to --force-redeploy. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "deploy" on the prompt, so the sync should go ahead after deployment
#[test]
fn needs_deploy_prompt_deploy() {
    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--force-redeploy".to_string(),
            "--needs-deploy".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("1:.*:Deploy"),
        ],
        expected_exit_code: 0,
        expected_output_messages: [&[
            (1, Regex::new("rjrssync needs to be deployed").unwrap()),
            (1, Regex::new(&regex::escape("Finished release [optimized] target")).unwrap()), // Deploy and build happens
        ], &<NumActions as Into<Vec<(usize, Regex)>>>::into(copied_files(1))[..]].concat(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", Some(&src)), // Src copied to dest, as the sync went ahead after deployment
        ],
        ..Default::default()
    });
}

/// Deploy is needed due to --force-redeploy. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to produce an error.
#[test]
fn needs_deploy_error() {
    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--force-redeploy".to_string(),
            "--needs-deploy".to_string(),
            "error".to_string(),
        ],
        expected_exit_code: 11,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Will not deploy")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", None), // Unchanged
        ],
        ..Default::default()
    });
}

/// Deploy is needed due to --force-redeploy. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to deploy, so the deploy should go ahead and the sync should succeed
#[test]
fn needs_deploy_deploy() {
    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--force-redeploy".to_string(),
            "--needs-deploy".to_string(),
            "deploy".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: [&[
            (1, Regex::new(&regex::escape("Finished release [optimized] target")).unwrap()), // Deploy and build happens
        ], &<NumActions as Into<Vec<(usize, Regex)>>>::into(copied_files(1))[..]].concat(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", Some(&src)), // Src copied to dest, as the sync went ahead after deployment
        ],
        ..Default::default()
    });
}
