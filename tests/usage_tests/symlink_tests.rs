use std::time::SystemTime;

use regex::Regex;

use crate::{test_framework::{run, TestDesc, symlink_file, folder, file_with_modified}, folder};
use map_macro::map;

/// Tests that syncing a folder that contains a file symlink to another file in the folder,
/// when running in symlink unaware mode, will sync the contents of the pointed-to file, 
/// rather than the symlink itself.
#[test]
#[cfg(windows)] // file-symlinks are only on Windows
fn test_file_symlink_unaware() {
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

//TODO: add symlink DIR (windows) and unspecified symlink (linux) variants of this test

//TODO: symlink modified time - update existing symlink with new target path