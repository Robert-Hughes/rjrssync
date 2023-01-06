use std::{path::{Path, PathBuf}, time::{SystemTime}, collections::HashMap, process::Command};
#[cfg(windows)]
use std::os::windows::fs::FileTypeExt;

use regex::Regex;
use tempdir::TempDir;

use crate::test_utils::{run_process_with_live_output, get_unique_remote_temp_folder, RemotePlatform};
use crate::test_utils::assert_process_with_live_output;

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
fn save_filesystem_node_to_disk_local(node: &FilesystemNode, path: &Path) { 
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
fn save_filesystem_node_to_disk_remote(node: &FilesystemNode, remote_host_and_path: &str) {
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
fn load_filesystem_node_from_disk_local(path: &Path) -> Option<FilesystemNode> {
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
fn load_filesystem_node_from_disk_remote(remote_host_and_path: &str) -> Option<FilesystemNode> {
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

/// Describes a test configuration in a generic way that hopefully covers various success and failure cases.
/// This is quite verbose to use directly, so some helper functions are available that fill this in for common
/// test cases.
/// For example, this can be used to check that a sync completes successfully with a message stating
/// that some files were copied, and check that the files were in fact copied onto the filesystem.
/// All the paths provided here will have special values substituted:
///     * $TEMP => a (local) empty temporary folder created for placing test files in
///     * $REMOTE_WINDOWS_TEMP => an empy temporary folder created on a remote windows platform for placing test files in
///     * $REMOTE_LINUX_TEMP => an empty temporary folder created on a remote linux platform for placing test files in
#[derive(Default)]
pub struct TestDesc<'a> {
    /// The given FilesystemNodes are saved to the given paths before running rjrssync
    /// (e.g. to set up src and dest).
    pub setup_filesystem_nodes: Vec<(&'a str, &'a FilesystemNode)>,
    /// Arguments provided to rjrssync, most likely the source and dest paths.
    /// (probably the same as paths in setup_filesystem_nodes, but may have different trailing slash for example).
    pub args: Vec<String>,
    /// List of responses to prompts that rjrssync asks (e.g. whether to overwrite files)
    pub prompt_responses: Vec<String>,
    /// The expected exit code of rjrssync (e.g. 0 for success).
    pub expected_exit_code: i32,
    /// Messages that are expected to be present in rjrssync's stdout/stderr, 
    /// along with the expected number of occurences (use zero to indicate that a message should _not_ appear).
    pub expected_output_messages: Vec<(usize, Regex)>,
    /// The filesystem at the given paths are expected to be as described (including None, for non-existent)
    pub expected_filesystem_nodes: Vec<(&'a str, Option<&'a FilesystemNode>)>
}

/// Checks that running rjrssync with the setup described by the TestDesc behaves as described by the TestDesc.
/// See TestDesc for more details.
pub fn run(desc: TestDesc) {
    // Create a temporary folder to store test files/folders,
    let temp_folder = TempDir::new("rjrssync-test").unwrap();
    let mut temp_folder = temp_folder.path().to_path_buf();
    if let Ok(o) = std::env::var("RJRSSYNC_TEST_TEMP_OVERRIDE") {
        // For keeping test data around afterwards
        std::fs::create_dir_all(&o).expect("Failed to create override dir");
        temp_folder = PathBuf::from(o); 
    }

    // Lazy-initialize as we might not need these, and we want to be able to work when remote platforms
    // aren't available
    let mut remote_windows_temp_path = None;
    let mut remote_linux_temp_path = None;

    // All paths provided in TestDesc have $TEMP (and remote windows/linux temps) replaced with the temporary folder.
    let mut substitute_vars = |p: &str| {
        let mut p = p.replace("$TEMP", &temp_folder.to_string_lossy());
        // Lazily evaluate the remote windows/linux variables, so that tests which do not use them do not 
        // need to have remote platforms available (e.g. on GitHub Actions).
        // Use a new remote temporary folder for each test (rather than re-using the root one)
        if p.contains("$REMOTE_WINDOWS_TEMP") {
            let platform = RemotePlatform::get_windows();
            if remote_windows_temp_path.is_none() {
                remote_windows_temp_path = Some(get_unique_remote_temp_folder(platform));
            }
            p = p.replace("$REMOTE_WINDOWS_TEMP", &format!("{}:{}", platform.user_and_host, remote_windows_temp_path.as_ref().unwrap()));
        }
        if p.contains("$REMOTE_LINUX_TEMP") {
            let platform = RemotePlatform::get_linux();
            if remote_linux_temp_path.is_none() {
                remote_linux_temp_path = Some(get_unique_remote_temp_folder(platform));
            }
            p = p.replace("$REMOTE_LINUX_TEMP", &format!("{}:{}", platform.user_and_host, remote_linux_temp_path.as_ref().unwrap()));
        }
        p
    };

    // Setup initial filesystem
    for (p, n) in desc.setup_filesystem_nodes {
        let p = substitute_vars(&p);
        if matches!(p.find(':'), Some(p) if p > 1) { // Note the colon must be after position 2, to avoid treating C:\blah as remote
            save_filesystem_node_to_disk_remote(&n, &p);
        } else {
            save_filesystem_node_to_disk_local(&n, &PathBuf::from(p));
        }
    }

    // Run rjrssync with the specified paths
    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    // Run with live output so that we can see the progress of slow tests as they happen, rather than waiting 
    // until the end.
    let output = run_process_with_live_output(
        std::process::Command::new(rjrssync_path)
        .current_dir(&temp_folder) // So that any relative paths are inside the test folder
        .env("RJRSSYNC_TEST_PROMPT_RESPONSE", desc.prompt_responses.join(","))
        .args(desc.args.iter().map(|a| substitute_vars(a))));

    // Check exit code
    assert_eq!(output.exit_status.code(), Some(desc.expected_exit_code));

    // Check for expected output messages
    let actual_output = output.stderr + &output.stdout;
    for (n, r) in desc.expected_output_messages {
        println!("Checking for match(es) against '{}'", r);
        let actual_matches = r.find_iter(&actual_output).count();
        assert_eq!(actual_matches, n);
    }

    // Check the filesystem is as expected afterwards
    for (p, n) in desc.expected_filesystem_nodes {
        let p = substitute_vars(&p);
        let actual_node = if matches!(p.find(':'), Some(p) if p > 1) {  // Note the colon must be after position 2, to avoid treating C:\blah as remote
            load_filesystem_node_from_disk_remote(&p)
        } else {
            load_filesystem_node_from_disk_local(&PathBuf::from(&p))
        };
        
        println!("Checking filesystem contents at '{}'", p);
        assert_eq!(actual_node.as_ref(), n);    
    }
}

#[derive(Default)]
pub struct NumActions {
    pub copied_files: u32,
    pub created_folders: u32,
    pub copied_symlinks: u32,

    pub deleted_files: u32,
    pub deleted_folders: u32,
    pub deleted_symlinks: u32,
}

pub fn copied_files(x: u32) -> NumActions {
    NumActions { copied_files: x, ..Default::default() }
}
#[allow(unused)]
pub fn copied_symlinks(x: u32) -> NumActions {
    NumActions { copied_symlinks: x, ..Default::default() }
}
pub fn copied_files_and_folders(files: u32, folders: u32) -> NumActions {
    NumActions { copied_files: files, created_folders: folders, ..Default::default() }
}
pub fn copied_files_and_symlinks(files: u32, symlinks: u32) -> NumActions {
    NumActions { copied_files: files, copied_symlinks: symlinks, ..Default::default() }
}
pub fn copied_files_folders_and_symlinks(files: u32, folders: u32, symlinks: u32) -> NumActions {
    NumActions { copied_files: files, created_folders: folders, copied_symlinks: symlinks, ..Default::default() }
}
impl From<NumActions> for Vec<(usize, Regex)> {
    fn from(a: NumActions) -> Vec<(usize, Regex)> {
        a.get_expected_output_messages()
    }
}

impl NumActions {
    pub fn get_expected_output_messages(&self) -> Vec<(usize, Regex)> {
        let mut result = vec![];
        if self.copied_files + self.created_folders + self.copied_symlinks > 0 {
            result.push((1, Regex::new(&regex::escape(&format!("Copied {} file(s)", self.copied_files))).unwrap()));
            result.push((1, Regex::new(&regex::escape(&format!("created {} folder(s)", self.created_folders))).unwrap()));
            result.push((1, Regex::new(&regex::escape(&format!("copied {} symlink(s)", self.copied_symlinks))).unwrap()));
        } else {
            result.push((0, Regex::new("Copied|copied|created").unwrap()));
        }
        if self.deleted_files + self.deleted_folders + self.deleted_symlinks > 0 {
            result.push((1, Regex::new(&regex::escape(&format!("Deleted {} file(s), {} folder(s) and {} symlink(s)",
                self.deleted_files,
                self.deleted_folders,
                self.deleted_symlinks))).unwrap()));
        } else {
            result.push((0, Regex::new("Deleted|deleted").unwrap()));            
        }
        if self.copied_files + self.created_folders + self.copied_symlinks +
           self.deleted_files + self.deleted_folders + self.deleted_symlinks == 0 {
            result.push((1, Regex::new(&regex::escape("Nothing to do")).unwrap()));
        }
        result
    }
}

/// Runs a test that syncs the given src FilesystemNode (e.g. file or folder) to the given dest 
/// FilesystemNode, and checks that the sync is successful, and the destination is updated to be equal
/// to the source.
pub fn run_expect_success(src_node: &FilesystemNode, dest_node: &FilesystemNode, expected_actions: NumActions) {
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", src_node),
            ("$TEMP/dest", dest_node),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            // Some tests require deleting the dest root, which we allow here. The default is to prompt
            // the user, which is covered by other tests (dest_root_needs_deleting_tests.rs)
            String::from("--dest-root-needs-deleting"), 
            String::from("delete"),
        ],
        expected_exit_code: 0,
        expected_output_messages: expected_actions.into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(src_node)), // Source should always be unchanged
            ("$TEMP/dest", Some(src_node)), // Dest should be identical to source
        ],
        ..Default::default()
    });
}

