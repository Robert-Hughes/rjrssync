use regex::Regex;

use crate::test_framework::{run, TestDesc, empty_folder};

/// The tests in this file rely on accessing "remote" hosts to test 
/// remote deploying and syncing. Therefore they require the test environment
/// to be set up (e.g. firewalls configured, remote hosts configured), and 
/// a Windows and Linux remote hostname are required.
/// One way of achieving this is to use WSL.

//TODO: make these configurable, e.g. via env var?
const REMOTE_USER_AND_HOST_WINDOWS: &str = "192.168.12.125";
const REMOTE_TEMP_FOLDER_WINDOWS: &str = "T:\\Temp\\rjrssync-tests";

const REMOTE_USER_AND_HOST_LINUX: &str = "rob@127.0.0.1";
const REMOTE_TEMP_FOLDER_LINUX: &str = "/tmp/rjrssync-tests";

/// Tests that rjrssync can be launched on a remote platform, and communication is estabilished.
/// There is no proper sync performed (just syncing an empty folder), but this checks that 
/// the ssh/scp/cargo commands and TCP connection works.
fn test_remote_launch_impl(remote_user_and_host: &str, remote_temp_folder: &str) {
    // First run with --force-redeploy, to check that the remote deploying and building works,
    // even when the remote already has rjrssync set up.
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &empty_folder())
        ],
        args: vec![
            "--force-redeploy".to_string(),
            "$TEMP/src".to_string(),
            format!("{remote_user_and_host}:{remote_temp_folder}")
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
            format!("{remote_user_and_host}:{remote_temp_folder}")
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
    test_remote_launch_impl(REMOTE_USER_AND_HOST_WINDOWS, REMOTE_TEMP_FOLDER_WINDOWS);
}

#[test]
fn test_remote_launch_linux() {
    test_remote_launch_impl(REMOTE_USER_AND_HOST_LINUX, REMOTE_TEMP_FOLDER_LINUX);
}