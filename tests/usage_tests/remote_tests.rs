use lazy_static::__Deref;
use regex::Regex;

use crate::test_framework::{run, TestDesc, empty_folder};
use crate::test_utils;

/// Tests that rjrssync can be launched on a remote platform, and communication is estabilished.
/// There is no proper sync performed (just syncing an empty folder), but this checks that
/// the ssh/scp/cargo commands and TCP connection works.
fn test_remote_launch_impl(remote_user_and_host: &str, remote_test_folder: &str) {
    // First run with --force-redeploy, to check that the remote deploying and building works,
    // even when the remote already has rjrssync set up.
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &empty_folder())
        ],
        args: vec![
            "--force-redeploy".to_string(),
            "$TEMP/src".to_string(),
            format!("{remote_user_and_host}:{remote_test_folder}")
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
            format!("{remote_user_and_host}:{remote_test_folder}")
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
    let (user_and_host, test_folder) = test_utils::REMOTE_WINDOWS_CONFIG.deref();
    test_remote_launch_impl(&user_and_host, &test_folder);
}

#[test]
fn test_remote_launch_linux() {
    let (user_and_host, test_folder) = test_utils::REMOTE_LINUX_CONFIG.deref();
    test_remote_launch_impl(&user_and_host, &test_folder);
}