use std::time::{SystemTime, Duration};

use regex::Regex;

use crate::test_framework::{file, copied_files_and_symlinks, run_expect_success, NumActions, copied_files_folders_and_symlinks, copied_symlinks};
#[allow(unused)]
use crate::{test_framework::{symlink_generic, run, empty_folder, TestDesc, symlink_file, symlink_folder, folder, file_with_modified}, folder};
use map_macro::map;
use std::path::Path;

/// Tests that syncing a folder that contains a file symlink to another file in the folder,
/// when running in symlink preserve mode, will sync the symlink and not the pointed-to file.
#[test]
fn test_symlink_file() {
    let src = folder! {
        "symlink" => symlink_file("file.txt"),
        "file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src, &empty_folder(), copied_files_and_symlinks(1, 1));
}

/// Tests that syncing a folder that contains a folder symlink to another folder,
/// when running in symlink preserve mode, will sync the symlink and not the pointed-to folder.
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

/// Tests that syncing a folder that contains a symlink (unspecified) to another folder,
/// when running in symlink preserve mode,  will sync the symlink and not the pointed-to folder.
#[test]
#[cfg(unix)] // unspecified-symlinks are only on Unix
fn test_symlink_unspecified() {
    let src = folder! {
        "symlink" => symlink_generic("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run_expect_success(&src, &empty_folder(), copied_files_folders_and_symlinks(2, 1, 1));
}

/// Tests that symlinks as ancestors of the root path are followed, regardless of the symlink mode.
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
        expected_output_messages: copied_files_and_symlinks(1, 0).get_expected_output_messages(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest.txt", Some(&file_with_modified("contents1", SystemTime::UNIX_EPOCH))),
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a file symlink symlink to another file,
/// when running in symlink preserve mode, will sync the symlink itself rather than the pointed-to file.
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
        expected_output_messages: copied_files_and_symlinks(0, 1).get_expected_output_messages(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be a symlink too
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a folder symlink to another folder,
/// when running in symlink preserve mode, will sync the symlink itself, not the contents of the pointed-to folder.
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
        expected_output_messages: copied_files_folders_and_symlinks(0, 0, 1).get_expected_output_messages(),
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

/// Tests that syncing a symlink that has a different target link is updated.
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
    run_expect_success(&src, &dest, copied_files_and_symlinks(1, 1));
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

/// Tests that syncing a symlink as the root which is a broken symlink still works in preserve mode.
/// This is relevant because the WalkDir crate doesn't handle this.
#[test]
fn test_symlink_root_broken() {
    let src = symlink_file("target doesn't exist");
    run_expect_success(&src, &empty_folder(), copied_files_and_symlinks(0, 1));
}

/// Tests that having a symlink file as the dest root will be replaced by the source
/// when in preserve mode, but only the symlink itself will be deleted -
/// the target will remain as it was.
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
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap(),
            Regex::new(&regex::escape("copied 0 symlink(s)")).unwrap(),
            Regex::new("Deleted .* and 1 symlink").unwrap()
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/target.txt", Some(&target)), // Target should be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be same as source
        ],
        ..Default::default()
    });
}

/// Tests that having a symlink folder as the dest root will be replaced by the source
/// when in preserve mode, but only the symlink itself will be deleted -
/// the target will remain as it was.
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
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 1 file(s)")).unwrap(),
            Regex::new(&regex::escape("copied 0 symlink(s)")).unwrap(),
            Regex::new("Deleted .* and 1 symlink").unwrap()
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

use crate::{remote_tests::RemotePlatform, test_framework::run_process_with_live_output};

use super::*;

/// Tests that syncing a symlink to another platform will replace the slashes in the target as appropriate.
#[test]
fn test_symlink_target_relative_slashes() {
    // Start with a symlink with a relative path in the native representation.
    let src = symlink_file(Path::new("a").join("b").join("c").to_str().unwrap());
    // Sync it to a remote platform (both Windows and Linux), and check that it was converted to the correct 
    // representation for that platform
    for remote_platform in [RemotePlatform::Windows, RemotePlatform::Linux] {
        let (remote_user_and_host, remote_test_folder) = remote_platform.get_config();


        //TODO: the remote folder needs to be cleaned out first!

        run(TestDesc {
            setup_filesystem_nodes: vec![
                ("$TEMP/src", &src),
            ],
            args: vec![
                "$TEMP/src".to_string(),
                format!("{remote_user_and_host}:{remote_test_folder}")
            ],
            expected_exit_code: 0,
            expected_output_messages: copied_symlinks(1).get_expected_output_messages(),
            ..Default::default()
        });

        let mut cmd = std::process::Command::new("ssh");
        let cmd = cmd.arg(format!("{remote_user_and_host}:{remote_test_folder}"));
        let cmd = match remote_platform {            
            RemotePlatform::Linux => cmd.arg("readlink").arg(remote_test_folder),
            RemotePlatform::Windows => cmd.arg("readlink?").arg(remote_test_folder),
        };
        let result = run_process_with_live_output(cmd);
        assert!(result.exit_status.success());
        assert_eq!(result.stdout, "the/expected/link/value");
    }
}

//TODO: same as above test, but with an absolute path - probably the expected behaviour is that rjrssync
// doesn't try to modify it, cos it won't be valid anyway

}

//TODO: test cross-platform syncing - e.g. trying to create file symlink on unix, or vice versa.
//TODO: - when syncing windows to linux, the type of symlink might be different (e.g. File vs Generic), and so it would
// delete then re-create the symlink, which we might not want.

//TODO: Need to possibly replace backwards slashes with forward slashes in the link when going Windows -> Linux

