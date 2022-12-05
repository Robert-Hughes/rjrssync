use network_interface::NetworkInterface;
use network_interface::NetworkInterfaceConfig;
use network_interface::V4IfAddr;
use network_interface::Addr::V4;
use regex::Regex;

use crate::test_framework::{run, TestDesc, empty_folder};

/// The tests in this file rely on accessing "remote" hosts to test
/// remote deploying and syncing. Therefore they require the test environment
/// to be set up (e.g. firewalls configured, remote hosts configured), and
/// a Windows and Linux remote hostname are required.
/// One way of achieving this is to use WSL.

/// Gets the remote host configuration to use for remote Windows tests.
/// This can come from environment variables specified by the user, or if not specified,
/// a default is returned assuming a WSL setup.
fn get_remote_windows_config() -> (String, String) {
    let user_and_host = match std::env::var("RJRSSYNC_TEST_REMOTE_USER_AND_HOST_WINDOWS") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => {
            if cfg!(windows) {
                // We want to simply connect to the current OS, but using localhost or 127.0.0.1 won't
                // work if SSH on WSL is also listening on the same port, as that takes precedence.
                // Instead we need to find another IP to refer to the current OS.
                NetworkInterface::show().expect("Error getting network interfaces").into_iter()
                    .filter_map(|i| i.addr.and_then(|a| if let V4(V4IfAddr { ip, .. }) = a { Some(ip.to_string()) } else { None }))
                    .filter(|a| a != "127.0.0.1").nth(0).expect("No appropriate network interfaces")
            } else if cfg!(unix) {
                // Figure out the IP address of the external host windows system from /etc/resolv.conf
                let windows_ip = std::fs::read_to_string("/etc/resolv.conf").expect("Failed to read /etc/resolv.conf")
                    .lines().filter_map(|l| l.split("nameserver ").last()).last().expect("Couldn't find nameserver in /etc/resolv.conf").to_string();

                // Get windows username
                // Note the full path to cmd.exe need to be used when running on GitHub actions (cmd.exe is not enough)
                let output = std::process::Command::new("/mnt/c/Windows/system32/cmd.exe").arg("/c").arg("echo %USERNAME%").output().expect("Failed to query windows username");
                assert!(output.status.success());
                let username = String::from_utf8(output.stdout).expect("Unable to decode utf-8").trim().to_string();
          
                format!("{username}@{windows_ip}")
            } else {
                panic!("Not implemented for this OS" );
            }
        }
        _ => panic!("Unexpected error"),
    };
    println!("Windows remote user and host: {user_and_host}");

    // Confirm that we can connect to this remote host, to help debugging the test environment
    confirm_remote_test_environment(&user_and_host, "Windows");

    let test_folder = match std::env::var("RJRSSYNC_TEST_REMOTE_TEST_FOLDER_WINDOWS") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => {
            // Figure out the remote temp dir, based on the remote environment variable %TEMP%
            let output = std::process::Command::new("ssh").arg(&user_and_host).arg("echo %TEMP%\\rjrssync-tests").output().expect("Failed to query remote temp folder");
            assert!(output.status.success());
            String::from_utf8(output.stdout).expect("Unable to decode utf-8").trim().to_string()
        }
        _ => panic!("Unexpected error"),
    };
    println!("Windows remote test folder: {test_folder}");
    
    (user_and_host, test_folder)
}

/// Gets the remote host configuration to use for remote Linux tests.
/// This can come from environment variables specified by the user, or if not specified,
/// a default is returned assuming a WSL setup.
fn get_remote_linux_config() -> (String, String) {
    let user_and_host = match std::env::var("RJRSSYNC_TEST_REMOTE_USER_AND_HOST_LINUX") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => {
            if cfg!(windows) {
                // We want to connect to the WSL instance which we assume is running, which can be done 
                // by simply using localhost or 127.0.0.1. If both WSL SSH and windows SSH are both listening,
                // then WSL takes precedence.
                // The username is more complicated, as the WSL username might differ from Windows username
                //TODO: running this command messes up terminal line endings briefly :(
                let output = std::process::Command::new("wsl").arg("echo").arg("$USER").output().expect("Failed to query WSL username");
                assert!(output.status.success());
                let username = String::from_utf8(output.stdout).expect("Unable to decode utf-8").trim().to_string();
                   
                format!("{username}@127.0.0.1")
            } else if cfg!(unix) {
                // Simply connect to the current OS, with the current user
                "127.0.0.1".to_string()
            } else {
                panic!("Not implemented for this OS" );
            }
        }
        _ => panic!("Unexpected error"),
    };
    println!("Linux remote user and host: {user_and_host}");

    // Confirm that we can connect to this remote host, to help debugging the test environment
    confirm_remote_test_environment(&user_and_host, "Linux");

    let test_folder = match std::env::var("RJRSSYNC_TEST_REMOTE_TEST_FOLDER_LINUX") {
        Ok(x) => x,
        Err(std::env::VarError::NotPresent) => "/tmp/rjrssync-tests".to_string(),
        _ => panic!("Unexpected error"),
    };
    println!("Linux remote test folder: {test_folder}");
    
    (user_and_host, test_folder)
}

fn confirm_remote_test_environment(remote_user_and_host: &str, expected_os: &str) {
    // Confirm that we can connect to this remote host, to help debugging the test environment
    let test_command = match expected_os {
        "Windows" => "echo Remote host is working && ver",
        "Linux" => "echo Remote host is working && uname -a",
        _ => panic!("Unexpected OS"),
    };

    println!("Checking connection to {} with ssh command '{}'", remote_user_and_host, test_command);
    let output = std::process::Command::new("ssh").arg(remote_user_and_host).arg(test_command)
        .output().expect("Failed to check if remote host is available");
    println!("ssh exit code: {}", output.status);
    println!("ssh stdout:");
    let stdout_text = String::from_utf8(output.stdout).expect("Unable to decode utf-8");
    println!("{}", stdout_text);
    println!("ssh stderr:");
    println!("{}", String::from_utf8(output.stderr).expect("Unable to decode utf-8"));

    assert!(output.status.success());
    assert!(stdout_text.contains(expected_os));
}


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
    let (user_and_host, test_folder) = get_remote_windows_config();
    test_remote_launch_impl(&user_and_host, &test_folder);
}

#[test]
fn test_remote_launch_linux() {
    let (user_and_host, test_folder) = get_remote_linux_config();
    test_remote_launch_impl(&user_and_host, &test_folder);
}