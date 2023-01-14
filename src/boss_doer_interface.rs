use const_format::concatcp;
use indicatif::HumanBytes;
use regex::{RegexSet};
use serde::{Deserialize, Serialize, Serializer, Deserializer, de::Error};
use std::{
    fmt::{self},
    time::{SystemTime}
};

use crate::encrypted_comms;
use crate::profiling::ProcessProfilingData;
use crate::root_relative_path::RootRelativePath;

// We include the profiling config in the version number, as profiling and non-profiling builds are not compatible
// (because a non-profiling doer won't record any events).
pub const VERSION: &str = concatcp!("126", if cfg!(feature="profiling") { "+profiling"} else { "" });

// Message printed by a doer copy of the program to indicate that it has loaded and is ready
// to receive data over its stdin. Once the boss receives this, it knows that ssh has connected
// correctly etc. It also identifies its version, so the boss side can decide
// if it can continue to communicate or needs to copy over an updated copy of the doer program.
// Note that this format needs to always be backwards-compatible, so is very basic.
pub const HANDSHAKE_STARTED_MSG: &str = "rjrssync doer v"; // Version number will be appended

// Message sent by the doer back to the boss to indicate that it has received the secret key and
// is listening on a network port for a connection.
pub const HANDSHAKE_COMPLETED_MSG: &str = "Waiting for incoming network connection on port "; // Port number will be appended.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Filters {
    /// Use a RegexSet rather than separate Regex objects for better performance.
    /// Serialize the regexes as strings - even though they will need compiling on the 
    /// doer side as well (as part of deserialization), we compile them on the boss side to report earlier errors to 
    /// user (as part of input validation, rather than waiting until the sync starts),
    /// and so that we don't need to validate them again on the doer side,
    /// and won't report the same error twice (from both doers).
    #[serde(serialize_with = "serialize_regex_set_as_strings", deserialize_with="deserialize_regex_set_from_strings")] 
    pub regex_set: RegexSet,
    /// For each regex in the RegexSet above, is it an include filter or an exclude filter.
    pub kinds: Vec<FilterKind>, 
}

/// Serializes a RegexSet by serializing the patterns (strings) that it was originally created from.
/// This won't preserve any non-default creation options!
fn serialize_regex_set_as_strings<S: Serializer>(r: &RegexSet, s: S) -> Result<S::Ok, S::Error> {
    r.patterns().serialize(s)
}

/// Deserializes a RegexSet by deserializing the patterns (strings) that it was originally created from
/// and then re-compiling the RegexSet.
/// This won't preserve any non-default creation options!
fn deserialize_regex_set_from_strings<'de, D: Deserializer<'de>>(d: D) -> Result<RegexSet, D::Error> {
    let patterns = <Vec<String>>::deserialize(d)?;
    RegexSet::new(patterns).map_err(|e| D::Error::custom(e))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum FilterKind {
    Include,
    Exclude,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressMarker {
    /// How much work (in arbitrary units) has been completed.
    pub completed_work: u64,
    /// Whereabouts are we in more descriptive terms.
    pub phase: ProgressPhase,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProgressPhase {
    Deleting {
        /// The number of entries already deleted.
        num_entries_deleted: u32,
        /// The ID of the (dest) entry that is being deleted next, so we can show the filename.
        current_entry_id: Option<u32>,
    },
    Copying {
        /// The number of entries already copied.
        num_entries_copied: u32,
        /// Especially useful for when large files are being copied, this indicates how many total bytes have been copied,
        /// which can increase even though num_entries_copied remains the same.
        num_bytes_copied: u64,
        /// The ID of the (src) entry that is being copied next, so we can show the filename.
        current_entry_id: Option<u32>,
    },
    Done
}

/// Commands are sent from the boss to the doer, to request something to be done.
#[derive(Serialize, Deserialize)]
pub enum Command {
    // Checks the root file/folder and send back information about it,
    // as the boss may need to do something before we send it all the rest of the entries
    SetRoot {
        root: String, // Note this doesn't use a RootRelativePath as it isn't relative to the root - it _is_ the root!
    },
    GetEntries {
        filters: Filters,
    },
    CreateRootAncestors,
    GetFileContent {
        path: RootRelativePath,
    },
    CreateOrUpdateFile {
        path: RootRelativePath,
        #[serde(with = "serde_bytes")] // Make serde fast
        data: Vec<u8>,
        // Note that SystemTime is safe to serialize across platforms, because Serde serializes this 
        // as the elapsed time since UNIX_EPOCH, so it is platform-independent.        
        set_modified_time: Option<SystemTime>,
        /// If set, there is more data for this same file being sent in a following Command.
        /// This is used to split up large files so that we don't send them all in one huge message.
        /// See GetFileContent for more details.
        more_to_follow: bool,
    },
    CreateSymlink {
        path: RootRelativePath,
        kind: SymlinkKind,
        target: SymlinkTarget,
    },
    CreateFolder {
        path: RootRelativePath,
    },
    DeleteFile {
        path: RootRelativePath,
    },
    DeleteFolder {
        path: RootRelativePath,
    },
    DeleteSymlink {
        path: RootRelativePath,
        kind: SymlinkKind,
    },

    ProfilingTimeSync,

    /// Used to mark a position in the sequence of commands, which the doer will echo back 
    /// so that the boss knows when the doer has reached this point. 
    /// The boss can't update the progress bar as soon as it sends a command, as the doer hasn't actually done 
    /// anything yet, so instead it inserts a marker and when the doer echoes this marker back 
    /// (meaning it got this far), the boss updates the progress bar.
    Marker(ProgressMarker),

    Shutdown,
}
impl encrypted_comms::IsFinalMessage for Command {
    fn is_final_message(&self) -> bool {
        match self {
            Self::Shutdown => true,
            _ => false
        }
    }
}
// The default Debug implementation prints all the file data, which is way too much, so we have to override this :(
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note that rust-analyzer can auto-generate the complete version of this for us (delete the function, then Ctrl+Space),
        // then we can make the tweaks that we need.
        match self {
            Self::SetRoot { root } => f.debug_struct("SetRoot").field("root", root).finish(),
            Self::GetEntries { filters } => f.debug_struct("GetEntries").field("filters", filters).finish(),
            Self::CreateRootAncestors => write!(f, "CreateRootAncestors"),
            Self::GetFileContent { path } => f.debug_struct("GetFileContent").field("path", path).finish(),
            Self::CreateOrUpdateFile { path, data, set_modified_time, more_to_follow } => f.debug_struct("CreateOrUpdateFile").field("path", path).field("data", &format!("... ({})", HumanBytes(data.len() as u64))).field("set_modified_time", set_modified_time).field("more_to_follow", more_to_follow).finish(),
            Self::CreateSymlink { path, kind, target } => f.debug_struct("CreateSymlink").field("path", path).field("kind", kind).field("target", target).finish(),
            Self::CreateFolder { path } => f.debug_struct("CreateFolder").field("path", path).finish(),
            Self::DeleteFile { path } => f.debug_struct("DeleteFile").field("path", path).finish(),
            Self::DeleteFolder { path } => f.debug_struct("DeleteFolder").field("path", path).finish(),
            Self::DeleteSymlink { path, kind } => f.debug_struct("DeleteSymlink").field("path", path).field("kind", kind).finish(),
            Self::ProfilingTimeSync => write!(f, "ProfilingTimeSync"),
            Self::Marker(arg0) => f.debug_tuple("Marker").field(arg0).finish(),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// We need to distinguish what a symlink points to, as Windows filesystems
/// have this distinction and so we need to know when creating one on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymlinkKind {
    File, // A symlink that points to a file
    Folder, // A symlink that points to a folder
    Unknown, // Unix-only - a symlink that we couldn't determine the target type for, e.g. if it is broken.
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymlinkTarget {
    /// A symlink target which we identified as a relative path and converted the slashes to
    /// forward slashes, so it can be converted to the destination platform's local path syntax.
    Normalized(String),
    /// A symlink target which we couldn't normalize, e.g. because it is an absolute path.
    /// This is transferred without any changes.
    NotNormalized(String)
}

/// Details of a file or folder.
/// Note that this representation is consistent with the approach described in the README,
/// and so doesn't consider the name of the node to be part of the node itself.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EntryDetails {
    File {
        // Note that SystemTime is safe to serialize across platforms, because Serde serializes this 
        // as the elapsed time since UNIX_EPOCH, so it is platform-independent.
        modified_time: SystemTime,
        size: u64
    },
    Folder,
    Symlink {
        kind: SymlinkKind,
        target: SymlinkTarget,
    },
}

/// Responses are sent back from the doer to the boss to report on something, usually
/// the result of a Command.
#[derive(Serialize, Deserialize)]
pub enum Response {
    RootDetails {
        root_details: Option<EntryDetails>, // Option<> because the root might not exist at all
        /// Whether or not this platform differentiates between file and folder symlinks (e.g. Windows),
        /// vs. treating all symlinks the same (e.g. Linux).
        platform_differentiates_symlinks: bool,
        /// Forward vs backwards slash.
        platform_dir_separator: char,
    },

    // The result of GetEntries is split into lots of individual messages (rather than one big list)
    // so that the boss can start doing stuff before receiving the full list.
    Entry((RootRelativePath, EntryDetails)),
    EndOfEntries,

    FileContent {
        #[serde(with = "serde_bytes")] // Make serde fast
        data: Vec<u8>,
        /// If set, there is more data for this same file being sent in a following Response.
        /// This is used to split up large files so that we don't send them all in one huge message:
        ///   - better memory usage
        ///   - doesn't crash for really large files
        ///   - more opportunities for pipelining
        more_to_follow: bool,
    },

    ProfilingTimeSync(std::time::Duration),
    ProfilingData(ProcessProfilingData),

    /// The doer echoes back Marker commands, so the boss can keep track of the doer's progress.
    Marker(ProgressMarker),

    Error(String),
}
impl encrypted_comms::IsFinalMessage for Response {
    fn is_final_message(&self) -> bool {
        match self {
            Self::ProfilingData{..} => true,
            _ => false
        }
    }
}
// The default Debug implementation prints all the file data, which is way too much, so we have to override this :(
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note that rust-analyzer can auto-generate the complete version of this for us (delete the function, then Ctrl+Space),
        // then we can make the tweaks that we need.
        match self {
            Self::RootDetails { root_details, platform_differentiates_symlinks, platform_dir_separator } => f.debug_struct("RootDetails").field("root_details", root_details).field("platform_differentiates_symlinks", platform_differentiates_symlinks).field("platform_dir_separator", platform_dir_separator).finish(),
            Self::Entry(arg0) => f.debug_tuple("Entry").field(arg0).finish(),
            Self::EndOfEntries => write!(f, "EndOfEntries"),
            Self::FileContent { data, more_to_follow } => f.debug_struct("FileContent").field("data", &format!("... ({})", HumanBytes(data.len() as u64))).field("more_to_follow", more_to_follow).finish(),
            Self::ProfilingTimeSync(arg0) => f.debug_tuple("ProfilingTimeSync").field(arg0).finish(),
            Self::ProfilingData(_) => f.debug_tuple("ProfilingData").finish(),
            Self::Marker(arg0) => f.debug_tuple("Marker").field(arg0).finish(),
            Self::Error(arg0) => f.debug_tuple("Error").field(arg0).finish(),
        }
    }
}
