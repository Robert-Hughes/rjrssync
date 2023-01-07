use std::{collections::HashMap, time::SystemTime, path::{PathBuf, Path}, process::Command, os::windows::fs::FileTypeExt};

use crate::test_utils::*;

use tempdir::TempDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymlinkKind {
    #[cfg_attr(unix, allow(unused))]
    File, // Windows-only
    #[cfg_attr(unix, allow(unused))]
    Folder, // Windows-only
    #[cfg_attr(windows, allow(unused))]
    Generic, // Unix-only
}

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
    },
    Symlink {
        kind: SymlinkKind,
        target: PathBuf,
    },
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
/// Creates a file symlink, but on Linux where all symlinks are generic, this creates a generic symlink instead.
/// This allows us to write generic test code, but we need to make sure to run the tests on both Linux and Windows.
pub fn symlink_file(target: &str) -> FilesystemNode {
    if cfg!(windows) {
        FilesystemNode::Symlink { kind: SymlinkKind::File, target: PathBuf::from(target) }
    } else {
        FilesystemNode::Symlink { kind: SymlinkKind::Generic, target: PathBuf::from(target) }
    }
}
/// Creates a folder symlink, but on Linux where all symlinks are generic, this creates a generic symlink instead.
/// This allows us to write generic test code, but we need to make sure to run the tests on both Linux and Windows.
pub fn symlink_folder(target: &str) -> FilesystemNode {
    if cfg!(windows) {
        FilesystemNode::Symlink { kind: SymlinkKind::Folder, target: PathBuf::from(target) }
    } else {
        FilesystemNode::Symlink { kind: SymlinkKind::Generic, target: PathBuf::from(target) }
    }
}
/// Creates a generic symlink, which is only supported on Linux. Attempting to write this to the filesystem on
/// Windows will panic.
#[cfg_attr(windows, allow(unused))]
pub fn symlink_generic(target: &str) -> FilesystemNode {
    FilesystemNode::Symlink { kind: SymlinkKind::Generic, target: PathBuf::from(target) }
}

/// Mirrors the given file/folder and its descendants onto disk, at the given path.
pub fn save_filesystem_node_to_disk_local(node: &FilesystemNode, path: &Path) { 
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
                save_filesystem_node_to_disk_local(child, &path.join(child_name));
            }
        }
        FilesystemNode::Symlink { kind, target } => {
            match kind {
                SymlinkKind::File => {
                    #[cfg(windows)]
                    std::os::windows::fs::symlink_file(target, path).expect("Failed to create symlink file");
                    #[cfg(not(windows))]
                    panic!("Not supported on this OS");
                },
                SymlinkKind::Folder => {
                    #[cfg(windows)]
                    std::os::windows::fs::symlink_dir(target, path).expect("Failed to create symlink dir");
                    #[cfg(not(windows))]
                    panic!("Not supported on this OS");        
                }
                SymlinkKind::Generic => {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(target, path).expect("Failed to create unspecified symlink");
                    #[cfg(not(unix))]
                    panic!("Not supported on this OS");        
                },
            }
        }
    }
}

/// Mirrors the given file/folder and its descendants onto disk, at the given path, which includes a remote prefix
/// Save the folder structure locally, tar it up, copy it over and untar it. 
/// We use tar to preserve symlinks (as scp would otherwise follow these and we would lose them).
pub fn save_filesystem_node_to_disk_remote(node: &FilesystemNode, remote_host_and_path: &str) {
    let (remote_host, remote_path) = remote_host_and_path.split_once(':').expect("Missing colon");
    let (remote_parent_folder, node_name) = remote_path.rsplit_once(|d| d == '/' || d == '\\').expect("Missing slash");

    let local_temp_folder = TempDir::new("rjrssync-test-remote-staging").unwrap();
    let local_temp_folder = local_temp_folder.path();
  
    // Create local
    let local_node_path = local_temp_folder.join(node_name);
    save_filesystem_node_to_disk_local(node, &local_node_path);

    // Pack into tar
    let tar_file_local = local_temp_folder.join("stuff.tar");
    // Important to use --format=posix so that modified timestamps are preserved at higher precision (the default is just 1 second)
    assert_process_with_live_output(Command::new("tar").arg("--format=posix") 
        .arg("-cf").arg(&tar_file_local).arg("-C").arg(local_temp_folder).arg(node_name));

    // Copy tar to remote
    let tar_file_remote = String::from(remote_path) + ".tar";
    assert_process_with_live_output(Command::new("scp").arg(&tar_file_local).arg(format!("{}:{}", remote_host, tar_file_remote)));

    // Check that the destination doesn't already exist (otherwise will cause problems as the 
    // new stuff will be merged with the existing stuff)
    let r = run_process_with_live_output(Command::new("ssh").arg(remote_host).arg(format!("stat {remote_path} || dir {remote_path}")));
    assert!(!r.exit_status.success());

    // Extract on remote
    assert_process_with_live_output(Command::new("ssh").arg(remote_host)
        .arg(format!("tar -xf {tar_file_remote} -C {remote_parent_folder}")));
}

/// Creates an in-memory representation of the file/folder and its descendents at the given path.
/// Returns None if the path doesn't point to anything.
pub fn load_filesystem_node_from_disk_local(path: &Path) -> Option<FilesystemNode> {
    // Note using symlink_metadata, so that we see the metadata for a symlink,
    // not the thing that it points to.
    let metadata = match std::fs::symlink_metadata(path) {
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
            load_filesystem_node_from_disk_local(&path.join(entry.file_name())).unwrap());
        }        
        Some(FilesystemNode::Folder { children })
    } else if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path).expect("Unable to read symlink target");
        // On Windows, symlinks are either file-symlinks or dir-symlinks
        #[cfg(windows)]
        let kind = if metadata.file_type().is_symlink_file() {
            SymlinkKind::File
        } else if metadata.file_type().is_symlink_dir() {
            SymlinkKind::Folder
        } else {
            panic!("Unknown symlink type type")
        };
        #[cfg(not(windows))]
        let kind = SymlinkKind::Generic;

        Some(FilesystemNode::Symlink { kind, target })
    } else {
        panic!("Unknown file type");
    }
}

/// Creates an in-memory representation of the file/folder and its descendents at the given path, which includes a remote prefix
/// Returns None if the path doesn't point to anything.
/// Tar up the folder structure remotely, copy it locally and read it
/// We use tar to preserve symlinks (as scp would otherwise follow these and we would lose them).
pub fn load_filesystem_node_from_disk_remote(remote_host_and_path: &str) -> Option<FilesystemNode> {
    let (remote_host, remote_path) = remote_host_and_path.split_once(':').expect("Missing colon");
    let (remote_parent_folder, node_name) = remote_path.rsplit_once(|d| d == '/' || d == '\\').expect("Missing slash");
    
    let local_temp_folder = TempDir::new("rjrssync-test-remote-staging").unwrap();
    let local_temp_folder = local_temp_folder.path();

    // Pack into tar
    let tar_file_remote = String::from(remote_path) + ".tar";
    let r = run_process_with_live_output(Command::new("ssh").arg(remote_host)
        // Important to use --format=posix so that modified timestamps are preserved at higher precision (the default is just 1 second)
        .arg(format!("tar --format=posix -cf {tar_file_remote} -C {remote_parent_folder} {node_name}")));
    if r.stderr.contains("No such file or directory") {
        return None;
    } else {
        assert!(r.exit_status.success());
    }

    // Copy tar from remote
    let tar_file_local = local_temp_folder.join("stuff.tar");
    assert_process_with_live_output(Command::new("scp").arg(format!("{}:{}", remote_host, tar_file_remote)).arg(&tar_file_local));
    
    // Extract it
    assert_process_with_live_output(Command::new("tar").arg("-xf").arg(tar_file_local)
        .arg("-C").arg(&local_temp_folder));

    // Load into memory
    let local_node_path = local_temp_folder.join(node_name);
    load_filesystem_node_from_disk_local(&local_node_path)
}