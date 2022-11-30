use std::time::Duration;
use std::time::SystemTime;

use crate::test_framework::*;
use crate::folder;
use map_macro::map;
use regex::Regex;

/// Runs a test with an optional trailing slash on the src and dest paths provided to rjrssync.
/// The expected result is sucess with the given number of files copied.
/// Note the slash is provided as a str rather than bool, so that it's more readable at the call-site.
fn run_trailing_slashes_test_expect_success(src_node: Option<&FilesystemNode>, src_trailing_slash: &str,
    dest_node: Option<&FilesystemNode>, dest_trailing_slash: &str,
    expected_num_copies: u32
) {
    run_trailing_slashes_test_expect_success_override_dest(src_node, src_trailing_slash, dest_node, dest_trailing_slash,
        expected_num_copies, "$TEMP/dest");
}

fn run_trailing_slashes_test_expect_success_override_dest(src_node: Option<&FilesystemNode>, src_trailing_slash: &str,
    dest_node: Option<&FilesystemNode>, dest_trailing_slash: &str,
    expected_num_copies: u32, override_dest: &str,
) {
    let mut setup_filesystem_nodes = vec![];
    if let Some(n) = src_node {
        setup_filesystem_nodes.push(("$TEMP/src", n)); // Note no trailing slash here, as this is just to set up the filesystem, not to run rjrssync
    }
    if let Some(n) = dest_node {
        setup_filesystem_nodes.push(("$TEMP/dest", n));  // Note no trailing slash here, as this is just to set up the filesystem, not to run rjrssync
    }
    run(TestDesc {
        setup_filesystem_nodes,
        args: vec![
            "$TEMP/src".to_string() + src_trailing_slash,
            "$TEMP/dest".to_string() + dest_trailing_slash,
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape(&format!("Copied {} file(s)", expected_num_copies))).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(src_node.unwrap())), // Source should always be unchanged
            (override_dest, Some(src_node.unwrap())), // Dest should be identical to source
        ],
        ..Default::default()
    });
}

/// Runs a test with an optional trailing slash on the src and dest paths provided to rjrssync.
/// The expected result is a failure with the given error message.
/// Note the slash is provided as a str rather than bool, so that it's more readable at the call-site.
fn run_trailing_slashes_test_expected_failure(src_node: Option<&FilesystemNode>, src_trailing_slash: &str,
    dest_node: Option<&FilesystemNode>, dest_trailing_slash: &str,
    expected_error: Regex
) {
    let mut setup_filesystem_nodes = vec![];
    if let Some(n) = src_node {
        setup_filesystem_nodes.push(("$TEMP/src", n)); // Note no trailing slash here, as this is just to set up the filesystem, not to run rjrssync
    }
    if let Some(n) = dest_node {
        setup_filesystem_nodes.push(("$TEMP/dest", n));  // Note no trailing slash here, as this is just to set up the filesystem, not to run rjrssync
    }
    run(TestDesc {
        setup_filesystem_nodes,
        args: vec![
            "$TEMP/src".to_string() + src_trailing_slash,
            "$TEMP/dest".to_string() + dest_trailing_slash,
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            expected_error,
        ],
        expected_filesystem_nodes: vec![
            // Both src and dest should be unchanged, as the sync should have failed
            ("$TEMP/src", src_node),
            ("$TEMP/dest", dest_node),                
        ],
        ..Default::default()
    });
}

// In some environments (e.g. Linux), a file with a trailing slash  is caught on the doer side when it attempts to
// get the metadata for the root, but on some environments it isn't caught (Windows, depending on the drive)
// so do our own check here, so the error message could be either.
fn get_file_trailing_slash_error() -> Regex {
    return Regex::new("(is a file but is referred to with a trailing slash)|(can't be read: Not a directory)").unwrap();
}

// ====================================================================================
// Folder => Folder with variations of trailing slashes
// ====================================================================================

/// Tries syncing a folder to a folder. This should work fine.
#[test]
fn test_folder_no_trailing_slash_to_folder_no_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "", Some(&empty_folder()), "", 1);
}

/// Tries syncing a folder to a folder/. This should work fine.
#[test]
fn test_folder_no_trailing_slash_to_folder_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "", Some(&empty_folder()), "/", 1);
}

/// Tries syncing a folder/ to a folder. This should work fine.
#[test]
fn test_folder_trailing_slash_to_folder_no_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "/", Some(&empty_folder()), "", 1);
}

/// Tries syncing a folder/ to a folder/. This should work fine.
#[test]
fn test_folder_trailing_slash_to_folder_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "/", Some(&empty_folder()), "/", 1);
}

// ====================================================================================
// File => Folder with variations of trailing slashes
// ====================================================================================

/// Tries syncing a file to a folder. This should replace the folder with the file.
#[test]
fn test_file_no_trailing_slash_to_folder_no_trailing_slash() {
    run_trailing_slashes_test_expect_success(Some(&file("contents1")), "", Some(&empty_folder()), "", 1);
}

/// Tries syncing a file to a folder/. This should place the file inside the folder 
#[test]
fn test_file_no_trailing_slash_to_folder_trailing_slash() {
    run_trailing_slashes_test_expect_success_override_dest(Some(&file("contents1")), "", Some(&empty_folder()), "/", 1, "$TEMP/dest/src");
}

/// Tries syncing a file/ to a folder. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_folder_no_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "/", Some(&empty_folder()), "", get_file_trailing_slash_error());
}

/// Tries syncing a file/ to a folder/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_folder_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "/", Some(&empty_folder()), "/", get_file_trailing_slash_error());
}

// ====================================================================================
// Folder => File with variations of trailing slashes
// ====================================================================================

/// Tries syncing a folder to a file. This should replace the file with the folder.
#[test]
fn test_folder_no_trailing_slash_to_file_no_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "", Some(&file("contents2")), "", 1);
}

/// Tries syncing a folder to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_folder_no_trailing_slash_to_file_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test_expected_failure(Some(&src_folder), "", Some(&file("contents2")), "/", get_file_trailing_slash_error());
}

/// Tries syncing a folder/ to a file. This should replace the file with the folder.
#[test]
fn test_folder_trailing_slash_to_file_no_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "/", Some(&file("contents2")), "", 1);
}

/// Tries syncing a folder/ to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_folder_trailing_slash_to_file_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test_expected_failure(Some(&src_folder), "/", Some(&file("contents2")), "/", get_file_trailing_slash_error());
}

// ====================================================================================
// File => File with variations of trailing slashes
// ====================================================================================

/// Tries syncing a file to a file. This should update dest to match src.
#[test]
fn test_file_no_trailing_slash_to_file_no_trailing_slash() {
    run_trailing_slashes_test_expect_success(
        Some(&file_with_modified("contents1", SystemTime::UNIX_EPOCH + Duration::from_secs(1))), "", 
        Some(&file_with_modified("contents2", SystemTime::UNIX_EPOCH)), "", 
        1);
}

/// Tries syncing a file to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_no_trailing_slash_to_file_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "", Some(&file("contents2")), "/", get_file_trailing_slash_error());
}

/// Tries syncing a file/ to a file. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_file_no_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "/", Some(&file("contents2")), "", get_file_trailing_slash_error());
}

/// Tries syncing a file/ to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_file_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "/", Some(&file("contents2")), "/", get_file_trailing_slash_error());
}

// ====================================================================================
// File => Non-existent with variations of trailing slashes
// ====================================================================================

/// Tries syncing a file to a non-existent path. Should create a new file.
#[test]
fn test_file_no_trailing_slash_to_non_existent_no_trailing_slash() {
    run_trailing_slashes_test_expect_success(Some(&file("contents1")), "", None, "", 1);
}

/// Tries syncing a file to a non-existent path/. This should create a new folder to put the file in.
#[test]
fn test_file_no_trailing_slash_to_non_existent_trailing_slash() {
    run_trailing_slashes_test_expect_success_override_dest(Some(&file("contents1")), "", None, "/", 1, "$TEMP/dest/src");
}

/// Tries syncing a file/ to a non-existent path. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_non_existent_no_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "/", None, "", get_file_trailing_slash_error());
}

/// Tries syncing a file/ to a non-existent path/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_non_existent_trailing_slash() {
    run_trailing_slashes_test_expected_failure(Some(&file("contents1")), "/", None, "/", get_file_trailing_slash_error());
}

// ====================================================================================
// Folder => Non-existent with variations of trailing slashes
// ====================================================================================

/// Tries syncing a folder to a folder. This should work fine.
#[test]
fn test_folder_no_trailing_slash_to_non_existent_no_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "", None, "", 1);
}

/// Tries syncing a folder to a folder/. This should work fine.
#[test]
fn test_folder_no_trailing_slash_to_non_existent_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "", None, "/", 1);
}

/// Tries syncing a folder/ to a folder. This should work fine.
#[test]
fn test_folder_trailing_slash_to_non_existent_no_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "/", None, "", 1);
}

/// Tries syncing a folder/ to a folder/. This should work fine.
#[test]
fn test_folder_trailing_slash_to_non_existent_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "/", None, "/", 1);
}

// ====================================================================================
// Non-existent => File/Folder/Non-existent with variations of trailing slashes
// ====================================================================================

/// Tries syncing a non-existent path (with/without a trailing slash) to a variety of destinations.
/// These should all fail as can't copy something that doesn't exist.
#[test]
fn test_non_existent_to_others() {
    // => File
    run_trailing_slashes_test_expected_failure(None, "", Some(&file("contents")), "", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "", Some(&file("contents")), "/", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "/", Some(&file("contents")), "", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "/", Some(&file("contents")), "/", Regex::new("doesn't exist").unwrap());

    // => Folder
    run_trailing_slashes_test_expected_failure(None, "", Some(&empty_folder()), "", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "", Some(&empty_folder()), "/", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "/", Some(&empty_folder()), "", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "/", Some(&empty_folder()), "/", Regex::new("doesn't exist").unwrap());

    // => Non-existent
    run_trailing_slashes_test_expected_failure(None, "", None, "", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "", None, "/", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "/", None, "", Regex::new("doesn't exist").unwrap());
    run_trailing_slashes_test_expected_failure(None, "/", None, "/", Regex::new("doesn't exist").unwrap());
}
