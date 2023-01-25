use std::time::{SystemTime, Duration};

use regex::Regex;

use crate::test_framework::{copied_files_and_symlinks, run_expect_success, NumActions, copied_files_folders_and_symlinks, copied_symlinks};
#[allow(unused)]
use crate::filesystem_node::*;
use crate::{test_framework::{run, TestDesc}, folder};
use map_macro::map;
use std::path::Path;

/// Tests that syncing a folder that contains a file symlink to another file in the folder,
/// will sync the symlink and not the pointed-to file.
#[test]
fn test_symlink_file() {
    let src = folder! {
        "symlink" => symlink_file("file.txt"),
        "file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src, &empty_folder(), copied_files_and_symlinks(1, 1));
}

/// Tests that syncing a folder that contains a folder symlink to another folder,
/// will sync the symlink and not the pointed-to folder.
#[test]
fn test_symlink_folder() {
    let src = folder! {
        "symlink" => symlink_folder("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run_expect_success(&src, &empty_folder(), copied_files_folders_and_symlinks(2, 1, 1));
}

/// Tests that syncing a folder that contains a broken symlink
/// will sync the symlink successfully.
#[test]
fn test_symlink_broken() {
    let src = folder! {
        "symlink" => symlink_file("target"),
    };
    run_expect_success(&src, &empty_folder(), copied_symlinks(1));
}

/// Tests that symlinks as ancestors of the root path are followed.
#[test]
fn test_symlink_folder_above_root() {
    let src = folder! {
        "symlink" => symlink_folder("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        }
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src/symlink/file1.txt".to_string(),
            "$TEMP/dest.txt".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: copied_files_and_symlinks(1, 0).into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest.txt", Some(&file_with_modified("contents1", SystemTime::UNIX_EPOCH))),
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a file symlink symlink to another file,
/// will sync the symlink itself rather than the pointed-to file.
#[test]
fn test_symlink_file_root() {
    let src = symlink_file("target.txt");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/target.txt", &file_with_modified("contents1", SystemTime::UNIX_EPOCH)),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: copied_files_and_symlinks(0, 1).into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be a symlink too
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a folder symlink to another folder,
/// will sync the symlink itself, not the contents of the pointed-to folder.
#[test]
fn test_symlink_folder_root() {
    let src = symlink_folder("target");
    let target_folder = folder! {
        "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/target", &target_folder),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
        ],
        expected_exit_code: 0,
        // We should only be copying the symlink - not any files! (this was a sneaky bug where we copy the symlink but also all the things inside it!)
        expected_output_messages: copied_files_folders_and_symlinks(0, 0, 1).into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be the same symlink as the source
            ("$TEMP/target", Some(&target_folder)) // Target folder should not have been changed
        ],
        ..Default::default()
    });
}

/// Tests that syncing a symlink that hasn't changed results in nothing being done.
#[test]
fn test_symlink_unchanged() {
    let src = folder! {
        "symlink" => symlink_file("target.txt"),
        "target.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src, &src, copied_files_and_symlinks(0, 0));
}

/// Tests that syncing a symlink that has a different target address is updated.
#[test]
fn test_symlink_new_target() {
    let src = folder! {
        "symlink" => symlink_file("target1.txt"),
        "target1.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "symlink" => symlink_file("target2.txt"),
        "target2.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src, &dest, NumActions { deleted_symlinks: 1, copied_symlinks: 1, copied_files: 1, deleted_files: 1, ..Default::default() });
}

/// Tests that syncing a symlink that has the same target address but a different kind is updated correctly.
/// Folder => File
#[test]
fn test_symlink_change_kind_folder_to_file() {
    let src = folder! {
        "symlink" => symlink_file("target"),
        "target" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "symlink" => symlink_folder("target"),
        "target" => empty_folder(),
    };
    // On Windows, the symlink will need deleting and recreating, as it's a different kind.
    #[cfg(windows)]
    let expected_actions = NumActions { deleted_folders: 1, copied_files: 1, deleted_symlinks: 1, copied_symlinks: 1, ..Default::default() };
    // On Linux though, all symlinks are the same so nothing needs doing!
    #[cfg(not(windows))]
    let expected_actions = NumActions { deleted_folders: 1, copied_files: 1, deleted_symlinks: 0, copied_symlinks: 0, ..Default::default() };

    run_expect_success(&src, &dest, expected_actions);
}

/// Tests that syncing a symlink that has the same target address but a different kind is updated correctly.
/// File => Folder
#[test]
fn test_symlink_change_kind_file_to_folder() {
    let src = folder! {
        "symlink" => symlink_folder("target"),
        "target" => empty_folder(),
    };
    let dest = folder! {
        "symlink" => symlink_file("target"),
        "target" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    // On Windows, the symlink will need deleting and recreating, as it's a different kind.
    #[cfg(windows)]
    let expected_actions = NumActions { deleted_files: 1, created_folders: 1, deleted_symlinks: 1, copied_symlinks: 1, ..Default::default() };
    // On Linux though, all symlinks are the same so nothing needs doing!
    #[cfg(not(windows))]
    let expected_actions = NumActions { deleted_files: 1, created_folders: 1, deleted_symlinks: 0, copied_symlinks: 0, ..Default::default() };

    run_expect_success(&src, &dest, expected_actions);
}

/// Tests that syncing a symlink that has the same target address but a different kind is updated correctly.
/// File => Broken
/// Folder => Broken
/// Broken => File
/// Broken => Folder
/// This test isn't relevant for Windows, because all symlinks would be file/folder.
/// There is a test further down for syncing a broken symlink from Unix to Windows (cross-platform).
#[test]
#[cfg(not(windows))]
fn test_symlink_change_kind_broken() {
    let src = folder! {
        "symlink1" => symlink_generic("target1"),
        "symlink2" => symlink_generic("target2"),
        "symlink3" => symlink_file("target3"),
        "symlink3" => symlink_folder("target4"),
        "target3" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
        "target4" => empty_folder(),
    };
    let dest = folder! {
        "symlink1" => symlink_folder("target1"),
        "symlink2" => symlink_file("target2"),
        "symlink3" => symlink_generic("target3"),
        "symlink3" => symlink_generic("target4"),
        "target1" => empty_folder(),
        "target2" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src, &dest, NumActions { deleted_files: 1, copied_files: 1,
        deleted_folders: 1, created_folders: 1,
        // No symlinks should be recreated, as they don't need to change
        deleted_symlinks: 0, copied_symlinks: 0, ..Default::default() });
}

/// Tests that an existing symlink (both directory and file) is deleted from the dest
/// when it is not present on the source side.
#[test]
fn test_symlink_delete_from_dest() {
    let src = folder! {
        "file-symlink1" => file("not a symlink!"),
        "folder-symlink1" => empty_folder(),
    };
    let dest = folder! {
        // This one should be deleted and replaced with a regular file
        "file-symlink1" => symlink_file("target-file.txt"),
        // This one should be deleted and replaced with a regular folder
        "folder-symlink1" => symlink_folder("target-folder"),
        // This one should be just deleted
        "file-symlink2" => symlink_file("target-file.txt"),
        // This one should be just deleted
        "folder-symlink2" => symlink_folder("target-folder"),

        "target-file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
        "target-folder" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run_expect_success(&src, &dest, NumActions { copied_files: 1, created_folders: 1, copied_symlinks: 0,
        deleted_files: 3, deleted_folders: 1, deleted_symlinks: 4 });
}

/// Tests that syncing a symlink as the root which is a broken symlink works.
/// This is relevant because the WalkDir crate (and our new parallel replacement) doesn't handle this.
#[test]
fn test_symlink_root_broken() {
    let src = symlink_file("target doesn't exist");
    run_expect_success(&src, &empty_folder(), NumActions { copied_symlinks: 1, deleted_folders: 1, ..Default::default() });
}

/// Tests that having a symlink file as the dest root will be replaced by the source,
/// but only the symlink itself will be deleted - the target will remain as it was.
#[test]
fn test_file_to_symlink_file_dest_root() {
    let src = file_with_modified("just a regular file", SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    let target = file_with_modified("this is the target", SystemTime::UNIX_EPOCH);
    let dest = symlink_file("target.txt");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
            ("$TEMP/target.txt", &target),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            // This test requires deleting the dest root, which we allow here. The default is to prompt
            // the user, which is covered by other tests (dest_root_needs_deleting_tests.rs)
            String::from("--dest-root-needs-deleting"),
            String::from("delete"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Copied 1 file(s)")).unwrap()),
            (1, Regex::new(&regex::escape("copied 0 symlink(s)")).unwrap()),
            (1, Regex::new("Deleted .* and 1 symlink").unwrap())
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/target.txt", Some(&target)), // Target should be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be same as source
        ],
        ..Default::default()
    });
}

/// Tests that having a symlink folder as the dest root will be replaced by the source,
/// but only the symlink itself will be deleted - the target will remain as it was.
#[test]
fn test_file_to_symlink_folder_dest_root() {
    let src = file_with_modified("just a regular file", SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    let target = folder! {
        "inside-target" => file("contents")
    };
    let dest = symlink_folder("target-folder");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
            ("$TEMP/target-folder", &target),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            // This test requires deleting the dest root, which we allow here. The default is to prompt
            // the user, which is covered by other tests (dest_root_needs_deleting_tests.rs)
            String::from("--dest-root-needs-deleting"),
            String::from("delete"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Copied 1 file(s)")).unwrap()),
            (1, Regex::new(&regex::escape("copied 0 symlink(s)")).unwrap()),
            (1, Regex::new("Deleted .* and 1 symlink").unwrap())
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/target-folder", Some(&target)), // Target should be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be same as source
        ],
        ..Default::default()
    });
}

// "Tag" these tests as they require remote platforms (GitHub Actions differentiates these)
mod remote {

use crate::{test_utils::{run_process_with_live_output, get_unique_remote_temp_folder, RemotePlatforms}};

use super::*;

/// Tests that syncing a symlink to another platform will replace the slashes in the target as appropriate.
/// Checks both a symlink file and symlink folder, to cover syncing these between platforms.
#[test]
fn test_symlink_target_slashes() {
    // (using temp_dir here is just an arbitrary way to get an absolute path)
    let abs_path = std::env::temp_dir().to_string_lossy().to_string();
    let src = folder!{
        // Start with a symlink with a relative path in the native representation.
        // Note the target must exist, so that the symlink kind can be determined when doing Linux => Windows
        "relative-symlink" => symlink_file(Path::new("..").join("src").join("..").join("src").join("dummy-target").to_str().unwrap()),
        // And a symlink with an absolute path in the native representation
        "absolute-symlink" => symlink_folder(&abs_path),
        "dummy-target" => file("contents"),
    };
    // Sync it to a remote platform (both Windows and Linux), and check that they were converted to the correct
    // representation for that platform
    let remote_platforms = RemotePlatforms::lock();
    for remote_platform in [&remote_platforms.windows, &remote_platforms.linux] {
        // The remote part of the test framework doesn't handle symlinks so well, as tar messes about
        // with the links, so we do the check manually, and so need to create our own temp folder rather
        // than using REMOTE_WINDOWS_TEMP.
        let remote_dest = get_unique_remote_temp_folder(remote_platform);

        run(TestDesc {
            setup_filesystem_nodes: vec![
                ("$TEMP/src", &src),
            ],
            args: vec![
                "$TEMP/src".to_string(),
                format!("{}:{}", remote_platform.user_and_host, remote_dest),
                "--needs-deploy".to_string(),
                "deploy".to_string(), // Skip the confirmation prompt for deploying
            ],
            expected_exit_code: 0,
            expected_output_messages: copied_files_folders_and_symlinks(1, 0, 2).into(),
            ..Default::default()
        });

        // The remote part of the test framework doesn't handle symlinks so well, as tar messes about
        // with the links, so we do the check manually.
        let mut cmd = std::process::Command::new("ssh");
        let cmd = cmd.arg(&remote_platform.user_and_host);
        let cmd = match &remote_platform.path_separator {
            '/' => cmd.arg("ls").arg("-al").arg(remote_dest),
            '\\' => cmd.arg("dir").arg(remote_dest),
            _ => panic!()
        };
        let result = run_process_with_live_output(cmd);
        assert!(result.exit_status.success());
        match &remote_platform.path_separator {
            '/' => {
                assert!(result.stdout.contains("relative-symlink -> ../src/../src/dummy-target")); // Forward slashes on the relative symlink (converted from the local representation)
                assert!(result.stdout.contains(&format!("absolute-symlink -> {abs_path}"))); // Absolute symlink has been unchanged
            }
            '\\' => {
                assert!(result.stdout.contains(r"<SYMLINK>      relative-symlink [..\src\..\src\dummy-target]")); // Backwards slashes on the relative symlink  (converted from the local representation)
                assert!(result.stdout.contains(&format!("<SYMLINKD>     absolute-symlink [{abs_path}]"))); // Absolute symlink has been unchanged
            }
            _ => panic!()
        };
    }
}

/// Tests that syncing a broken/unknown symlink from Unix to Windows raises an error as expected.
#[test]
#[cfg(unix)]
fn test_unknown_symlink_unix_to_windows() {
    let src = symlink_generic("broken!");

    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            String::from("$REMOTE_WINDOWS_TEMP/dest"),
            "--needs-deploy".to_string(),
            "deploy".to_string(), // Skip the confirmation prompt for deploying
        ],
        expected_exit_code: 12,
        expected_output_messages: vec! [
            (1, Regex::new(&regex::escape("Can't create symlink of unknown kind on this platform")).unwrap())
        ],
        ..Default::default()
    });
}

}
