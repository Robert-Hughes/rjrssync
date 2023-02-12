use std::time::SystemTime;

use regex::Regex;

use map_macro::map;
use crate::{test_framework::{run, TestDesc, NumActions, copied_files}, folder, test_utils::{RemotePlatforms, run_process_with_live_output, RemotePlatform, self}};
use crate::filesystem_node::*;

/// Tests that rjrssync can be launched on a remote platform, and communication is estabilished.
/// There is no proper sync performed, but this checks that
/// the binary augmentation, ssh/scp commands, TCP connection etc. works.
fn test_remote_launch_impl(remote_platforms: &RemotePlatforms, first_remote_platform: &RemotePlatform) {
    let deploy_msg = "Deploying onto";
    // First run with --deploy=force, to check that the remote deploying works,
    // even when the remote already has rjrssync set up.
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &file("blah"))
        ],
        args: vec![
            "--deploy=force".to_string(),
            "--dest-root-needs-deleting=skip".to_string(), // We sync a file to a folder, and then skip when we hit the prompt. This means that no sync is performed, but it checks the connection is good
            "$TEMP/src".to_string(),
            format!("{}:{}", first_remote_platform.user_and_host, first_remote_platform.test_folder),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape(deploy_msg)).unwrap()),
        ],
        ..Default::default()
    });

    // Then run without --deploy=force, and it should use the existing copy
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &file("blah"))
        ],
        args: vec![
            "--dest-root-needs-deleting=skip".to_string(), // We sync a file to a folder, and then skip when we hit the prompt. This means that no sync is performed, but it checks the connection is good
            "$TEMP/src".to_string(),
            format!("{}:{}", first_remote_platform.user_and_host, first_remote_platform.test_folder),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (0, Regex::new(&regex::escape(deploy_msg)).unwrap()),
        ],
        ..Default::default()
    });

    // Now make sure that the binary we deployed is also capable of deploying
    // to other platforms (i.e. that we deployed a big binary, not a lite one).

    // Initially do a simpler check with --list-embedded-binaries, because
    // our testing coverage of actually deploying from this binary is limited (see below)
    let output = run_process_with_live_output(
        std::process::Command::new("ssh")
        .arg(&first_remote_platform.user_and_host).arg(&first_remote_platform.rjrssync_path)
        .arg("--list-embedded-binaries")
    );
    assert_eq!(output.exit_status.code(), Some(0));
    // No need to check all the embedded binaries, just a couple
    assert!(output.stdout.contains("x86_64-pc-windows") && output.stdout.contains("aarch64"));

    // Coverage of exe_utils.rs functions:
    //  extract_section_from_pe (on a progenitor binary):       Yes - Deploying from Windows to Linux
    //  extract_section_from_pe (on a non-progenitor binary):   Yes - Deploying from Linux -> Windows (and then checking with --list-embedded-binaries)
    //  add_section_to_pe (on a lite binary):                   Yes - Deploying from Linux to Windows (and then checking with --list-embedded-binaries)
    //  extract_section_from_elf (on a progenitor binary):      Yes - Deploying from Linux to Windows
    //  extract_section_from_elf (on a non-progenitor binary):  Yes - Deploying from Windows -> Linux (and then checking with --list-embedded-binaries)
    //  add_section_to_elf (on a lite binary):                  Yes - Deploying from Windows to Linux (and then checking with --list-embedded-binaries)
    for second_remote_platform in &[&remote_platforms.windows, &remote_platforms.linux] {
        if *second_remote_platform == first_remote_platform {
            continue; // Don't try deploying to yourself, because then it would try to overwrite the rjrssync exe that's already running
        }

        if first_remote_platform.is_windows {
            continue; // Windows ssh seems to have a bug(?) where if we run rjrssync through ssh, it then can't ssh to something else
        }

        // We sync a file to a folder, and then skip when we hit the prompt. This means that no sync is performed, but it checks the connection is good
        let remote_command = format!("{} --dest-root-needs-deleting=skip {} {}:{} --deploy=force",
            first_remote_platform.rjrssync_path,
            first_remote_platform.rjrssync_path, // This is a file we know exists!
            second_remote_platform.user_and_host, second_remote_platform.test_folder,
        );
        // ssh (or possibly bash) will escape backslashes, messing up our paths
        let remote_command = remote_command.replace(r"\", r"\\");
        let output = run_process_with_live_output(
            std::process::Command::new("ssh")
            .arg(&first_remote_platform.user_and_host).arg(remote_command)
        );

        assert_eq!(output.exit_status.code(), Some(0));

        let actual_output = output.stderr + &output.stdout;
        assert!(actual_output.contains(deploy_msg));
    }
}

#[test]
fn test_remote_launch_windows() {
    let remote_platforms = RemotePlatforms::lock();
    test_remote_launch_impl(&remote_platforms, &remote_platforms.windows);
}

#[test]
fn test_remote_launch_linux() {
    let remote_platforms = RemotePlatforms::lock();
    test_remote_launch_impl(&remote_platforms, &remote_platforms.linux);
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
                    "--deploy=ok".to_string(),  // Skip the confirmation prompt for deploying
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

/// Deploy is needed due to missing on remote target.
/// The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "cancel" on the prompt, so the sync should be stopped.
#[test]
fn needs_deploy_prompt_cancel() {
    // Delete rjrssync on the remote, so that a deploy is required
    let remote_platforms = RemotePlatforms::lock();
    test_utils::delete_remote_file(&remote_platforms.windows.rjrssync_path, &remote_platforms.windows);

    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    let dest = file_with_modified("replace me!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        remote_platforms: Some(&remote_platforms),
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$REMOTE_WINDOWS_TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--deploy=prompt".to_string(),
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

/// Deploy is needed due to missing on remote target.
/// The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "deploy" on the prompt, so the sync should go ahead after deployment
#[test]
fn needs_deploy_prompt_deploy() {
    // Delete rjrssync on the remote, so that a deploy is required
    let remote_platforms = RemotePlatforms::lock();
    test_utils::delete_remote_file(&remote_platforms.windows.rjrssync_path, &remote_platforms.windows);

    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        remote_platforms: Some(&remote_platforms),
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--deploy=prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("1:.*:Deploy"),
        ],
        expected_exit_code: 0,
        expected_output_messages: [&[
            (1, Regex::new("rjrssync needs to be deployed").unwrap()),
            (1, Regex::new(&regex::escape("Deploying onto")).unwrap()), // Deploy and build happens
        ], &<NumActions as Into<Vec<(usize, Regex)>>>::into(copied_files(1))[..]].concat(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", Some(&src)), // Src copied to dest, as the sync went ahead after deployment
        ],
        ..Default::default()
    });
}

/// Deploy is needed due to missing on remote target.
/// The expected behaviour is controlled by a command-line argument, which in this case
/// we set to produce an error.
#[test]
fn needs_deploy_error() {
    // Delete rjrssync on the remote, so that a deploy is required
    let remote_platforms = RemotePlatforms::lock();
    test_utils::delete_remote_file(&remote_platforms.windows.rjrssync_path, &remote_platforms.windows);

    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        remote_platforms: Some(&remote_platforms),
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--deploy=error".to_string(),
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

/// Deploy is needed due to missing on remote target.
/// The expected behaviour is controlled by a command-line argument, which in this case
/// we set to ok, so the deploy should go ahead and the sync should succeed
#[test]
fn needs_deploy_ok() {
    // Delete rjrssync on the remote, so that a deploy is required
    let remote_platforms = RemotePlatforms::lock();
    test_utils::delete_remote_file(&remote_platforms.windows.rjrssync_path, &remote_platforms.windows);

    let src = file_with_modified("this will replace the dest!", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        remote_platforms: Some(&remote_platforms),
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--deploy=ok".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: [&[
            (1, Regex::new(&regex::escape("Deploying onto")).unwrap()), // Deploy and build happens
        ], &<NumActions as Into<Vec<(usize, Regex)>>>::into(copied_files(1))[..]].concat(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", Some(&src)), // Src copied to dest, as the sync went ahead after deployment
        ],
        ..Default::default()
    });
}

/// Tests that the --remote-port option works.
#[test]
fn remote_port() {
    let src = file_with_modified("something to sync", SystemTime::UNIX_EPOCH);
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$REMOTE_WINDOWS_TEMP/dest".to_string(),
            "--deploy=ok".to_string(),
            "--verbose".to_string(), // So that we can check the port number in the logs
            "--remote-port=1234".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: [&[
            (2, Regex::new("Waiting for incoming network connection on port 1234").unwrap()),
            (1, Regex::new("Connecting to doer over network at .*1234").unwrap()),
        ], &<NumActions as Into<Vec<(usize, Regex)>>>::into(copied_files(1))[..]].concat(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$REMOTE_WINDOWS_TEMP/dest", Some(&src)), // Src copied to dest
        ],
        ..Default::default()
    });
}
