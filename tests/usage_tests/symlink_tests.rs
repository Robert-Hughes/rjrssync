use std::time::SystemTime;

use regex::Regex;

use crate::test_framework::FilesystemNode;
#[allow(unused)]
use crate::{test_framework::{symlink_unspecified, run, empty_folder, TestDesc, symlink_file, symlink_folder, folder, file_with_modified}, folder};
use map_macro::map;

pub fn run_expect_success_preserve(src_node: &FilesystemNode, dest_node: &FilesystemNode, expected_num_copies: u32) {
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", src_node),
            ("$TEMP/dest", dest_node),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--symlinks".to_string(),
            "preserve".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape(&format!("Copied {} file(s)", expected_num_copies))).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(src_node)), // Source should always be unchanged
            ("$TEMP/dest", Some(src_node)), // Dest should be identical to source
        ],
        ..Default::default()
    });
}

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

/// Tests that syncing a folder that contains a file symlink to another file in the folder,
/// when running in symlink preserve mode, will sync the symlink and not the pointed-to file.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_symlink_file_preserve() {
    let src = folder! {
        "symlink" => symlink_file("file.txt"),
        "file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success_preserve(&src, &empty_folder(), 1);
}

/// Tests that syncing a folder that contains a folder symlink to another folder,
/// when running in symlink preserve mode, will sync the symlink and not the pointed-to folder.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_symlink_folder_preserve() {
    let src = folder! {
        "symlink" => symlink_folder("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run_expect_success_preserve(&src, &empty_folder(), 2);
}

/// Tests that syncing a folder that contains a symlink (unspecified) to another folder,
/// when running in symlink preserve mode,  will sync the symlink and not the pointed-to folder.
#[test]
#[cfg(unix)] // unspecified-symlinks are only on Unix
fn test_symlink_unspecified_preserve() {
    let src = folder! {
        "symlink" => symlink_unspecified("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
            "file2.txt" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        }
    };
    run_expect_success_preserve(&src, &empty_folder(), 2);
}

//TODO: symlink modified time - update existing symlink with new target path if it's newer, otherwise 
// leave it alone?
//TODO: test deleting symlinks on dest side
//TODO: test cross-platform syncing - e.g. trying to create file symlink on unix, or vice versa