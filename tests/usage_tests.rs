use std::path::Path;

use tempdir::TempDir;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FilesystemNode {
    Folder {
        name: String,
        children: Vec<FilesystemNode>,
    },
    File {
        name: String,  
        contents: Vec<u8>,     
    }
}
impl FilesystemNode {
    fn folder(name: &str, children: &[FilesystemNode]) -> FilesystemNode {
        FilesystemNode::Folder{name: name.to_string(), children: children.into() }
    }
    fn file(name: &str, contents: &str) -> FilesystemNode {
        FilesystemNode::File{name: name.to_string(), contents: contents.as_bytes().to_vec() }       
    }
}

fn save_filesystem_tree_to_disk(tree: &[FilesystemNode], folder: &Path) { 
    for n in tree {
        match n {
            FilesystemNode::File { name, contents } => {
                std::fs::write(folder.join(name), contents).unwrap();
            },
            FilesystemNode::Folder { name, children } => {
                std::fs::create_dir(folder.join(name)).unwrap();
                save_filesystem_tree_to_disk(children, &folder.join(name));
            }
        }
    }
}

fn load_filesystem_tree_from_disk(folder: &Path) -> Vec<FilesystemNode> {
    let mut result : Vec<FilesystemNode> = vec![];
    for entry in std::fs::read_dir(folder).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            result.push(FilesystemNode::File {
                name: entry.file_name().to_string_lossy().to_string(),
                contents: std::fs::read(entry.path()).unwrap(),
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

#[test]
fn test_simple_sync() {
    let src_tree = &[
        FilesystemNode::file("c1", "contents1"),
        FilesystemNode::file("c2", "contents2"),
        FilesystemNode::folder("c3", &[
            FilesystemNode::file("sc", "contents3"),
        ])
    ];

    let src_dir = TempDir::new("rjrssync-test").unwrap();
    save_filesystem_tree_to_disk(src_tree, &src_dir.path());

    let dest_dir = TempDir::new("rjrssync-test").unwrap();

    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    std::process::Command::new(rjrssync_path)
        .arg(src_dir.path())
        .arg(dest_dir.path())
        .status().expect("rjrssync failed");

    let dest_tree = load_filesystem_tree_from_disk(&dest_dir.path());

    assert_eq!(src_tree.to_vec(), dest_tree);
}