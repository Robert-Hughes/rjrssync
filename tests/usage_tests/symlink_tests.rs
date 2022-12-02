use std::time::SystemTime;

use regex::Regex;

#[allow(unused)]
use crate::{test_framework::{run, TestDesc, symlink_file, symlink_folder, folder, file_with_modified}, folder};
use map_macro::map;

/// Tests that syncing a folder that contains a file symlink to another file in the folder,
/// when running in symlink unaware mode, will sync the contents of the pointed-to file, 
/// rather than the symlink itself.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_symlink_file_unaware() {
    let src = folder! {
        "symlink" => symlink_file("file.txt"),
        "file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    let expected_dest = folder! {
        "symlink" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
        "file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "--symlinks".to_string(),
            "unaware".to_string(),
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 2 file(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec! [
            ("$TEMP/src", Some(&src)), // Source is unchanged (still a symlink)
            ("$TEMP/dest", Some(&expected_dest)), // Dest has a copy of the file, rather than a symlink
        ],
        ..Default::default()
    });
}

/// Tests that syncing a folder that contains a folder symlink to another folder,
/// when running in symlink unaware mode, will sync the contents of the pointed-to folder, 
/// rather than the symlink itself.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_symlink_folder_unaware() {
    let src = folder! {
        "symlink" => symlink_folder("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    let expected_dest = folder! {
        "symlink" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        },
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "--symlinks".to_string(),
            "unaware".to_string(),
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 4 file(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec! [
            ("$TEMP/src", Some(&src)), // Source is unchanged (still a symlink)
            ("$TEMP/dest", Some(&expected_dest)), // Dest has a copy of the folder, rather than a symlink
        ],
        ..Default::default()
    });
}

/// Tests that syncing a folder that contains a symlink (unspecified) to another folder,
/// when running in symlink unaware mode, will sync the contents of the pointed-to folder, 
/// rather than the symlink itself.
#[test]
#[cfg(unix)] // unspecified-symlinks are only on Unix
fn test_symlink_unspecified_unaware() {
    use crate::test_framework::symlink_unspecified;

    let src = folder! {
        "symlink" => symlink_unspecified("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    let expected_dest = folder! {
        "symlink" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        },
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
        ],
        args: vec![
            "--symlinks".to_string(),
            "unaware".to_string(),
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Copied 4 file(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec! [
            ("$TEMP/src", Some(&src)), // Source is unchanged (still a symlink)
            ("$TEMP/dest", Some(&expected_dest)), // Dest has a copy of the folder, rather than a symlink
        ],
        ..Default::default()
    });
}

//TODO: symlink modified time - update existing symlink with new target path