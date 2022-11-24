use std::{path::Path, time::{SystemTime, Duration}, collections::HashMap};

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

/// Macro to ergonomically create a folder.
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

fn run_usage_test(src_node: FilesystemNode, dest_node: FilesystemNode, 
    expected_num_copies: Option<u32>) {
        run_usage_test_impl(Some(src_node), Some(dest_node), 0, expected_num_copies);
}

fn run_usage_test_expect_failure(src_node: FilesystemNode, dest_node: FilesystemNode, 
    expected_exit_code: i32) {
        run_usage_test_impl(Some(src_node), Some(dest_node), expected_exit_code, None);
}

/// Checks that running rjrssync with src and dest arguments pointing to the specified files/folders
/// (or None to indicate a path to something non-existent) has the expected exit code.
/// If successful, it checks that dest ends up identical to the src (including any children),
/// otherwise it checks that the dest is unchanged (this probably won't be true for all tests...).
/// Optionally also tests that the expected number of files were copied.
fn run_usage_test_impl(src_node: Option<FilesystemNode>, dest_node: Option<FilesystemNode>, 
    expected_exit_code: i32, expected_num_copies: Option<u32>) {
    // Create temporary folders to store the src and dest nodes,
    // then place the src and dest files/folders into these holder folders.
    // The names of the src/dest nodes are unimportant (they are not part of the definition of a node),
    // so we can choose anything here.

    let src_holder = TempDir::new("rjrssync-test").unwrap();
    let src = src_holder.path().join("src");
    if let Some(t) = &src_node {
        save_filesystem_node_to_disk(&t, &src);
    }

    let dest_holder = TempDir::new("rjrssync-test").unwrap();
    let dest = dest_holder.path().join("dest");
    if let Some(t) = &dest_node {
        save_filesystem_node_to_disk(&t, &dest);
    }

    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    let output = std::process::Command::new(rjrssync_path)
        .arg(&src)
        .arg(&dest)
        .output().expect("Failed to launch rjrssync");
    assert_eq!(output.status.code(), Some(expected_exit_code));

    // Source should always be unchanged
    let new_src_node = load_filesystem_node_from_disk(&src);
    assert_eq!(src_node, new_src_node);

    if expected_exit_code == 0 {
        if let Some(expected_num_copies) = expected_num_copies {
            let search = format!("Copied {} file(s)", expected_num_copies);
            let actual = String::from_utf8(output.stderr).unwrap(); //TODO: not stdout?
            assert!(actual.contains(&search));
        }

        // Dest should be identical to source
        let new_dest_node = load_filesystem_node_from_disk(&dest);

        assert_eq!(src_node, new_dest_node);
    } else {
        // Dest should be unchanged
        let new_dest_node = load_filesystem_node_from_disk(&dest);

        assert_eq!(dest_node, new_dest_node);
    }
}

/// A simple copying of a few files and a folder.
#[test]
fn test_folder_to_folder() {
    let src_folder = folder! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder! {
            "sc" => file("contents3"),
        }
    };
    run_usage_test(src_folder, empty_folder(), Some(3));
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
    run_usage_test(src_folder, dest_folder, Some(3));
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
    run_usage_test(src_folder, dest_folder, Some(1));
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
    run_usage_test(src_folder, dest_folder, Some(1)); 
}

/// Tries syncing a file to a folder
#[test]
fn test_file_to_folder() {
    run_usage_test_expect_failure(file("contents1"), empty_folder(), 12); 
}

/// Tries syncing a folder to a file
#[test]
fn test_folder_to_file() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_usage_test_expect_failure(src_folder, file("contents2"), 12); 
}

/// Tries syncing a file to a file
#[test]
fn test_file_to_file() {
    run_usage_test_expect_failure(file("contents1"), file("contents2"), 12); 
}

/// Tries syncing a file to a non-existent path
#[test]
fn test_file_to_nothing() {
    run_usage_test_impl(Some(file("contents1")), None, 12, None); 
}

/// Tries syncing a folder to a non-existent path
#[test]
fn test_folder_to_nothing() {
    let src_folder = folder! {
        "file1" => file("contents"),
    };
    run_usage_test_impl(Some(src_folder), None, 12, None); 
}

/// Tries syncing a non-existent path to a file
#[test]
fn test_nothing_to_file() {
    run_usage_test_impl(None, Some(file("contents")), 12, None); 
}

/// Tries syncing a non-existent path to a folder
#[test]
fn test_nothing_to_folder() {
    let dest_folder = folder! {
        "file1" => file("contents"),
    };
    run_usage_test_impl(None, Some(dest_folder), 12, None); 
}

/// Tries syncing a non-existent path to a non-existent path
#[test]
fn test_nothing_to_nothing() {
    run_usage_test_impl(None, None, 12, None); 
}