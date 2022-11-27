use crate::test_framework::*;
use crate::folder;
use map_macro::map;

/// Runs a test with an optional trailing slash on the src and dest paths provided to rjrssync.
/// The expected result is either a sucess with the given number of files copied (>0), or a failure
/// if zero is given for expected_num_copies.
/// Note the slash is provided as a str rather than bool, so that it's more readable at the call-site.
/// TODO: for failures, check the error message?
fn run_trailing_slashes_test(src_node: Option<&FilesystemNode>, src_trailing_slash: &str,
    dest_node: Option<&FilesystemNode>, dest_trailing_slash: &str,
    expected_num_copies: u32
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
        src: &("$TEMP/src".to_string() + src_trailing_slash),
        dest: &("$TEMP/dest".to_string() + dest_trailing_slash),
        expected_exit_code: if expected_num_copies == 0 { 12 } else { 0 },
        expected_output_messages: 
            if expected_num_copies > 0 {
                vec![
                    format!("Copied {} file(s)", expected_num_copies),
                ] 
            } else { vec![] },
        expected_filesystem_nodes:
            if expected_num_copies > 0 {
                vec![
                    ("$TEMP/src", Some(src_node.unwrap())), // Source should always be unchanged
                    ("$TEMP/dest", Some(src_node.unwrap())), // Dest should be identical to source
                ]
            } else { 
                vec![
                    // Both src and dest should be unchanged, as the sync should have failed
                    ("$TEMP/src", src_node),
                    ("$TEMP/dest", dest_node),                
                ] 
            }, 
    });
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
    run_trailing_slashes_test(Some(&src_folder), "", Some(&empty_folder()), "", 1);
}

/// Tries syncing a folder to a folder/. This should work fine.
#[test]
fn test_folder_no_trailing_slash_to_folder_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test(Some(&src_folder), "", Some(&empty_folder()), "/", 1);
}

/// Tries syncing a folder/ to a folder. This should work fine.
#[test]
fn test_folder_trailing_slash_to_folder_no_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test(Some(&src_folder), "/", Some(&empty_folder()), "", 1);
}

/// Tries syncing a folder/ to a folder/. This should work fine.
#[test]
fn test_folder_trailing_slash_to_folder_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test(Some(&src_folder), "/", Some(&empty_folder()), "/", 1);
}

// ====================================================================================
// File => Folder with variations of trailing slashes
// ====================================================================================

/// Tries syncing a file to a folder. This should replace the folder with the file.
#[test]
fn test_file_no_trailing_slash_to_folder_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", Some(&empty_folder()), "", 1);
}

/// Tries syncing a file to a folder/. This should place the file inside the folder 
#[test]
fn test_file_no_trailing_slash_to_folder_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", Some(&empty_folder()), "/", 17); //TODO: check in new folder!
}

/// Tries syncing a file/ to a folder. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_folder_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "/", Some(&empty_folder()), "", 0);
}

/// Tries syncing a file/ to a folder/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_folder_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "/", Some(&empty_folder()), "/", 0);
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
    run_trailing_slashes_test(Some(&src_folder), "", Some(&file("contents2")), "", 1);
}

/// Tries syncing a folder to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_folder_no_trailing_slash_to_file_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test(Some(&src_folder), "", Some(&file("contents2")), "/", 0);
}

/// Tries syncing a folder/ to a file. This should replace the file with the folder.
#[test]
fn test_folder_trailing_slash_to_file_no_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test(Some(&src_folder), "/", Some(&file("contents2")), "", 1);
}

/// Tries syncing a folder/ to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_folder_trailing_slash_to_file_trailing_slash() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_trailing_slashes_test(Some(&src_folder), "/", Some(&file("contents2")), "/", 0);
}

// ====================================================================================
// File => File with variations of trailing slashes
// ====================================================================================

/// Tries syncing a file to a file. This should update dest to match src.
#[test]
fn test_file_no_trailing_slash_to_file_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", Some(&file("contents2")), "", 1);
}

/// Tries syncing a file to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_no_trailing_slash_to_file_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", Some(&file("contents2")), "/", 0);
}

/// Tries syncing a file/ to a file. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_file_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "/", Some(&file("contents2")), "", 0);
}

/// Tries syncing a file/ to a file/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_file_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "/", Some(&file("contents2")), "/", 0);
}

// ====================================================================================
// File => Non-existent with variations of trailing slashes
// ====================================================================================

/// Tries syncing a file to a non-existent path. Should create a new file.
#[test]
fn test_file_no_trailing_slash_to_non_existent_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", None, "", 1);
}

/// Tries syncing a file to a non-existent path/. This should create a new folder to put the file in.
#[test]
fn test_file_no_trailing_slash_to_non_existent_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", None, "/", 17); //TODO: check in new folder!
}

/// Tries syncing a file/ to a non-existent path. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_non_existent_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "/", None, "", 0);
}

/// Tries syncing a file/ to a non-existent path/. This should fail because trailing slashes on files are not allowed.
#[test]
fn test_file_trailing_slash_to_non_existent_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "/", None, "/", 0);
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
    run_trailing_slashes_test(Some(&src_folder), "", None, "", 1);
}

/// Tries syncing a folder to a folder/. This should work fine.
#[test]
fn test_folder_no_trailing_slash_to_non_existent_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test(Some(&src_folder), "", None, "/", 1);
}

/// Tries syncing a folder/ to a folder. This should work fine.
#[test]
fn test_folder_trailing_slash_to_non_existent_no_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test(Some(&src_folder), "/", None, "", 1);
}

/// Tries syncing a folder/ to a folder/. This should work fine.
#[test]
fn test_folder_trailing_slash_to_non_existent_trailing_slash() {
    let src_folder = folder! {
        "c1" => file("contents1"),
    };
    run_trailing_slashes_test(Some(&src_folder), "/", None, "/", 1);
}

// ====================================================================================
// Non-existent => File/Folder/Non-existent with variations of trailing slashes
// ====================================================================================

/// Tries syncing a non-existent path (with/without a trailing slash) to a variety of destinations.
/// These should all fail as can't copy something that doesn't exist.
#[test]
fn test_non_existent_to_others() {
    // => File
    run_trailing_slashes_test(None, "", Some(&file("contents")), "", 0);
    run_trailing_slashes_test(None, "", Some(&file("contents")), "/", 0);
    run_trailing_slashes_test(None, "/", Some(&file("contents")), "", 0);
    run_trailing_slashes_test(None, "/", Some(&file("contents")), "/", 0);

    // => Folder
    run_trailing_slashes_test(None, "", Some(&empty_folder()), "", 0);
    run_trailing_slashes_test(None, "", Some(&empty_folder()), "/", 0);
    run_trailing_slashes_test(None, "/", Some(&empty_folder()), "", 0);
    run_trailing_slashes_test(None, "/", Some(&empty_folder()), "/", 0);

    // => Non-existent
    run_trailing_slashes_test(None, "", None, "", 0);
    run_trailing_slashes_test(None, "", None, "/", 0);
    run_trailing_slashes_test(None, "/", None, "", 0);
    run_trailing_slashes_test(None, "/", None, "/", 0);
}
