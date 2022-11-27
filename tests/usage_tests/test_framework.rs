use std::{path::{Path, PathBuf}, time::{SystemTime}, collections::HashMap};

use tempdir::TempDir;

/// Simple in-memory representation of a file or folder (including any children), to use for testing.
/// Note that this representation is consistent with the approach described in the README,
/// and so doesn't consider the name of the node to be part of the node itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemNode {
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
#[macro_export]
macro_rules! folder {
    ($($tts:tt)*) => {
        folder(map! { $($tts)* })
    }
}

pub fn folder(children: HashMap<&str, FilesystemNode>) -> FilesystemNode {
    // Convert to a map with owned Strings (rather than &str). We take &strs in the param
    // to make the test code simpler.
    let children : HashMap<String, FilesystemNode> = children.into_iter().map(|(n, c)| (n.to_string(), c)).collect();
    FilesystemNode::Folder{ children }
}
pub fn empty_folder() -> FilesystemNode {
    FilesystemNode::Folder{ children: HashMap::new() }
}
pub fn file(contents: &str) -> FilesystemNode {
    FilesystemNode::File{ contents: contents.as_bytes().to_vec(), modified: SystemTime::now() }       
}
pub fn file_with_modified(contents: &str, modified: SystemTime) -> FilesystemNode {
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
pub struct TestDesc<'a> {
    /// The given FilesystemNodes are saved to the given paths before running rjrssync 
    /// (e.g. to set up src and dest).
    pub setup_filesystem_nodes: Vec<(&'a str, &'a FilesystemNode)>,
    /// The value provided to rjrssync as its source path.
    /// (probably the same as a path in setup_filesystem_nodes, but may have different trailing slash for example).
    pub src: &'a str,
    /// The value provided to rjrssync as its dest path.
    /// (probably the same as a path in setup_filesystem_nodes, but may have different trailing slash for example).
    pub dest: &'a str, 
    /// The expected exit code of rjrssync (e.g. 0 for success).
    pub expected_exit_code: i32,
    /// Messages that are expected to be present in rjrssync's stdout/stderr
    pub expected_output_messages: Vec<String>, 
    /// The filesystem at the given paths are expected to be as described (including None, for non-existent)
    pub expected_filesystem_nodes: Vec<(&'a str, Option<&'a FilesystemNode>)>
}

/// Checks that running rjrssync with the setup described by the TestDesc behaves as described by the TestDesc.
/// See TestDesc for more details.
pub fn run(desc: TestDesc) {
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
pub fn run_expect_success(src_node: &FilesystemNode, dest_node: &FilesystemNode, expected_num_copies: u32) {
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

