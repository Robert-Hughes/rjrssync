use std::{path::Path, time::{SystemTime, Duration}, collections::HashMap};

use map_macro::map;
use tempdir::TempDir;

/// Simple in-memory representation of a tree of files and folders, to use for testing.
/// Note that this representation is consistent with the approach described in the README.
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

fn folder(children: HashMap<&str, FilesystemNode>) -> FilesystemNode {
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
fn save_filesystem_tree_to_disk(node: &FilesystemNode, path: &Path) { 
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
                save_filesystem_tree_to_disk(child, &path.join(child_name));
            }
        }
    }
}

/// Creates an in-memory representation of the file/folder and its descendents at the given path.
fn load_filesystem_tree_from_disk(path: &Path) -> FilesystemNode {
    let metadata = std::fs::metadata(path).unwrap();

    if metadata.file_type().is_file() {
        FilesystemNode::File {
            contents: std::fs::read(path).unwrap(),
            modified: metadata.modified().unwrap()
        }
    } else if metadata.file_type().is_dir() {
        let mut children = HashMap::<String, FilesystemNode>::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            children.insert(entry.file_name().to_str().unwrap().to_string(), load_filesystem_tree_from_disk(&path.join(entry.file_name())));
        }        
        FilesystemNode::Folder { children }
    } else {
        panic!("Unsuppoted file type");
    }
}

/// Checks that running rjrssync with src and dest arguments pointing to the specified files/folders
/// is successful and that the dest ends up looking identical to the src (including children).
/// Optionally also tests that the expected number of files were copied.
fn run_usage_test(src_tree: FilesystemNode, dest_tree: FilesystemNode, expected_num_copies: Option<u32>) {
    // Create temporary folders to store the src and dest nodes
    let src_holder = TempDir::new("rjrssync-test").unwrap();
    let src = src_holder.path().join("src");
    save_filesystem_tree_to_disk(&src_tree, &src);

    let dest_holder = TempDir::new("rjrssync-test").unwrap();
    let dest = dest_holder.path().join("dest");
    save_filesystem_tree_to_disk(&dest_tree, &dest);

    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    let output = std::process::Command::new(rjrssync_path)
        .arg(&src)
        .arg(&dest)
        .output().expect("rjrssync failed");
    assert!(output.status.success());

    if let Some(expected_num_copies) = expected_num_copies {
        let search = format!("Copied {} file(s)", expected_num_copies);
        let actual = String::from_utf8(output.stderr).unwrap(); //TODO: not stdout?
        assert!(actual.contains(&search));
    }

    let new_dest_tree = load_filesystem_tree_from_disk(&dest);

    assert_eq!(src_tree, new_dest_tree);
}

/// A simple copying of a few files and a folder.
#[test]
fn test_simple_sync() {
    let src_tree = folder(map! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder(map! {
            "sc" => file("contents3"),
        })
    });
    run_usage_test(src_tree, empty_folder(), Some(3));
}

/// Some files and a folder in the destination need deleting.
#[test]
fn test_remove_dest_stuff() {
    let src_tree = folder(map! {
        "c1" => file("contents1"),
        "c2" => file("contents2"),
        "c3" => folder(map! {
            "sc" => file("contents3"),
        })
    });
    let dest_tree = folder(map! {
        "remove me" => file("contents1"),
        "remove me too" => file("contents2"),
        "remove this whole folder" => folder(map! {
            "sc" => file("contents3"),
        })
    });
    run_usage_test(src_tree, dest_tree, Some(3));
}

/// A file exists but has an old timestamp so needs updating.
#[test]
fn test_update_file() {
    let src_tree = folder(map! {
        "file" => file_with_modified("contents1", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    });
    let dest_tree = folder(map! {
        "file" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    });
    run_usage_test(src_tree, dest_tree, Some(1));
}

/// Most files have the same timestamp so don't need updating, but one does.
#[test]
fn test_skip_unchanged() {
    let src_tree = folder(map! {
        "file1" => file_with_modified("contentsNEW", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "file2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "file3" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
    });
    let dest_tree = folder(map! {
        "file1" => file_with_modified("contentsOLD", SystemTime::UNIX_EPOCH),
        "file2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "file3" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
    });
    // Check that exactly one file was copied (the other two should have been skipped)
    run_usage_test(src_tree, dest_tree, Some(1)); 
}
