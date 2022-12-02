use std::time::SystemTime;

use regex::Regex;

use crate::test_framework::FilesystemNode;
#[allow(unused)]
use crate::{test_framework::{symlink_unspecified, run, empty_folder, TestDesc, symlink_file, symlink_folder, folder, file_with_modified}, folder};
use map_macro::map;

//TODO: make Windows tests work on Linux too, by falling back to generic symlink?

pub fn run_expect_success_unaware(src_node: &FilesystemNode, initial_dest_node: &FilesystemNode, 
    expected_final_dest_node: &FilesystemNode, expected_num_copies: u32) {
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", src_node),
            ("$TEMP/dest", initial_dest_node),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--symlinks".to_string(),
            "unaware".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape(&format!("Copied {} file(s)", expected_num_copies))).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(src_node)), // Source should always be unchanged
            ("$TEMP/dest", Some(expected_final_dest_node)), // Dest won't be identical to source, because symlinks have been followed.
        ],
        ..Default::default()
    });
}

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
    // Dest should get a copy of the file, rather than a symlink
    let expected_dest = folder! {
        "symlink" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
        "file.txt" => file_with_modified("contents", SystemTime::UNIX_EPOCH),
    };
    run_expect_success_unaware(&src, &empty_folder(), &expected_dest, 2);
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
    // Dest should get a copy of the folder, rather than a symlink
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
    run_expect_success_unaware(&src, &empty_folder(), &expected_dest, 4);
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
    run_expect_success_unaware(&src, &empty_folder(), &expected_dest, 4);
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

/// Tests that symlinks as ancestors of the root path are followed, regardless of the symlink mode.
#[test]
#[cfg(windows)] // folder-symlinks are only on Windows
fn test_symlink_folder_above_root() {
    let src = folder! {
        "symlink" => symlink_folder("target"),
        "target" => folder! {
            "file1.txt" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        }
    };
    for mode in ["unaware", "preserve"] {
        run(TestDesc {
            setup_filesystem_nodes: vec![
                ("$TEMP/src", &src),
            ],
            args: vec![
                "$TEMP/src/symlink/file1.txt".to_string(),
                "$TEMP/dest.txt".to_string(),
                "--symlinks".to_string(),
                mode.to_string(),
            ],
            expected_exit_code: 0,
            expected_output_messages: vec![
                Regex::new(&regex::escape(&format!("Copied {} file(s)", 1))).unwrap(),
            ],
            expected_filesystem_nodes: vec![
                ("$TEMP/src", Some(&src)), // Source should always be unchanged
                ("$TEMP/dest.txt", Some(&file_with_modified("contents1", SystemTime::UNIX_EPOCH))),
            ],
            ..Default::default()
        });
    }
}

/// Tests that specifying a root which is itself a file symlink symlink to another file,
/// when running in symlink unaware mode, will sync the contents of that pointed-to file, 
/// rather than the symlink itself.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_symlink_file_root_unaware() {
    let src = symlink_file("target.txt");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/target.txt", &file_with_modified("contents1", SystemTime::UNIX_EPOCH)),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--symlinks".to_string(),
            "unaware".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape(&format!("Copied {} file(s)", 1))).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&file_with_modified("contents1", SystemTime::UNIX_EPOCH))), // Dest won't be identical to source, because symlinks have been followed.
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a file symlink symlink to another file,
/// when running in symlink preserve mode, will sync the symlink itself rather than the pointed-to file.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_symlink_file_root_preserve() {
    let src = symlink_file("target.txt");
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/target.txt", &file_with_modified("contents1", SystemTime::UNIX_EPOCH)),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--symlinks".to_string(),
            "preserve".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be a symlink too
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a folder symlink symlink to another folder,
/// when running in symlink unaware mode, will sync the contents of that pointed-to folder, 
/// rather than the symlink itself.
#[test]
#[cfg(windows)] // folder-symlinks are only on Windows
fn test_symlink_folder_root_unaware() {
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
            "--symlinks".to_string(),
            "unaware".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape(&format!("Copied {} file(s)", 1))).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&target_folder)), // Dest will have a copy of the target folder
        ],
        ..Default::default()
    });
}

/// Tests that specifying a root which is itself a folder symlink symlink to another folder,
/// when running in symlink preserve mode, will sync the symlink itself, not the contents of the pointed-to folder.
#[test]
#[cfg(windows)] // folder-symlinks are only on Windows
fn test_symlink_folder_root_preserve() {
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
            "--symlinks".to_string(),
            "preserve".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            // We should only be copying the symlink - not any files! (this was a sneaky bug where we copy the symlink but also all the things inside it!)
            Regex::new(&regex::escape("Copied 0 file(s), created 0 folder(s) and copied 1 symlink(s)")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Source should always be unchanged
            ("$TEMP/dest", Some(&src)), // Dest should be the same symlink as the source
            ("$TEMP/target", Some(&target_folder)) // Target folder should not have been changed
        ],
        ..Default::default()
    });
}




//TODO: symlink modified time - update existing symlink with new target path if it's newer, otherwise 
// leave it alone?
//TODO: test deleting symlinks on dest side if they're no longer needed
//TODO: test cross-platform syncing - e.g. trying to create file symlink on unix, or vice versa