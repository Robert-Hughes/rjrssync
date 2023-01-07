use std::{path::{PathBuf}};


use regex::Regex;
use tempdir::TempDir;

use crate::test_utils::{run_process_with_live_output, get_unique_remote_temp_folder, RemotePlatform};
use crate::filesystem_node::*;

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

