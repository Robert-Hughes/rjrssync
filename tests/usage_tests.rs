use std::{path::{Path, PathBuf}, time::{SystemTime, Duration}, collections::HashMap};

use map_macro::map;
use tempdir::TempDir;

/// Simple in-memory representation of a file or folder (including any children), to use for testing.
/// Note that this representation is consistent with the approach described in the README,
/// and so doesn't consider the name of the node to be part of the node itself.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FilesystemNode {
    Folder {
        children: HashMap<String, FilesystemNode>, // Use map rather than Vec, so that comparison of FilesystemNodes doesn't depend on order of children.
    },
    File {
        contents: Vec<u8>,     
        modified: SystemTime,
    }
}

/// Macro to ergonomically create a folder with a list of children.
/// Works by forwarding to the map! macro (see map-macro crate) to get the HashMap of children,
/// then forwarding that the `folder` function (below) which creates the actual FilesystemNode::Folder.
macro_rules! folder {
    ($($tts:tt)*) => {
        folder(map! { $($tts)* })
    }
}

fn folder(children: HashMap<&str, FilesystemNode>) -> FilesystemNode {
    // Convert to a map with owned Strings (rather than &str). We take &strs in the param
    // to make the test code simpler.
    let children : HashMap<String, FilesystemNode> = children.into_iter().map(|(n, c)| (n.to_string(), c)).collect();
    FilesystemNode::Folder{ children }
}
fn empty_folder() -> FilesystemNode {
    FilesystemNode::Folder{ children: HashMap::new() }
}
fn file(contents: &str) -> FilesystemNode {
    FilesystemNode::File{ contents: contents.as_bytes().to_vec(), modified: SystemTime::now() }       
}
fn file_with_modified(contents: &str, modified: SystemTime) -> FilesystemNode {
    FilesystemNode::File{ contents: contents.as_bytes().to_vec(), modified }       
}

/// Mirrors the given file/folder and its descendants onto disk, at the given path.
fn save_filesystem_node_to_disk(node: &FilesystemNode, path: &Path) { 
    if std::fs::metadata(path).is_ok() {
        panic!("Already exists!");
    }

    match node {
        FilesystemNode::File { contents, modified } => {
            std::fs::write(path, contents).unwrap();
            filetime::set_file_mtime(path, filetime::FileTime::from_system_time(*modified)).unwrap();
        },
        FilesystemNode::Folder { children } => {
            std::fs::create_dir(path).unwrap();
            for (child_name, child) in children {
                save_filesystem_node_to_disk(child, &path.join(child_name));
            }
        }
    }
}

/// Creates an in-memory representation of the file/folder and its descendents at the given path.
/// Returns None if the path doesn't point to anything.
fn load_filesystem_node_from_disk(path: &Path) -> Option<FilesystemNode> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return None, // Non-existent
    };

    if metadata.file_type().is_file() {
        Some(FilesystemNode::File {
            contents: std::fs::read(path).unwrap(),
            modified: metadata.modified().unwrap()
        })
    } else if metadata.file_type().is_dir() {
        let mut children = HashMap::<String, FilesystemNode>::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            children.insert(entry.file_name().to_str().unwrap().to_string(), 
                load_filesystem_node_from_disk(&path.join(entry.file_name())).unwrap());
        }        
        Some(FilesystemNode::Folder { children })
    } else {
        panic!("Unsuppoted file type");
    }
}

/// Describes a test configuration in a generic way that hopefully covers various success and failure cases.
/// This is quite verbose to use directly, so some helper functions are available that fill this in for common
/// test cases.
/// For example, this can be used to check that a sync completes successfully with a message stating
/// that some files were copied, and check that the files were in fact copied onto the filesystem.
/// All the paths provided here will have the special value $TEMP substituted for the temporary folder
/// created for placing test files in.
struct TestDesc<'a> {
    /// The given FilesystemNodes are saved to the given paths before running rjrssync 
    /// (e.g. to set up src and dest).
    setup_filesystem_nodes: Vec<(&'a str, &'a FilesystemNode)>,
    /// The value provided to rjrssync as its source path.
    /// (probably the same as a path in setup_filesystem_nodes, but may have different trailing slash for example).
    src: &'a str,
    /// The value provided to rjrssync as its dest path.
    /// (probably the same as a path in setup_filesystem_nodes, but may have different trailing slash for example).
    dest: &'a str, 
    /// The expected exit code of rjrssync (e.g. 0 for success).
    expected_exit_code: i32,
    /// Messages that are expected to be present in rjrssync's stdout/stderr
    expected_output_messages: Vec<String>, 
    /// The filesystem at the given paths are expected to be as described (including None, for non-existent)
    expected_filesystem_nodes: Vec<(&'a str, Option<&'a FilesystemNode>)>
}

/// Checks that running rjrssync with the setup described by the TestDesc behaves as described by the TestDesc.
/// See TestDesc for more details.
fn run(desc: TestDesc) {
    // Create a temporary folder to store test files/folders,
    let temp_folder = TempDir::new("rjrssync-test").unwrap();

    // All paths provided in TestDesc have $TEMP replaced with the temporary folder.
    let substitute_temp = |p: &str| PathBuf::from(p.replace("$TEMP", &temp_folder.path().to_string_lossy()));

    // Setup initial filesystem
    for (p, n) in desc.setup_filesystem_nodes {
        save_filesystem_node_to_disk(&n, &substitute_temp(&p));
    }

    // Run rjrssync with the specified paths
    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    let output = std::process::Command::new(rjrssync_path)
        .arg(substitute_temp(desc.src))
        .arg(substitute_temp(desc.dest))
        .output().expect("Failed to launch rjrssync");

    // Check exit code
    assert_eq!(output.status.code(), Some(desc.expected_exit_code));

    // Check for expected output messages
    let actual_output = String::from_utf8(output.stderr).unwrap(); //TODO: not stdout?
    for m in desc.expected_output_messages {
        assert!(actual_output.contains(&m));
    }

    // Check the filesystem is as expected afterwards
    for (p, n) in desc.expected_filesystem_nodes {
        let actual_node = load_filesystem_node_from_disk(&substitute_temp(&p));
        assert_eq!(actual_node.as_ref(), n);    
    }
}

/// Runs a test that syncs the given src FilesystemNode (e.g. file or folder) to the given dest 
/// FilesystemNode, and checks that the sync is successful, and the destination is updated to be equal
/// to the source.
fn run_expect_success(src_node: &FilesystemNode, dest_node: &FilesystemNode, expected_num_copies: u32) {
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", src_node),
            ("$TEMP/dest", dest_node),
        ],
        src: "$TEMP/src",
        dest: "$TEMP/dest",
        expected_exit_code: 0,
        expected_output_messages: vec![
            format!("Copied {} file(s)", expected_num_copies),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(src_node)), // Source should always be unchanged
            ("$TEMP/dest", Some(src_node)), // Dest should be identical to source
        ]
    });
}

/// Runs a test with an optional trailing slash on the src and dest paths provided to rjrssync.
/// The expected result is either a sucess with the given number of files copied (>0), or a failure
/// if zero is given for expected_num_copies.
/// Note the slash is provided as a str rather than bool, so that it's more readable at the call-site.
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

/// Simple folder -> folder sync
#[test]
fn test_simple_folder_sync() {
    let src_folder = folder! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder! {
            "sc" => file("contents3"),
        }
    };
    run_expect_success(&src_folder, &empty_folder(), 3);
}

/// Some files and a folder in the destination need deleting.
#[test]
fn test_remove_dest_stuff() {
    let src_folder = folder! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder! {
            "sc" => file("contents3"),
        }
    };
    let dest_folder = folder! {
        "remove me" => file("contents1"),
        "remove me too" => file("contents2"),
        "remove this whole folder" => folder! {
            "sc" => file("contents3"),
        }
    };
    run_expect_success(&src_folder, &dest_folder, 3);
}

/// A file exists but has an old timestamp so needs updating.
#[test]
fn test_update_file() {
    let src_folder = folder! {
        "file" => file_with_modified("contents1", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    let dest_folder = folder! {
        "file" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    run_expect_success(&src_folder, &dest_folder, 1);
}

/// Most files have the same timestamp so don't need updating, but one does.
#[test]
fn test_skip_unchanged() {
    let src_folder = folder! {
        "file1" => file_with_modified("contentsNEW", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "file2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "file3" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
    };
    let dest_folder = folder! {
        "file1" => file_with_modified("contentsOLD", SystemTime::UNIX_EPOCH),
        "file2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "file3" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
    };
    // Check that exactly one file was copied (the other two should have been skipped)
    run_expect_success(&src_folder, &dest_folder, 1); 
}

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

/// Tries syncing a file to a folder. This should replace the folder with the file.
#[test]
fn test_file_no_trailing_slash_to_folder_no_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", Some(&empty_folder()), "", 1);
}

/// Tries syncing a file to a folder/. This should replace the folder with the file.
#[test]
fn test_file_no_trailing_slash_to_folder_trailing_slash() {
    run_trailing_slashes_test(Some(&file("contents1")), "", Some(&empty_folder()), "/", 1);
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

// /// Tries syncing a folder to a file
// #[test]
// fn test_folder_to_file() {
//     let src_folder = folder! {
//         "file1" => file("contents"),
//     };
//     // Trailing slash variants
//     run_usage_test_impl(Some(&src_folder), "src", Some(&file("contents2")), "dest", "dest", 0, Some(1)); // dest should be replaced with src
//     run_usage_test_impl(Some(&src_folder), "src/", Some(&file("contents2")), "dest", "dest", 0, Some(1)); // dest should be replaced with src
//     run_usage_test_impl(Some(&src_folder), "src", Some(&file("contents2")), "dest/", "???", 12, None); // Can't have a trailing slash on a file
//     run_usage_test_impl(Some(&src_folder), "src/", Some(&file("contents2")), "dest/", "???", 12, None); // Can't have a trailing slash on a file
// }

// /// Tries syncing a file to a file
// #[test]
// fn test_file_to_file() {
//     // Trailing slash variants
//     run_usage_test_impl(Some(&file("contents1")), "src", Some(&file("contents2")), "dest", "dest", 0, Some(1)); // dest should be replaced with src
//     run_usage_test_impl(Some(&file("contents1")), "src/", Some(&file("contents2")), "dest", "???", 12, None); // Can't have a trailing slash on a file
//     run_usage_test_impl(Some(&file("contents1")), "src", Some(&file("contents2")), "dest/", "???", 12, None); // Can't have a trailing slash on a file
//     run_usage_test_impl(Some(&file("contents1")), "src/", Some(&file("contents2")), "dest/", "???", 12, None); // Can't have a trailing slash on a file 
// }

// /// Tries syncing a file to a non-existent path
// #[test]
// fn test_file_to_nothing() {
//     // Trailing slash variants
//     run_usage_test_impl(Some(&file("contents1")), "src", None, "dest", "dest", 0, Some(1));
//     run_usage_test_impl(Some(&file("contents1")), "src/", None, "dest", "???", 12, None); // Can't have a trailing slash on a file
//     run_usage_test_impl(Some(&file("contents1")), "src", None, "dest/", "dest/src", 0, Some(1));
//     run_usage_test_impl(Some(&file("contents1")), "src/", None, "dest/", "???", 12, None); // Can't have a trailing slash on a file
// }

// /// Tries syncing a folder to a non-existent path
// #[test]
// fn test_folder_to_nothing() {
//     let src_folder = folder! {
//         "file1" => file("contents"),
//     };
//     // Trailing slash variants - irrelevant
//     run_usage_test_impl(Some(&src_folder), "src", None, "dest", "dest", 0, Some(1));
//     run_usage_test_impl(Some(&src_folder), "src/", None, "dest", "dest", 0, Some(1));
//     run_usage_test_impl(Some(&src_folder), "src", None, "dest/", "dest", 0, Some(1));
//     run_usage_test_impl(Some(&src_folder), "src/", None, "dest/", "dest", 0, Some(1));
// }

// /// Tries syncing a non-existent path to a file
// #[test]
// fn test_nothing_to_file() {
//     // Trailing slash variants. Doesn't matter, source doesn't exist so is failure.
//     run_usage_test_impl(None, "src", Some(&file("contents")), "dest", "???", 12, None);
//     run_usage_test_impl(None, "src/", Some(&file("contents")), "dest", "???", 12, None);
//     run_usage_test_impl(None, "src", Some(&file("contents")), "dest/", "???", 12, None);
//     run_usage_test_impl(None, "src/", Some(&file("contents")), "dest/", "???", 12, None);
// }

// /// Tries syncing a non-existent path to a folder
// #[test]
// fn test_nothing_to_folder() {
//     let dest_folder = folder! {
//         "file1" => file("contents"),
//     };

//     // Trailing slash variants. Doesn't matter, source doesn't exist so is failure.
//     run_usage_test_impl(None, "src", Some(&dest_folder), "dest", "???", 12, None);
//     run_usage_test_impl(None, "src/", Some(&dest_folder), "dest", "???", 12, None);
//     run_usage_test_impl(None, "src", Some(&dest_folder), "dest/", "???", 12, None);
//     run_usage_test_impl(None, "src/", Some(&dest_folder), "dest/", "???", 12, None);
// }

// /// Tries syncing a non-existent path to a non-existent path
// #[test]
// fn test_nothing_to_nothing() {
//     // Trailing slash variants. Doesn't matter, source doesn't exist so is failure.
//     run_usage_test_impl(None, "src", None, "dest", "???", 12, None);
//     run_usage_test_impl(None, "src/", None, "dest", "???", 12, None);
//     run_usage_test_impl(None, "src", None, "dest/", "???", 12, None);
//     run_usage_test_impl(None, "src/", None, "dest/", "???", 12, None);
// }