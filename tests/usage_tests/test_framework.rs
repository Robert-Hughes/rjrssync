use std::{path::{Path, PathBuf}, time::{SystemTime}, collections::HashMap, process::Stdio, sync::mpsc::{Sender, Receiver, self, SendError}, thread, fmt::{Display, self}, io::{BufReader, BufRead}};
#[cfg(windows)]
use std::os::windows::fs::FileTypeExt;

use regex::Regex;
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
pub fn symlink_unspecified(target: &str) -> FilesystemNode {
    FilesystemNode::Symlink { kind: SymlinkKind::Generic, target: PathBuf::from(target) }
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

/// Creates an in-memory representation of the file/folder and its descendents at the given path.
/// Returns None if the path doesn't point to anything.
fn load_filesystem_node_from_disk(path: &Path) -> Option<FilesystemNode> {
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
                load_filesystem_node_from_disk(&path.join(entry.file_name())).unwrap());
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

struct ProcessOutput {
    exit_status: std::process::ExitStatus,
    stdout: String,
    #[allow(unused)]
    stderr: String,
}

/// Runs a child processes and waits for it to exit. The stdout and stderr of the child process
/// are captured and forwarded to our own.
/// This is mostly a copy-paste of the same function from boss_launch.rs, but we don't have a good way to share the code
/// and this version is slightly different, more suitable for tests (e.g. simpler error checking).
fn run_process_with_live_output(c: &mut std::process::Command) -> ProcessOutput {
    println!("Running {:?} {:?}...", c.get_program(), c.get_args());

    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to launch child process");

    // unwrap is fine here, as the streams should always be available as we piped them all
    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    // Spawn a background thread for each stdout and stderr, to process messages we get from the child
    // and forward them to the main thread. This is easier than some kind of async IO stuff.
    let (sender1, receiver): (
        Sender<OutputReaderThreadMsg2>,
        Receiver<OutputReaderThreadMsg2>,
    ) = mpsc::channel();
    let sender2 = sender1.clone();
    let thread_builder = thread::Builder::new().name("child_stdout_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main2(child_stdout, OutputReaderStreamType::Stdout, sender1)
    }).unwrap();
    let thread_builder = thread::Builder::new().name("child_stderr_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main2(child_stderr, OutputReaderStreamType::Stderr, sender2)
    }).unwrap();

    let mut captured_stdout = String::new();
    let mut captured_stderr = String::new();
    loop {
        match receiver.recv() {
            Ok(OutputReaderThreadMsg2::Line(stream_type, l)) => {
                // Print output for test debugging. Note that we need to use println, not write directly to stdout, so that
                // cargo's testing framework captures the output correctly.
                match stream_type {
                    OutputReaderStreamType::Stdout => {
                        println!("{}", l);
                        captured_stdout += &(l + "\n");
                    }
                    OutputReaderStreamType::Stderr => {
                        eprintln!("{}", l);
                        captured_stderr += &(l + "\n");
                    }
                }
            }
            Ok(OutputReaderThreadMsg2::Error(stream_type, e)) => {
                panic!("Error reading from {}: {}", stream_type, e);
            }
            Ok(OutputReaderThreadMsg2::StreamClosed(stream_type)) => {
                println!("Child process {} closed", stream_type);
            }
            Err(_) => {
                // Both senders have been dropped, i.e. both background threads exited
                println!("Both reader threads done, child process must have exited. Waiting for process.");
                // Wait for the process to exit, to get the exit code
                let result = match child.wait() {
                    Ok(r) => r,
                    Err(e) => panic!("Error waiting for child process: {}", e),
                };
                return ProcessOutput { exit_status: result, stdout: captured_stdout, stderr: captured_stderr };
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum OutputReaderStreamType {
    Stdout,
    Stderr,
}
impl Display for OutputReaderStreamType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OutputReaderStreamType::Stdout => write!(f, "stdout"),
            OutputReaderStreamType::Stderr => write!(f, "stderr"),
        }
    }
}

// Sent from the threads reading stdout and stderr of a child process back to the main thread.
enum OutputReaderThreadMsg2 {
    Line(OutputReaderStreamType, String),
    Error(OutputReaderStreamType, std::io::Error),
    StreamClosed(OutputReaderStreamType),
}

fn output_reader_thread_main2<S>(
    stream: S,
    stream_type: OutputReaderStreamType,
    sender: Sender<OutputReaderThreadMsg2>,
) -> Result<(), SendError<OutputReaderThreadMsg2>>
where S : std::io::Read {
    let mut reader = BufReader::new(stream);
    loop {
        let mut l: String = "".to_string();
        // Note we ignore errors on the sender here, as the other end should never have been dropped while it still cares
        // about our messages, but may have dropped if they abandon the child process, letting it finish itself.
        match reader.read_line(&mut l) {
            Err(e) => {
                sender.send(OutputReaderThreadMsg2::Error(stream_type, e))?;
                return Ok(());
            }
            Ok(0) => {
                // end of stream
                sender.send(OutputReaderThreadMsg2::StreamClosed(stream_type))?;
                return Ok(());
            }
            Ok(_) => {
                l.pop(); // Remove the trailing newline
                // A line of other content, for example a prompt or error from ssh itself
                sender.send(OutputReaderThreadMsg2::Line(stream_type, l))?;
            }
        }
    }
}

/// Describes a test configuration in a generic way that hopefully covers various success and failure cases.
/// This is quite verbose to use directly, so some helper functions are available that fill this in for common
/// test cases.
/// For example, this can be used to check that a sync completes successfully with a message stating
/// that some files were copied, and check that the files were in fact copied onto the filesystem.
/// All the paths provided here will have the special value $TEMP substituted for the temporary folder
/// created for placing test files in.
#[derive(Default)]
pub struct TestDesc<'a> {
    /// The given FilesystemNodes are saved to the given paths before running rjrssync
    /// (e.g. to set up src and dest).
    pub setup_filesystem_nodes: Vec<(&'a str, &'a FilesystemNode)>,
    /// Arguments provided to rjrssync, most likely the source and dest paths.
    /// (probably the same as paths in setup_filesystem_nodes, but may have different trailing slash for example).
    pub args: Vec<String>,
    /// The expected exit code of rjrssync (e.g. 0 for success).
    pub expected_exit_code: i32,
    /// Messages that are expected to be present in rjrssync's stdout/stderr
    pub expected_output_messages: Vec<Regex>,
    /// Messages that are expected to _not_ be present in rjrssync's stdout/stderr
    pub unexpected_output_messages: Vec<Regex>,
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

    // All paths provided in TestDesc have $TEMP replaced with the temporary folder.
    let substitute_temp = |p: &str| PathBuf::from(p.replace("$TEMP", &temp_folder.to_string_lossy()));

    // Setup initial filesystem
    for (p, n) in desc.setup_filesystem_nodes {
        save_filesystem_node_to_disk(&n, &substitute_temp(&p));
    }

    // Run rjrssync with the specified paths
    let rjrssync_path = env!("CARGO_BIN_EXE_rjrssync");
    // Run with live output so that we can see the progress of slow tests as they happen, rather than waiting 
    // until the end.
    let output = run_process_with_live_output(
        std::process::Command::new(rjrssync_path)
        .current_dir(&temp_folder) // So that any relative paths are inside the test folder
        .args(desc.args.iter().map(|a| substitute_temp(a))));

    // Check exit code
    assert_eq!(output.exit_status.code(), Some(desc.expected_exit_code));

    // Check for expected output messages
    let actual_output = output.stderr + &output.stdout;
    for m in desc.expected_output_messages {
        println!("Checking for match against '{}'", m);
        assert!(m.is_match(&actual_output));
    }

    // Check for unexpected output messages
    for m in desc.unexpected_output_messages {
        println!("Checking for NO match against '{}'", m);
        assert!(!m.is_match(&actual_output));
    }

    // Check the filesystem is as expected afterwards
    for (p, n) in desc.expected_filesystem_nodes {
        let actual_node = load_filesystem_node_from_disk(&substitute_temp(&p));
        println!("Checking filesystem contents at '{}'", p);
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
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string()
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape(&format!("Copied {} file(s)", expected_num_copies))).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(src_node)), // Source should always be unchanged
            ("$TEMP/dest", Some(src_node)), // Dest should be identical to source
        ],
        ..Default::default()
    });
}

