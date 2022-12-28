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
            // Some tests require deleting the dest root, which we allow here. The default is to prompt
            // the user, which is covered by other tests (dest_root_needs_deleting_tests.rs)
            String::from("--dest-root-needs-deleting"), 
            String::from("delete"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            if matches!(src_node, Some(FilesystemNode::Symlink { .. })) {
                (1, Regex::new(&regex::escape(&format!("copied {} symlink(s)", expected_num_copies))).unwrap())
            } else {
                (1, Regex::new(&regex::escape(&format!("Copied {} file(s)", expected_num_copies))).unwrap())
            }
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
            (1, expected_error),
        ],
        expected_filesystem_nodes: vec![
            // Both src and dest should be unchanged, as the sync should have failed
            ("$TEMP/src", src_node),
            ("$TEMP/dest", dest_node),                
        ],
        ..Default::default()
    });
}

// In some environments (e.g. Linux), a file with a trailing slash is caught on the doer side when it attempts to
// get the metadata for the root, but on some environments it isn't caught (Windows, depending on the drive)
// so we do our own additional check, so the error message could be either. Note that different versions of Windows 
// seem to report this differently (observed different behaviour locally vs on GitHub Actions).
fn get_file_trailing_slash_error() -> Regex {
    return Regex::new("(is a file or symlink but is referred to with a trailing slash)|(can't be read)").unwrap();
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

// ====================================================================================
// Symlinks with trailing slashes - these are treated the same as files (i.e. no trailing slash allowed), no matter their kind,
//   however there is a quirk with Linux and symlink folders.
// ====================================================================================

/// On Windows, any trailing slash on a symlink is an error.
#[test]
#[cfg(windows)]
fn test_symlinks_broken_targets() {
    run_trailing_slashes_test_expected_failure(Some(&symlink_file("hello")), "/", Some(&file("contents")), "", get_file_trailing_slash_error());
    run_trailing_slashes_test_expected_failure(Some(&file("hello")), "", Some(&symlink_file("hello")), "/", get_file_trailing_slash_error());
    run_trailing_slashes_test_expected_failure(Some(&symlink_file("hello")), "/", Some(&symlink_file("hello")), "/", get_file_trailing_slash_error());

    run_trailing_slashes_test_expected_failure(Some(&symlink_folder("hello")), "/", Some(&file("contents")), "", get_file_trailing_slash_error());
    run_trailing_slashes_test_expected_failure(Some(&file("hello")), "", Some(&symlink_folder("hello")), "/", get_file_trailing_slash_error());
    run_trailing_slashes_test_expected_failure(Some(&symlink_folder("hello")), "/", Some(&symlink_folder("hello")), "/", get_file_trailing_slash_error());
}

/// But on Linux, trailing slashes on symlinks mean to _follow_ the symlink (this is OS behaviour that we don't
/// really want to override). There is therefore different behaviour for valid or invalid symlinks.
#[test]
#[cfg(unix)]
fn test_symlinks_broken_targets() {
    run_trailing_slashes_test_expected_failure(Some(&symlink_file("hello")), "/", Some(&file("contents")), "", 
        Regex::new("src path .* doesn't exist").unwrap());
    // This is a bit of a weird one - the destination has a trailing slash, so Linux interprets it as the symlink target,
    // which doesn't exist, so we treat the dest as non-existent. Because it has a trailing slash though and the source is a file,
    // we assume that the user intends to copy the file into a new folder on the destination side, so we try to create that folder.
    // However this fails, because the dest symlink is already there!
    run_trailing_slashes_test_expected_failure(Some(&file("hello")), "", Some(&symlink_file("hello")), "/", 
        Regex::new("Error creating folder and ancestors .* File exists").unwrap());

    run_trailing_slashes_test_expected_failure(Some(&symlink_folder("hello")), "/", Some(&file("contents")), "",
        Regex::new("src path .* doesn't exist").unwrap());
    
    // Same weirdness as above
    run_trailing_slashes_test_expected_failure(Some(&file("hello")), "", Some(&symlink_folder("hello")), "/",
        Regex::new("Error creating folder and ancestors .* File exists").unwrap());
}

/// There is different behaviour for valid or invalid symlinks.
#[test]
#[cfg(unix)]
fn test_symlinks_valid_targets() {
    // symlink files with trailing slashes will fail to be read by the OS (not non-existent, but an error about it not being a directory)
    let existing_file = std::env::current_exe().unwrap().to_string_lossy().to_string(); // an arbitrary extant file
    run_trailing_slashes_test_expected_failure(Some(&symlink_file(&existing_file)), "/", Some(&file("contents")), "", get_file_trailing_slash_error());
    run_trailing_slashes_test_expected_failure(Some(&file("hello")), "", Some(&symlink_file(&existing_file)), "/", get_file_trailing_slash_error());
    run_trailing_slashes_test_expected_failure(Some(&symlink_file(&existing_file)), "/", Some(&symlink_file(&existing_file)), "/", get_file_trailing_slash_error());

    // symlink folders with trailing slashes though will be interpreted as the destination itself, and so should actually work fine
    // without any validation error.
    // Therefore we need to set up some valid target folders to be synced.

    // Trailing slash on source symlink folder only, so the source link is followed
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &symlink_folder("target")),
            ("$TEMP/target", &folder! {
                "file" => file_with_modified("hello", SystemTime::UNIX_EPOCH)
            }),
        ],
        args: vec![
            "$TEMP/src/".to_string(), // With trailing slash
            "$TEMP/dest".to_string() // No trailing slash
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&symlink_folder("target"))), // src unchanged
            ("$TEMP/target", Some(&folder! { // target unchanged
                "file" => file_with_modified("hello", SystemTime::UNIX_EPOCH)
            })), 
            ("$TEMP/dest", Some(&folder! { // dest is a folder containing the file 
                "file" => file_with_modified("hello", SystemTime::UNIX_EPOCH)
            })), 
        ],
        ..Default::default()
    }.with_expected_actions(copied_files_and_folders(1, 1)));

    // Trailing slash on dest symlink folder only, so the dest link is followed
    // Because the source is a file, it will be placed into the target folder rather than overwriting it
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &file_with_modified("src file", SystemTime::UNIX_EPOCH)),
            ("$TEMP/dest", &symlink_folder("target")),
            ("$TEMP/target", &folder! {
                "file" => file_with_modified("hello", SystemTime::UNIX_EPOCH)
            }),
        ],
        args: vec![
            "$TEMP/src".to_string(), // No trailing slash
            "$TEMP/dest/".to_string() // With trailing slash
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&file_with_modified("src file", SystemTime::UNIX_EPOCH))), // src unchanged
            ("$TEMP/target", Some(&folder! { // target folder has the new source file in it
                "file" => file_with_modified("hello", SystemTime::UNIX_EPOCH),
                "src" => file_with_modified("src file", SystemTime::UNIX_EPOCH)
            })), 
            ("$TEMP/dest", Some(&symlink_folder("target"))), // dest is still a symlink
        ],
        ..Default::default()
    }.with_expected_actions(copied_files(1)));

    // Trailing slash on both source and dest symlink folders, so both links are followed and 
    // the contents of the symlink target folders are synced.
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &symlink_folder("target1")),
            ("$TEMP/dest", &symlink_folder("target2")),
            ("$TEMP/target1", &folder! {
                "src_file" => file_with_modified("hello1", SystemTime::UNIX_EPOCH)
            }),
            ("$TEMP/target2", &folder! {
                "dest_file" => file_with_modified("hello2", SystemTime::UNIX_EPOCH)
            }),
        ],
        args: vec![
            "$TEMP/src/".to_string(), // With trailing slash
            "$TEMP/dest/".to_string() // With trailing slash
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&symlink_folder("target1"))), // src unchanged
            ("$TEMP/dest", Some(&symlink_folder("target2"))), // dest is still a symlink
            ("$TEMP/target2", Some(&folder! { // dest target folder is updated
                "src_file" => file_with_modified("hello1", SystemTime::UNIX_EPOCH)
            })), 
        ],
        ..Default::default()
    }.with_expected_actions(NumActions { deleted_files: 1, copied_files: 1, ..Default::default() }));
}

// ====================================================================================
// Symlinks without trailing slashes - these are treated the same as files, no matter their kind
// ====================================================================================

/// Tries syncing a symlink to a folder. This should replace the folder with the symlink.
#[test]
fn test_symlink_no_trailing_slash_to_folder_no_trailing_slash() {
    run_trailing_slashes_test_expect_success(Some(&symlink_file("target1")), "", Some(&empty_folder()), "", 1);
}

/// Tries syncing a symlink to a folder/. This should place the symlink inside the folder 
#[test]
fn test_symlink_no_trailing_slash_to_folder_trailing_slash() {
    run_trailing_slashes_test_expect_success_override_dest(Some(&symlink_file("target")), "", Some(&empty_folder()), "/", 1, "$TEMP/dest/src");
}

/// Tries syncing a folder to a symlink. This should replace the symlink with the folder.
#[test]
fn test_folder_no_trailing_slash_to_symlink_no_trailing_slash() {
    let src_folder = folder! {
        "file" => file("contents"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "", Some(&symlink_file("target2")), "", 1);
}

/// Tries syncing a folder/ to a symlink. This should replace the symlink with the folder.
#[test]
fn test_folder_trailing_slash_to_symlink_no_trailing_slash() {
    let src_folder = folder! {
        "file" => file("contents"),
    };
    run_trailing_slashes_test_expect_success(Some(&src_folder), "/", Some(&symlink_file("target2")), "", 1);
}

/// Tries syncing a symlink to a non-existent path. Should create a new symlink.
#[test]
fn test_symlink_no_trailing_slash_to_non_existent_no_trailing_slash() {
    run_trailing_slashes_test_expect_success(Some(&symlink_file("target1")), "", None, "", 1);
}

/// Tries syncing a symlink to a non-existent path/. This should create a new folder to put the symlink in.
#[test]
fn test_symlink_no_trailing_slash_to_non_existent_trailing_slash() {
    run_trailing_slashes_test_expect_success_override_dest(Some(&symlink_file("target")), "", None, "/", 1, "$TEMP/dest/src");
}

