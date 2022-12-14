use std::{
    cmp::Ordering,
    fmt::{Display, Write}, time::{Instant, SystemTime}, collections::HashMap,
};

use console::{Style};
use indicatif::{HumanCount, HumanBytes};
use log::{debug, info, trace};
use regex::{RegexSet};

use crate::{*, boss_progress::{Progress}};

#[derive(Default)]
struct FileSizeHistogram {
    buckets: Vec<u32>,
}
impl FileSizeHistogram {
    fn add(&mut self, val: u64) {
        let bucket = (val as f64).log10() as usize;
        while self.buckets.len() <= bucket {
            self.buckets.push(0);
        }
        self.buckets[bucket] += 1;
    }
}
impl Display for FileSizeHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f)?;

        if self.buckets.is_empty() {
            writeln!(f, "Empty")?;
            return Ok(());
        }

        let h = 5;
        let max = *self.buckets.iter().max().unwrap();
        for y in 0..h {
            let mut l = "".to_string();
            for x in 0..self.buckets.len() {
                if self.buckets[x] as f32 / max as f32 > (h - y - 1) as f32 / h as f32 {
                    l += "#";
                } else {
                    l += " ";
                }
            }
            writeln!(f, "{}", l)?;
        }

        let mut l = "".to_string();
        for x in 0..self.buckets.len() {
            match x {
                3 => l += "K",
                6 => l += "M",
                9 => l += "G",
                _ => write!(&mut l, "{x}").unwrap(),
            }
        }
        writeln!(f, "{}", l)?;

        std::fmt::Result::Ok(())
    }
}

#[derive(Default)]
struct Stats {
    pub num_src_entries: u32,
    pub num_src_files: u32,
    pub num_src_folders: u32,
    pub num_src_symlinks: u32,
    pub src_total_bytes: u64,
    pub src_file_size_hist: FileSizeHistogram,

    pub num_dest_entries: u32,
    pub num_dest_files: u32,
    pub num_dest_folders: u32,
    pub num_dest_symlinks: u32,
    pub dest_total_bytes: u64,

    pub delete_start_time: Option<Instant>,
    pub num_files_deleted: u32,
    pub num_bytes_deleted: u64,
    pub num_folders_deleted: u32,
    pub num_symlinks_deleted: u32,
    pub delete_end_time: Option<Instant>,

    pub copy_start_time: Option<Instant>,
    pub num_files_copied: u32,
    pub num_bytes_copied: u64,
    pub num_folders_created: u32,
    pub num_symlinks_copied: u32,
    pub copied_file_size_hist: FileSizeHistogram,
    pub copy_end_time: Option<Instant>,
}

enum Side {
    Source,
    Dest
}

/// For user-friendly display of a RootRelativePath on the source or dest.
/// Formats a path which is relative to the root, so that it is easier to understand for the user.
/// Especially if path is empty (i.e. referring to the root itself)
struct PrettyPath<'a> {
    side: Side,
    dir_separator: char,
    root: &'a str,
    path: &'a RootRelativePath,
    kind: &'static str, // e.g. 'folder', 'file'
}
impl<'a> Display for PrettyPath<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let side = match self.side {
            Side::Source { .. } => "source",
            Side::Dest { .. } => "dest",
        };
        let root = self.root;
        let path = self.path;
        let kind = self.kind;

        // Use styling to highlight which part of the path is the root, and which is the root-relative path.
        // We don't play with any characters in the path (e.g. adding brackets) so that the user can copy-paste the 
        // full paths if they want
        // The styling plays nicely with piping output to a file, as they are simply ignored (part of the `console` crate)
        let root_style = Style::new().italic();
        if self.path.is_root() {
            write!(f, "{side} root {kind} '{}'", root_style.apply_to(root))
        } else {
            let root_with_trailing_slash = if root.ends_with(self.dir_separator) {
                root.to_string()
            } else {
                root.to_string() + &self.dir_separator.to_string()
            };
            // Convert the path from normalized (forward slashes) to the native representation for that platform
            let path_with_appropriate_slashes = path.to_platform_path(self.dir_separator);
            write!(f, "{side} {kind} '{}{path_with_appropriate_slashes}'", root_style.apply_to(root_with_trailing_slash))
        }
    }
}

/// Validates if a trailing slash was provided incorrectly on the given entry.
/// Referring to an existing file with a trailing slash is an error, because it implies
/// that the user thinks it is a folder, and so could lead to unwanted behaviour.
/// In some environments (e.g. Linux), this is caught on the doer side when it attempts to
/// get the metadata for the root, but on some environments it isn't caught (Windows, depending on the drive)
/// so we have to do our own check here.
fn validate_trailing_slash(root_path: &str, entry_details: &EntryDetails) -> Result<(), String> {
    // Note that we can't use std::path::is_separator because this might be a remote path, so the current platform
    // is irrelevant
    if matches!(entry_details, EntryDetails::File {..} | EntryDetails::Symlink { .. }) {
        // Note that we can't use std::path::is_separator because this might be a remote path, so the current platform
        // is irrelevant
        if root_path.chars().last().unwrap() == '/' || root_path.chars().last().unwrap() == '\\' {
            return Err(format!("'{}' is a file or symlink but is referred to with a trailing slash.", root_path));
        }
    }
    Ok(())
}

/// To increase performance, we don't wait for the dest doer to confirm that every command we sent has been
/// successfully completed before moving on to do something else (like sending the next command),
/// so any errors that result won't be picked up until we check later, which is what we do here. 
/// This is called periodically to make sure nothing has gone wrong.
/// It also handles progress bar updates, based on Marker commands that we send and the doer echoes back.
/// If block_until_done is set, then this function will keep processing messages (blocking as necessary)
/// until it finds a response with a progress marker that shows the doer is finished. 
/// If not set, this function won't block and will return once it's processed all pending responses from the doer.
/// If an error is encountered though, it will return rather than blocking.
fn process_dest_responses(dest_comms: &mut Comms, progress: &mut Progress, 
    mut block_until_done: bool) -> Result<(), String> 
{
    // To make the rest of this function consistent for both cases of block_until_done,
    // this helper function will block or not as appropriate.
    // It acts as an iterator, so returns None or Some.
    let next_fn = |block_until_done| {
        if block_until_done {
            Some(dest_comms.receive_response()) // Blocking, as we need to wait until we find the Done marker value
        } else {
            match dest_comms.try_receive_response() { // Non-blocking
                Ok(Some(r)) => Some(Ok(r)),
                Ok(None) => None,
                Err(e) => Some(Err(e))
            }
        }
    };

    let mut errors = vec![]; // There might be multiple errors reported before we get round to checking for them
    while let Some(x) = next_fn(block_until_done) {
        match x {
            Ok(Response::Error(e)) => {
                errors.push(e);
                // If an error was encountered, don't block - just process the remaining messages to see if there 
                // were any other errors to report, then return the error(s)
                block_until_done = false; 
            }
            Ok(Response::Marker(m)) => {
                // Update the progress bar based on the progress that the dest doer has made.
                progress.update_completed(&m);
                if m.phase == ProgressPhase::Done {
                    break;
                }
            }
            Err(e) => {
                // Communications error - return immediately as we won't be able to receive any more messages and doing
                // so might lead to an infinite loop
                errors.push(format!("{}", e));
                break;
            }
            _ => errors.push(format!("Unexpected response (expected Error or Marker): {:?}", x)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(", "))
    }
}

/// A bunch of fields related to the current sync that would otherwise need to be passed
/// around as individual variables.
struct SyncContext<'a> {
    src_comms: &'a mut Comms,
    dest_comms: &'a mut Comms,
    filters: Filters,
    stats: Stats,
    dry_run: bool,
    dest_file_newer_behaviour: DestFileUpdateBehaviour,
    dest_file_older_behaviour: DestFileUpdateBehaviour,
    dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour,
    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour,
    show_stats: bool,
    src_root: String,
    dest_root: String,
    progress: Progress,

    // Used for debugging/display only, shouldn't be needed for any syncing logic
    src_dir_separator: Option<char>,
    dest_dir_separator: Option<char>,
}
impl<'a> SyncContext<'a> {
    fn pretty_src<'b>(&'b self, path: &'b RootRelativePath, details: &'b EntryDetails) -> PrettyPath {
        let kind = match details {
            EntryDetails::File { .. } => "file",
            EntryDetails::Folder => "folder",
            EntryDetails::Symlink { .. } => "symlink",
        };
        self.pretty_src_kind(path, kind)
    }
    fn pretty_dest<'b>(&'b self, path: &'b RootRelativePath, details: &'b EntryDetails) -> PrettyPath {
        let kind = match details {
            EntryDetails::File { .. } => "file",
            EntryDetails::Folder => "folder",
            EntryDetails::Symlink { .. } => "symlink",
        };
        self.pretty_dest_kind(path, kind)
    }
    fn pretty_src_kind<'b>(&'b self, path: &'b RootRelativePath, kind: &'static str) -> PrettyPath {
        PrettyPath { side: Side::Source, dir_separator: self.src_dir_separator.unwrap_or('/'), root: &self.src_root, path, kind }
    }
    fn pretty_dest_kind<'b>(&'b self, path: &'b RootRelativePath, kind: &'static str) -> PrettyPath {
        PrettyPath { side: Side::Dest, dir_separator: self.dest_dir_separator.unwrap_or('/'), root: &self.dest_root, path, kind }
    }
}

pub fn sync(
    sync_spec: &SyncSpec,
    dry_run: bool,
    show_stats: bool,
    src_comms: &mut Comms,
    dest_comms: &mut Comms,
) -> Result<(), String> {
    // Parse and compile the filter strings
    let filters = {
        let mut patterns = vec![];
        let mut kinds = vec![];
        for f in &sync_spec.filters {
            // Check if starts with a + (include) or a - (exclude)
            match f.chars().nth(0) {
                Some('+') => kinds.push(FilterKind::Include),
                Some('-') => kinds.push(FilterKind::Exclude),
                _ => return Err(format!("Invalid filter '{}': Must start with a '+' or '-'", f)),
            }
            let pattern = f.split_at(1).1.to_string();
            // Wrap in ^...$ to make it match the whole string, otherwise it's too easy
            // to make a mistake with filters that unintentionally match something else
            let pattern = format!("^{pattern}$");
            patterns.push(pattern);
        }
        let regex_set = match RegexSet::new(patterns) {
            Ok(r) => r,
            Err(e) => {
                // Note that the error reported by RegexSet includes the pattern being compiled, so we don't need to duplicate this
                return Err(format!("Invalid filter: {e}"));
            }
        };
        Filters { regex_set, kinds }
    };

    // Make context object, to avoid having to pass around a bunch of individual variables everywhere
    let context = SyncContext {
        src_comms,
        dest_comms,
        filters,
        stats: Stats::default(),
        dry_run,
        show_stats,
        dest_file_newer_behaviour: sync_spec.dest_file_newer_behaviour,
        dest_file_older_behaviour: sync_spec.dest_file_older_behaviour,
        dest_entry_needs_deleting_behaviour: sync_spec.dest_entry_needs_deleting_behaviour,
        dest_root_needs_deleting_behaviour: sync_spec.dest_root_needs_deleting_behaviour,
        src_root: sync_spec.src.clone(),
        dest_root: sync_spec.dest.clone(),
        progress: Progress::new(),
        src_dir_separator: None,
        dest_dir_separator: None,
    };
    // Call into separate function, to avoid the original function parameters being mis-used instead
    // of the context fields
    sync_impl(context)
}

fn sync_impl(mut ctx: SyncContext) -> Result<(), String> {
    profile_this!();

    let sync_start = Instant::now();

    // First get details of the root file/folder etc. of each side, as this might affect the sync
    // before we start it (e.g. errors, or changing the dest root)

    // Source SetRoot
    let timer = start_timer("SetRoot src");
    ctx.src_comms.send_command(Command::SetRoot { root: ctx.src_root.to_string() })?;
    let src_root_details = match ctx.src_comms.receive_response()? {
        Response::RootDetails { root_details, platform_differentiates_symlinks: _, platform_dir_separator } => {
            match &root_details {
                None => return Err(format!("src path '{}' doesn't exist!", ctx.src_root)),
                Some(d) => if let Err(e) = validate_trailing_slash(&ctx.src_root, &d) {
                    return Err(format!("src path {}", e));
                }
            };
            ctx.src_dir_separator = Some(platform_dir_separator);
            root_details
        }
        r => return Err(format!("Unexpected response getting root details from src: {:?}", r)),
    };
    let src_root_details = src_root_details.unwrap();
    stop_timer(timer);

    // Dest SetRoot
    let timer = start_timer("SetRoot dest");
    ctx.dest_comms.send_command(Command::SetRoot { root: ctx.dest_root.clone() })?;
    let (mut dest_root_details, dest_platform_differentiates_symlinks) = match ctx.dest_comms.receive_response()? {
        Response::RootDetails { root_details, platform_differentiates_symlinks, platform_dir_separator } => {
            match &root_details {
                None => (), // Dest root doesn't exist, but that's fine (we will create it later)
                Some(d) => if let Err(e) = validate_trailing_slash(&ctx.dest_root, &d) {
                    return Err(format!("dest path {}", e));
                }
            }
            ctx.dest_dir_separator = Some(platform_dir_separator);
            (root_details, platform_differentiates_symlinks)
        }
        r => return Err(format!("Unexpected response getting root details from dest: {:?}", r)),
    };
    stop_timer(timer);

    // If src is a file (or symlink, which we treat as a file), and the dest path ends in a slash, 
    // then we want to sync the file _inside_ the folder, rather then replacing the folder with the file
    // (see README for reasoning).
    // To do this, we modify the dest path and then continue as if that was the path provided by the
    // user.
    let last_dest_char = ctx.dest_root.chars().last();
    // Note that we can't use std::path::is_separator (or similar) because this might be a remote path, so the current platform
    // isn't appropriate.
    let dest_trailing_slash = last_dest_char == Some('/') || last_dest_char == Some('\\');
    if matches!(src_root_details, EntryDetails::File {..} | EntryDetails::Symlink { .. }) && dest_trailing_slash {
        let src_filename = ctx.src_root.split(|c| c == '/' || c == '\\').last();
        if let Some(c) = src_filename {
            ctx.dest_root = ctx.dest_root + c;
            debug!("Modified dest path to {}", ctx.dest_root);

            ctx.dest_comms.send_command(Command::SetRoot { root: ctx.dest_root.clone() })?;
            dest_root_details = match ctx.dest_comms.receive_response()? {
                Response::RootDetails { root_details, platform_differentiates_symlinks: _, platform_dir_separator: _ } => root_details,
                r => return Err(format!("Unexpected response getting root details from dest: {:?}", r)),
            }
        }
    }

    // Check if the dest root will need deleting, and potentially prompt the user.
    // We need to do this explicitly before starting to delete anything 
    // because we delete in reverse order, and we would end up deleting everything inside the 
    // dest root folder before getting to the root itself, so the prompt/error would be too late!
    if let Some(d) = &dest_root_details {
        if should_delete(&src_root_details, d, dest_platform_differentiates_symlinks) {
            let msg = format!(
                "{} needs deleting as it is incompatible with {}",
                ctx.pretty_dest(&RootRelativePath::root(), d),
                ctx.pretty_src(&RootRelativePath::root(), &src_root_details));
            let resolved_behaviour = match ctx.dest_root_needs_deleting_behaviour {
                DestRootNeedsDeletingBehaviour::Prompt => {
                    let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                        Some(&ctx.progress), 
                        &[
                            ("Skip", DestRootNeedsDeletingBehaviour::Skip),
                            ("Delete", DestRootNeedsDeletingBehaviour::Delete),
                            ], false, DestRootNeedsDeletingBehaviour::Error);
                    prompt_result.immediate_behaviour
                },
                x => x,
            };
            match resolved_behaviour {
                DestRootNeedsDeletingBehaviour::Prompt => panic!("Should have been alredy resolved!"),
                DestRootNeedsDeletingBehaviour::Error => return Err(format!("{msg}. Will not delete. See --dest-root-needs-deleting")),
                DestRootNeedsDeletingBehaviour::Skip => return Ok(()), // Don't raise an error, but we can't continue as it will fail, so skip the entire sync
                DestRootNeedsDeletingBehaviour::Delete => (), // We will delete it anyway later on
            }
        }
    }


    // If the dest doesn't yet exist, make sure that all its ancestors are created, so that
    // when we come to create the dest path itself, it can succeed
    if dest_root_details.is_none() {
        if !ctx.dry_run {
            ctx.dest_comms.send_command(Command::CreateRootAncestors)?;
        }
    }

    // Fetch all the entries for the source path and the dest path, if they are folders
    // Send off both GetEntries commands and wait for the results in parallel, rather than doing
    // one after the other (for performance)
    let timer = start_timer("GetEntries x 2");

    // Source GetEntries
    let mut src_entries = Vec::new();
    let mut src_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();
    let mut src_done = true;

    // Add the root entry - we already got the details for this before
    src_entries.push((RootRelativePath::root(), src_root_details.clone()));
    src_entries_lookup.insert(RootRelativePath::root(), src_root_details.clone());
    // Assume the worse case that we will need to copy every src entry to calculate an initial total progress
    ctx.progress.inc_total_for_copy(&src_root_details);

    if matches!(src_root_details, EntryDetails::Folder) {
        ctx.src_comms.send_command(Command::GetEntries { filters: ctx.filters.clone() })?;
        src_done = false;
    }

    // Dest GetEntries
    let mut dest_entries = Vec::new();
    let mut dest_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();
    let mut dest_done = true;

    // Add the root entry - we already got the details for this before
    if let Some(d) = &dest_root_details {
        // Assume the worse case that we will need to delete every dest entry to calculate an initial total progress
        ctx.progress.inc_total_for_delete(&d);
        dest_entries.push((RootRelativePath::root(), d.clone()));
        dest_entries_lookup.insert(RootRelativePath::root(), d.clone());
    }

    if matches!(dest_root_details, Some(EntryDetails::Folder)) {
        ctx.dest_comms.send_command(Command::GetEntries { filters: ctx.filters.clone() })?;
        dest_done = false;
    }

    while !src_done || !dest_done {
        // Wait for either src or dest to send us a response with an entry
        match memory_bound_channel::select_ready(ctx.src_comms.get_receiver(), ctx.dest_comms.get_receiver()) {
            0 => match ctx.src_comms.receive_response()? {
                Response::Entry((p, d)) => {
                    trace!("Source entry '{}': {:?}", p, d);
                    match d {
                        EntryDetails::File { size, .. } => {
                            ctx.stats.num_src_files += 1;
                            ctx.stats.src_total_bytes += size;
                            ctx.stats.src_file_size_hist.add(size);
                        }
                        EntryDetails::Folder => ctx.stats.num_src_folders += 1,
                        EntryDetails::Symlink { .. } => ctx.stats.num_src_symlinks += 1,
                    }
                    // Assume the worse case that we will need to copy every src entry to calculate an initial total progress
                    ctx.progress.inc_total_for_copy(&d);
                    src_entries.push((p.clone(), d.clone()));
                    src_entries_lookup.insert(p, d);
                }
                Response::EndOfEntries => src_done = true,
                r => return Err(format!("Unexpected response getting entries from src: {:?}", r)),
            },
            1 => match ctx.dest_comms.receive_response()? {
                Response::Entry((p, d)) => {
                    trace!("Dest entry '{}': {:?}", p, d);
                    match d {
                        EntryDetails::File { size, .. } => {
                            ctx.stats.num_dest_files += 1;
                            ctx.stats.dest_total_bytes += size;
                        }
                        EntryDetails::Folder => ctx.stats.num_dest_folders += 1,
                        EntryDetails::Symlink { .. } => ctx.stats.num_dest_symlinks += 1,
                    }
                    // Assume the worse case that we will need to delete every dest entry to calculate an initial total progress
                    ctx.progress.inc_total_for_delete(&d);
                    dest_entries.push((p.clone(), d.clone()));
                    dest_entries_lookup.insert(p, d);
                }
                Response::EndOfEntries => dest_done = true,
                r => return Err(format!("Unexpected response getting entries from dest: {:?}", r)),
            },
            _ => panic!("Invalid index"),
        }
    }
    ctx.stats.num_src_entries = src_entries.len() as u32;
    ctx.stats.num_dest_entries = dest_entries.len() as u32;

    stop_timer(timer);

    let query_elapsed = sync_start.elapsed().as_secs_f32();

    if ctx.show_stats {
        info!("Source: {} file(s) totalling {}, {} folder(s) and {} symlink(s)",
            HumanCount(ctx.stats.num_src_files as u64),
            HumanBytes(ctx.stats.src_total_bytes),
            HumanCount(ctx.stats.num_src_folders as u64),
            HumanCount(ctx.stats.num_src_symlinks as u64),
        );
        info!("  =>");
        info!("Dest: {} file(s) totalling {}, {} folder(s) and {} symlink(s)",
            HumanCount(ctx.stats.num_dest_files as u64),
            HumanBytes(ctx.stats.dest_total_bytes),
            HumanCount(ctx.stats.num_dest_folders as u64),
            HumanCount(ctx.stats.num_dest_symlinks as u64),
        );
        info!("Source file size distribution:");
        info!("{}", ctx.stats.src_file_size_hist);
        info!("Queried in {:.2} seconds", query_elapsed);
    }

    // Delete dest entries that don't exist on the source. This needs to be done first in case there
    // are entries with the same name but incompatible (e.g. files vs folders).
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly would also have problems with
    // files being filtered so the folder is needed still as there are filtered-out files in there,
    // see test_remove_dest_folder_with_excluded_files())

    // Update progress to start the deleting phase
    ctx.progress.update_completed(&ProgressMarker { completed_work: 0, phase: ProgressPhase::Deleting { num_entries_deleted: 0 }});

    {
        profile_this!("Sending delete commands");
        ctx.stats.delete_start_time = Some(Instant::now());
        for (dest_path, dest_details) in dest_entries.iter().rev() {
            let s = src_entries_lookup.get(dest_path);
            if !s.is_some() || should_delete(s.unwrap(), dest_details, dest_platform_differentiates_symlinks) {
                delete_dest_entry(&mut ctx, dest_details, dest_path)?;
            } else {
                // No need to delete this entry, so we can reduce the total progress
                ctx.progress.dec_total_for_delete(&dest_details); 
            }
        }
    }

    // Copy entries that don't exist, or do exist but are out-of-date.
    {
        profile_this!("Sending copy commands");
        // Mark the exact start of copying, to make sure our timing stats are split accurately between copying and deleting
        ctx.dest_comms.send_command(Command::Marker(ctx.progress.get_progress_marker()))?; 
        for (path, src_details) in src_entries {
            let dest_details = dest_entries_lookup.get(&path);
            match dest_details {
                Some(dest_details) if !should_delete (&src_details, dest_details, dest_platform_differentiates_symlinks) => {
                    // Dest already has this entry - check if it is up-to-date
                    match src_details {
                        EntryDetails::File { size, modified_time: src_modified_time } => {
                            let dest_modified_time = match dest_details {
                                EntryDetails::File { modified_time, .. } => modified_time,
                                _ => panic!("Wrong entry type"), // This should never happen as we check the type in the .find() above
                            };
                            handle_existing_file(&path, &src_details, size, &mut ctx, src_modified_time,
                                *dest_modified_time)?;
                        },
                        EntryDetails::Folder |  // Folders are always up-to-date
                        EntryDetails::Symlink { .. }  // Symlinks are always up-to-date, if should_delete indicated that we shouldn't delete it
                        => {
                            trace!("{} already exists at {} - nothing to do", 
                                ctx.pretty_src(&path, &src_details),
                                ctx.pretty_dest(&path, dest_details));
                            // No need to copy this entry, so we can reduce the total progress
                            ctx.progress.dec_total_for_copy(&src_details); 
                        },
                    }
                },
                _ => match src_details {
                    EntryDetails::File { size, modified_time: src_modified_time } => {
                        debug!("{} doesn't exist on dest - copying", ctx.pretty_src(&path, &src_details));
                        copy_file(&path, &src_details, size, src_modified_time, &mut ctx)?
                    }
                    EntryDetails::Folder => {
                        debug!("{} doesn't exist on dest - creating", ctx.pretty_src(&path, &src_details));
                        ctx.stats.num_folders_created += 1;
                        if !ctx.dry_run {
                            ctx.dest_comms
                                .send_command(Command::CreateFolder {
                                    path: path.clone(),
                                })?;
                        } else {
                            // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                            info!("Would create {}", ctx.pretty_dest_kind(&path, "folder"));
                        }
                        if let Some(m) = ctx.progress.copy_sent(&src_details) {
                            ctx.dest_comms.send_command(Command::Marker(m))?;                         
                        }
                    },
                    EntryDetails::Symlink { ref kind, ref target } => {
                        debug!("{} doesn't exist on dest - copying", ctx.pretty_src(&path, &src_details));
                        ctx.stats.num_symlinks_copied += 1;
                        if !ctx.dry_run {
                            ctx.dest_comms
                                .send_command(Command::CreateSymlink {
                                    path: path.clone(),
                                    kind: *kind,
                                    target: target.clone(),
                                })?;
                        } else {
                            // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                            info!("Would create {}", ctx.pretty_dest_kind(&path, "symlink"));
                        }
                        if let Some(m) = ctx.progress.copy_sent(&src_details) {
                            ctx.dest_comms.send_command(Command::Marker(m))?;                         
                        }
                    }
                },
            }
            process_dest_responses(ctx.dest_comms, &mut ctx.progress, false)?;
        }
    }

    // Wait for the dest doer to finish processing all its Commands so that everything is finished.
    // We don't need to wait for the src doer, because the dest doer is always last to finish.
    let m = ctx.progress.all_work_sent();
    ctx.dest_comms.send_command(Command::Marker(m))?;
    {
        profile_this!("Waiting for dest to finish");
        process_dest_responses(ctx.dest_comms, &mut ctx.progress, true)?;
    }

    ctx.stats.delete_end_time = ctx.progress.get_first_copy_time();
    ctx.stats.copy_start_time = ctx.progress.get_first_copy_time();
    ctx.stats.copy_end_time = Some(Instant::now());

    // Note that we print all the stats at the end (even though we could print the delete stats earlier),
    // so that they are together in the output (e.g. for dry run or --verbose, they could be a lot of other
    // messages between them)
    if (ctx.stats.num_files_deleted + ctx.stats.num_folders_deleted + ctx.stats.num_symlinks_deleted > 0) || ctx.show_stats {
        let delete_elapsed = ctx.stats.delete_end_time.unwrap() - ctx.stats.delete_start_time.unwrap();
        info!(
            "{} {} file(s) totalling {}, {} folder(s) and {} symlink(s){}",
            if !ctx.dry_run { "Deleted" } else { "Would delete" },
            HumanCount(ctx.stats.num_files_deleted as u64),
            HumanBytes(ctx.stats.num_bytes_deleted),
            HumanCount(ctx.stats.num_folders_deleted as u64),
            HumanCount(ctx.stats.num_symlinks_deleted as u64),
            if !ctx.dry_run && ctx.show_stats {
                format!(", in {:.2} seconds", delete_elapsed.as_secs_f32())
            } else { "".to_string() },
        );
    }
    if (ctx.stats.num_files_copied + ctx.stats.num_folders_created + ctx.stats.num_symlinks_copied > 0) || ctx.show_stats {
        let copy_elapsed = ctx.stats.copy_end_time.unwrap() - ctx.stats.copy_start_time.unwrap();
        info!(
            "{} {} file(s) totalling {}, {} {} folder(s) and {} {} symlink(s){}",
            if !ctx.dry_run { "Copied" } else { "Would copy" },
            HumanCount(ctx.stats.num_files_copied as u64),
            HumanBytes(ctx.stats.num_bytes_copied),
            if !ctx.dry_run { "created" } else { "would create" },
            HumanCount(ctx.stats.num_folders_created as u64),
            if !ctx.dry_run { "copied" } else { "would copy" },
            HumanCount(ctx.stats.num_symlinks_copied as u64),
            if !ctx.dry_run && ctx.show_stats {
                format!(", in {:.2} seconds ({}/s)",
                    copy_elapsed.as_secs_f32(), HumanBytes((ctx.stats.num_bytes_copied as f32 / copy_elapsed.as_secs_f32()).round() as u64))
            } else { "".to_string() },
        );
        if ctx.show_stats {
            info!("{} file size distribution:",
                if !ctx.dry_run { "Copied" } else { "Would copy" },
            );
            info!("{}", ctx.stats.copied_file_size_hist);
        }
    }
    if ctx.stats.num_files_deleted
        + ctx.stats.num_folders_deleted
        + ctx.stats.num_symlinks_deleted
        + ctx.stats.num_files_copied
        + ctx.stats.num_folders_created
        + ctx.stats.num_symlinks_copied
        == 0
    {
        info!("Nothing to do!");
    }

    Ok(())
}

/// Checks if a given src entry could be updated to match the dest, or if it needs
/// to be deleted and recreated instead.
pub fn should_delete(src: &EntryDetails, dest: &EntryDetails, dest_platform_differentiates_symlinks: bool) -> bool {
    match src {
        EntryDetails::File { .. } => match dest {
            EntryDetails::File { .. } => false,
            _ => true,
        },
        EntryDetails::Folder => match dest {
            EntryDetails::Folder => false,
            _ => true,
        },
        EntryDetails::Symlink { kind: src_kind, target: src_target } => match dest {
            EntryDetails::Symlink { kind: dest_kind, target: dest_target } => {
                if src_target != dest_target {
                    true
                } else if src_kind != dest_kind && dest_platform_differentiates_symlinks {
                    true
                } else {
                    false
                }
            }
            _ => true,
        }
    }
}

fn delete_dest_entry(ctx: &mut SyncContext, dest_details: &EntryDetails, dest_path: &RootRelativePath) -> Result<(), String> {
    let msg = format!(
        "{} needs deleting as it doesn't exist on the src (or is incompatible)",
        ctx.pretty_dest(dest_path, dest_details));

    // Resolve any behaviour resulting from a prompt first
    let resolved_behaviour = match ctx.dest_entry_needs_deleting_behaviour {
        DestEntryNeedsDeletingBehaviour::Prompt => {
            let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                Some(&ctx.progress), 
                &[
                    ("Skip", DestEntryNeedsDeletingBehaviour::Skip),
                    ("Delete", DestEntryNeedsDeletingBehaviour::Delete),
                ], true, DestEntryNeedsDeletingBehaviour::Error);
            if let Some(b) = prompt_result.remembered_behaviour {
                ctx.dest_entry_needs_deleting_behaviour = b;
            }
            prompt_result.immediate_behaviour
        },
        x => x,
    };
    match resolved_behaviour {
        DestEntryNeedsDeletingBehaviour::Prompt => panic!("Should have already been resolved!"),
        DestEntryNeedsDeletingBehaviour::Error => return Err(format!(
            "{msg}. Will not delete. See --dest-entry-needs-deleting.",
        )),
        DestEntryNeedsDeletingBehaviour::Skip => {
            trace!("{msg}. Skipping.");
            // No need to delete this entry, so we can reduce the total progress
            ctx.progress.dec_total_for_delete(&dest_details); 
            Ok(())
        }
        DestEntryNeedsDeletingBehaviour::Delete => {
            trace!("{msg}. Deleting.");
            let c = match dest_details {
                EntryDetails::File { size, .. } => {
                    ctx.stats.num_files_deleted += 1;
                    ctx.stats.num_bytes_deleted += size;
                    Command::DeleteFile {
                        path: dest_path.clone(),
                    }
                }
                EntryDetails::Folder => {
                    ctx.stats.num_folders_deleted += 1;
                    Command::DeleteFolder {
                        path: dest_path.clone(),
                    }
                }
                EntryDetails::Symlink { kind, .. } => {
                    ctx.stats.num_symlinks_deleted += 1;
                    Command::DeleteSymlink {
                        path: dest_path.clone(),
                        kind: *kind,
                    }
                }
            };
            let result = Ok(if !ctx.dry_run {
                ctx.dest_comms.send_command(c)?;

                process_dest_responses(ctx.dest_comms, &mut ctx.progress, false)?;
            } else {
                // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be deleted
                info!("Would delete {}", ctx.pretty_dest(dest_path, dest_details));
            });
            
            if let Some(m) = ctx.progress.delete_sent(&dest_details) {
                ctx.dest_comms.send_command(Command::Marker(m))?;                         
            }

            result
        }
    }
}

fn handle_existing_file(
    path: &RootRelativePath,
    src_entry: &EntryDetails,
    size: u64,
    ctx: &mut SyncContext,
    src_modified_time: SystemTime, dest_modified_time: SystemTime,
) -> Result<(), String> {
    let copy = match src_modified_time.cmp(&dest_modified_time) {
        Ordering::Less => {
            let msg = format!(
                "{} is newer than {}",
                ctx.pretty_dest_kind(&path, "file"),
                ctx.pretty_src_kind(&path, "file"));
            // Resolve any behaviour resulting from a prompt first
            let resolved_behaviour = match ctx.dest_file_newer_behaviour {
                DestFileUpdateBehaviour::Prompt => {
                    let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                        Some(&ctx.progress), 
                        &[
                            ("Skip", DestFileUpdateBehaviour::Skip),
                            ("Overwrite", DestFileUpdateBehaviour::Overwrite),
                        ], true, DestFileUpdateBehaviour::Error);
                    if let Some(b) = prompt_result.remembered_behaviour {
                        ctx.dest_file_newer_behaviour = b;
                    }
                    prompt_result.immediate_behaviour
                },
                x => x,
            };
            match resolved_behaviour {
                DestFileUpdateBehaviour::Prompt => panic!("Should have already been resolved!"),
                DestFileUpdateBehaviour::Error => return Err(format!(
                    "{msg}. Will not overwrite. See --dest-file-newer."
                )),
                DestFileUpdateBehaviour::Skip => {
                    trace!("{msg}. Skipping.");
                    false
                }
                DestFileUpdateBehaviour::Overwrite => {
                    trace!("{msg}. Overwriting anyway.");
                    true
                }
            }
        }
        Ordering::Equal => {
            trace!("{} has same modified time as {}. Will not update.",
                ctx.pretty_dest_kind(&path, "file"),
                ctx.pretty_src_kind(&path, "file"));
            false
        }
        Ordering::Greater => {
            let msg = format!(
                "{} is older than {}",
                ctx.pretty_dest_kind(&path, "file"),
                ctx.pretty_src_kind(&path, "file"));
            // Resolve any behaviour resulting from a prompt first
            let resolved_behaviour = match ctx.dest_file_older_behaviour {
                DestFileUpdateBehaviour::Prompt => {
                    let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                        Some(&ctx.progress), 
                        &[
                            ("Skip", DestFileUpdateBehaviour::Skip),
                            ("Overwrite", DestFileUpdateBehaviour::Overwrite),
                        ], true, DestFileUpdateBehaviour::Error);
                    if let Some(b) = prompt_result.remembered_behaviour {
                        ctx.dest_file_older_behaviour = b;
                    }
                    prompt_result.immediate_behaviour
                },
                x => x,
            };
            match resolved_behaviour {
                DestFileUpdateBehaviour::Prompt => panic!("Should have already been resolved!"),
                DestFileUpdateBehaviour::Error => return Err(format!(
                    "{msg}. Will not overwrite. See --dest-file-older."
                )),
                DestFileUpdateBehaviour::Skip => {
                    trace!("{msg}. Skipping.");
                    false
                }
                DestFileUpdateBehaviour::Overwrite => {
                    trace!("{msg}. Overwriting.");
                    true
                }
            }
        }
    };
    if copy {                    
        copy_file(&path, src_entry, size, src_modified_time, ctx)?
    } else {
        // No need to copy this entry, so we can reduce the total progress
        ctx.progress.dec_total_for_copy(src_entry); 
    }
    Ok(())
}

fn copy_file(
    path: &RootRelativePath,
    src_entry: &EntryDetails,
    size: u64,
    modified_time: SystemTime,
    ctx: &mut SyncContext) -> Result<(), String> 
{
    if !ctx.dry_run {
        trace!("Fetching from {}", ctx.pretty_src_kind(&path, "file"));
        ctx.src_comms
            .send_command(Command::GetFileContent {
                path: path.clone(),
            })?;
        // Large files are split into chunks, loop until all chunks are transferred.
        loop {
            let (data, more_to_follow) = match ctx.src_comms.receive_response()? {
                Response::FileContent { data, more_to_follow } => (data, more_to_follow),
                x => return Err(format!(
                    "Unexpected response fetching {}: {:?}", ctx.pretty_src_kind(&path, "file"), x
                )),
            };
            trace!("Create/update {}", ctx.pretty_dest_kind(&path, "file"));
            ctx.dest_comms
                .send_command(Command::CreateOrUpdateFile {
                    path: path.clone(),
                    data,
                    set_modified_time: if more_to_follow { None } else { Some(modified_time) }, // Only set the modified time after the final chunk
                    more_to_follow,
                })?;

            //TODO: send some progress updates partway through the file so we can see the bytes increase!

            // For large files, it might be a while before process_dest_responses is called in the main sync function,
            // so check it periodically here too. 
            process_dest_responses(ctx.dest_comms, &mut ctx.progress, false)?;

            if !more_to_follow {
                break;
            }
        }
    } else {
        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
        info!("Would copy {} => {}",
            ctx.pretty_src_kind(&path, "file"),
            ctx.pretty_dest_kind(&path, "file"));
    }

    ctx.stats.num_files_copied += 1;
    ctx.stats.num_bytes_copied += size;
    ctx.stats.copied_file_size_hist.add(size);

    //TODO: this needs to be inside the chunking loop so we can update progress as the file is copied?
    if let Some(m) = ctx.progress.copy_sent(src_entry) {
        ctx.dest_comms.send_command(Command::Marker(m))?;                         
    }

    Ok(())
}
