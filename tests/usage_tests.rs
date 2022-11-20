use std::{path::Path, time::{SystemTime, Duration}};

use tempdir::TempDir;

/// Simple in-memory representation of a tree of files and folders, to use for testing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FilesystemNode {
    Folder {
        name: String,
        children: Vec<FilesystemNode>,
    },
    File {
        name: String,  
        contents: Vec<u8>,     
        modified: SystemTime,
    }
}
impl FilesystemNode {
    fn folder(name: &str, children: &[FilesystemNode]) -> FilesystemNode {
        FilesystemNode::Folder{name: name.to_string(), children: children.into() }
    }
    fn file(name: &str, contents: &str) -> FilesystemNode {
        FilesystemNode::File{name: name.to_string(), contents: contents.as_bytes().to_vec(), modified: SystemTime::now() }       
    }
    fn file_with_modified(name: &str, contents: &str, modified: SystemTime) -> FilesystemNode {
        FilesystemNode::File{name: name.to_string(), contents: contents.as_bytes().to_vec(), modified }       
    }
}

/// Mirrors the given tree of files/folders onto disk in the given folder.
fn save_filesystem_tree_to_disk(tree: &[FilesystemNode], folder: &Path) { 
    for n in tree {
        match n {
            FilesystemNode::File { name, contents, modified } => {
                std::fs::write(folder.join(name), contents).unwrap();
                filetime::set_file_mtime(folder.join(name), filetime::FileTime::from_system_time(*modified)).unwrap();
            },
            FilesystemNode::Folder { name, children } => {
                std::fs::create_dir(folder.join(name)).unwrap();
                save_filesystem_tree_to_disk(children, &folder.join(name));
            }
        }
    }
}

/// Creates an in-memory representation of the tree of files/folders in the given folder.
fn load_filesystem_tree_from_disk(folder: &Path) -> Vec<FilesystemNode> {
    let mut result : Vec<FilesystemNode> = vec![];
    for entry in std::fs::read_dir(folder).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            result.push(FilesystemNode::File {
                name: entry.file_name().to_string_lossy().to_string(),
                contents: std::fs::read(entry.path()).unwrap(),
                modified: entry.metadata().unwrap().modified().unwrap()
            });
        } else if entry.file_type().unwrap().is_dir() {
            result.push(FilesystemNode::Folder {
                name: entry.file_name().to_string_lossy().to_string(),
                children: load_filesystem_tree_from_disk(&folder.join(entry.file_name())),
            });
           
        } else {
            panic!("Unsuppoted file type");
        }
    }
    result
}

/// Checks that running rjrssync with src and dest folders containing the specified files/folders
/// is successful and the dest folder ends up looking identical to the src folder.
/// Optionally also tests that the expected number of files were copied.
fn run_usage_test(src_tree: &[FilesystemNode], dest_tree: &[FilesystemNode], 
    expected_num_copies: Option<u32>) {
    let src_dir = TempDir::new("rjrssync-test").unwrap();
    save_filesystem_tree_to_disk(src_tree, &src_dir.path());

    let dest_dir = TempDir::new("rjrssync-test").unwrap();
    save_filesystem_tree_to_disk(dest_tree, &dest_dir.path());

    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    let output = std::process::Command::new(rjrssync_path)
        .arg(src_dir.path())
        .arg(dest_dir.path())
        .output().expect("rjrssync failed");
    assert!(output.status.success());

    if let Some(expected_num_copies) = expected_num_copies {
        let search = format!("Copied {} file(s)", expected_num_copies);
        let actual = String::from_utf8(output.stderr).unwrap(); //TODO: not stdout?
        assert!(actual.contains(&search));
    }

    let new_dest_tree = load_filesystem_tree_from_disk(&dest_dir.path());

    assert_eq!(src_tree.to_vec(), new_dest_tree);
}

/// A simple copying of a few files and a folder.
#[test]
fn test_simple_sync() {
    let src_tree = &[
        FilesystemNode::file("c1", "contents1"),
        FilesystemNode::file("c2", "contents2"),
        FilesystemNode::folder("c3", &[
            FilesystemNode::file("sc", "contents3"),
        ])
    ];
    run_usage_test(src_tree, &[], Some(3));
}

/// Some files and a folder in the destination need deleting.
#[test]
fn test_remove_dest_stuff() {
    let src_tree = &[
        FilesystemNode::file("c1", "contents1"),
        FilesystemNode::file("c2", "contents2"),
        FilesystemNode::folder("c3", &[
            FilesystemNode::file("sc", "contents3"),
        ])
    ];
    let dest_tree = &[
        FilesystemNode::file("remove me", "contents1"),
        FilesystemNode::file("remove me too", "contents2"),
        FilesystemNode::folder("remove this whole folder", &[
            FilesystemNode::file("sc", "contents3"),
        ])
    ];
    run_usage_test(src_tree, dest_tree, Some(3));
}

/// A file exists but has an old timestamp so needs updating.
#[test]
fn test_update_file() {
    let src_tree = &[
        FilesystemNode::file_with_modified("file", "contents1", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    ];
    let dest_tree = &[
        FilesystemNode::file_with_modified("file", "contents2", SystemTime::UNIX_EPOCH),
    ];
    run_usage_test(src_tree, dest_tree, Some(1));
}

/// Most files have the same timestamp so don't need updating, but one does.
#[test]
fn test_skip_unchanged() {
    let src_tree = &[
        FilesystemNode::file_with_modified("file1", "contentsNEW", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        FilesystemNode::file_with_modified("file2", "contents2", SystemTime::UNIX_EPOCH),
        FilesystemNode::file_with_modified("file3", "contents3", SystemTime::UNIX_EPOCH),
    ];
    let dest_tree = &[
        FilesystemNode::file_with_modified("file1", "contentsOLD", SystemTime::UNIX_EPOCH),
        FilesystemNode::file_with_modified("file2", "contents2", SystemTime::UNIX_EPOCH),
        FilesystemNode::file_with_modified("file3", "contents3", SystemTime::UNIX_EPOCH),
    ];
    // Check that exactly one file was copied (the other two should have been skipped)
    run_usage_test(src_tree, dest_tree, Some(1)); 
}
