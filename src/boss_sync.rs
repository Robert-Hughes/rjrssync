use std::{
    cmp::Ordering, time::{Instant, SystemTime, Duration},
};

use indicatif::{HumanCount, HumanBytes, ProgressBar};
use log::{debug, info, trace};
use regex::{RegexSet};

use crate::{*, boss_progress::{Progress}, histogram::FileSizeHistogram, root_relative_path::{RootRelativePath, PrettyPath, Side}, boss_doer_interface::{ProgressPhase, EntryDetails, Response, Command, Filters, FilterKind}, ordered_map::OrderedMap};

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
    progress: Option<Progress>, //TODO: unwrap() everywhere yuck!

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

    fn send_progress_marker_limited(&mut self,) -> Result<(), String> {
        if let Some(m) = self.progress.as_mut().unwrap().get_progress_marker_limited() {
            self.dest_comms.send_command(Command::Marker(m))
        } else {
            Ok(())
        }
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
    let filters = match compile_filters(sync_spec) {
        Ok(f) => f,
        Err(e) => return Err(e),
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
        progress: None,
        src_dir_separator: None,
        dest_dir_separator: None,
    };
    // Call into separate function, to avoid the original function parameters being mis-used instead
    // of the context fields
    sync_impl(context)
}

fn compile_filters(sync_spec: &SyncSpec) -> Result<Filters, String> {
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
    Ok(Filters { regex_set, kinds })
}

fn sync_impl(mut ctx: SyncContext) -> Result<(), String> {
    profile_this!();

    let sync_start = Instant::now();

    let progress_bar = ProgressBar::new_spinner().with_message("Querying...");
    progress_bar.enable_steady_tick(Duration::from_millis(100));
    //TODO: remove the one from boss_progress - don't create the Progress object til later,
    // and it no longer needs to be aware of the querying phase!

    // First get details of the root file/folder etc. of each side, as this might affect the sync
    // before we start it (e.g. errors, or changing the dest root)
    let (src_root_details, dest_root_details, dest_platform_differentiates_symlinks) = get_root_details(&mut ctx)?;

    // Check if the dest root will need deleting, and potentially prompt the user.
    // We do this before we start querying everything to show this prompt as soon as possible.
    if let Some(d) = &dest_root_details {
        if needs_delete(&ctx, &RootRelativePath::root(), &src_root_details, d, dest_platform_differentiates_symlinks) {
            if !check_dest_root_delete_ok(&mut ctx, &src_root_details, d)? {
                // Don't raise an error if we've been told to skip, but we can't continue as it will fail, so skip the entire sync
                return Ok(());
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

    let mut actions = query_entries(&mut ctx, src_root_details, dest_root_details, dest_platform_differentiates_symlinks)?;

    // Stop the progress bar before we (potentially) prompt the user, so the progress bar
    // redrawing doesn't interfere with the prompts
    progress_bar.finish_and_clear();

    // Confirm that the user is happy to take these actions
    confirm_actions(&mut ctx, &mut actions)?;

    let query_elapsed_secs = sync_start.elapsed().as_secs_f32();
    show_post_query_stats(&ctx, query_elapsed_secs);

    // Delete dest entries that don't exist on the source. This needs to be done first in case there
    // are entries with the same name but incompatible (e.g. files vs folders).

    // Update progress to start the deleting phase
    ctx.progress = Some(Progress::new(&actions));

    {
        profile_this!("Sending delete commands");
        ctx.stats.delete_start_time = Some(Instant::now());
        for (dest_path, (dest_details, _reason)) in actions.to_delete.iter() {
            delete_dest_entry(&mut ctx, dest_details, dest_path)?;

            process_dest_responses(ctx.dest_comms, &mut ctx.progress.as_mut().unwrap(), false)?;
        }
    }

    // Copy entries that don't exist, or do exist but are out-of-date.
    {
        profile_this!("Sending copy commands");
        // Mark the exact start of copying, to make sure our timing stats are split accurately between copying and deleting
        ctx.dest_comms.send_command(Command::Marker(ctx.progress.as_mut().unwrap().get_progress_marker()))?;
        for (src_path, (src_details, _reason)) in actions.to_copy.iter() {
            copy_entry(&mut ctx, &src_path, &src_details)?;

            process_dest_responses(ctx.dest_comms, &mut ctx.progress.as_mut().unwrap(), false)?;
        }
    }

    // Wait for the dest doer to finish processing all its Commands so that everything is finished.
    // We don't need to wait for the src doer, because the dest doer is always last to finish.
    let m = ctx.progress.as_mut().unwrap().all_work_sent();
    ctx.dest_comms.send_command(Command::Marker(m))?;
    {
        profile_this!("Waiting for dest to finish");
        process_dest_responses(ctx.dest_comms, &mut ctx.progress.as_mut().unwrap(), true)?;
    }

    ctx.stats.delete_end_time = ctx.progress.as_mut().unwrap().get_first_copy_time();
    ctx.stats.copy_start_time = ctx.progress.as_mut().unwrap().get_first_copy_time();
    ctx.stats.copy_end_time = Some(Instant::now());

    show_post_sync_stats(&ctx);

    Ok(())
}

fn get_root_details(ctx: &mut SyncContext) -> Result<(EntryDetails, Option<EntryDetails>, bool), String> {
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
            ctx.dest_root = ctx.dest_root.clone() + c;
            debug!("Modified dest path to {}", ctx.dest_root);

            ctx.dest_comms.send_command(Command::SetRoot { root: ctx.dest_root.clone() })?;
            dest_root_details = match ctx.dest_comms.receive_response()? {
                Response::RootDetails { root_details, platform_differentiates_symlinks: _, platform_dir_separator: _ } => root_details,
                r => return Err(format!("Unexpected response getting root details from dest: {:?}", r)),
            }
        }
    }

    Ok((src_root_details, dest_root_details, dest_platform_differentiates_symlinks))
}

fn check_dest_root_delete_ok(ctx: &mut SyncContext, src_root_details: &EntryDetails, dest_root_details: &EntryDetails)
    -> Result<bool, String> {
    let msg = format!(
        "{} needs deleting as it is incompatible with {}",
        ctx.pretty_dest(&RootRelativePath::root(), dest_root_details),
        ctx.pretty_src(&RootRelativePath::root(), &src_root_details));
    let resolved_behaviour = match ctx.dest_root_needs_deleting_behaviour {
        DestRootNeedsDeletingBehaviour::Prompt => {
            let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                None,
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
        DestRootNeedsDeletingBehaviour::Skip => return Ok(false), // Don't raise an error, but we can't continue as it will fail, so skip the entire sync
        DestRootNeedsDeletingBehaviour::Delete => return Ok(true), // We will delete it anyway later on
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteReason {
    NotOnSource,
    Incompatible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyReason {
    NotOnDest,
    DestNewer,
    DestOlder,
}

pub struct Actions {
    pub to_delete: OrderedMap<RootRelativePath, (EntryDetails, DeleteReason)>,
    pub to_copy: OrderedMap<RootRelativePath, (EntryDetails, CopyReason)>,
}

fn query_entries(ctx: &mut SyncContext, src_root_details: EntryDetails, dest_root_details: Option<EntryDetails>,
    dest_platform_differentiates_symlinks: bool)
 ->
    Result<Actions, String>
{
    profile_this!();

    // As we receive entry details from the source and dest, we will build up lists of which
    // entries need copying and which need deleting. We will be both adding and removing entries
    // from these lists as we need details from both source and dest to make the right decision.
    let mut to_delete = OrderedMap::<RootRelativePath, (EntryDetails, DeleteReason)>::new(); //TODO: is the order of stuff in these lists ok? if we're adding/removing stuff?
    let mut to_copy = OrderedMap::<RootRelativePath, (EntryDetails, CopyReason)>::new();

    let mut src_entries = OrderedMap::<RootRelativePath, EntryDetails>::new();
    let mut src_done = true;

    let mut dest_entries = OrderedMap::<RootRelativePath, EntryDetails>::new();
    let mut dest_done = true;

    process_src_entry(ctx, RootRelativePath::root(), src_root_details.clone(),
        &mut src_entries, &dest_entries, dest_platform_differentiates_symlinks, &mut to_delete, &mut to_copy);

    if matches!(src_root_details, EntryDetails::Folder) {
        ctx.src_comms.send_command(Command::GetEntries { filters: ctx.filters.clone() })?;
        src_done = false;
    }

    if let Some(d) = &dest_root_details {
        process_dest_entry(ctx, RootRelativePath::root(), d.clone(), &src_entries, &mut dest_entries, dest_platform_differentiates_symlinks, &mut to_delete, &mut to_copy)
    }

    if matches!(dest_root_details, Some(EntryDetails::Folder)) {
        ctx.dest_comms.send_command(Command::GetEntries { filters: ctx.filters.clone() })?;
        dest_done = false;
    }

    while !src_done || !dest_done {
        // Wait for either src or dest to send us a response with an entry
        match memory_bound_channel::select_ready(ctx.src_comms.get_receiver(), ctx.dest_comms.get_receiver()) {
            0 => match ctx.src_comms.receive_response()? {
                Response::Entry((p, src_entry)) => process_src_entry(ctx, p, src_entry,
                    &mut src_entries, &dest_entries, dest_platform_differentiates_symlinks,
                    &mut to_delete, &mut to_copy),
                Response::EndOfEntries => src_done = true,
                r => return Err(format!("Unexpected response getting entries from src: {:?}", r)),
            },
            1 => match ctx.dest_comms.receive_response()? {
                Response::Entry((p, dest_entry)) => process_dest_entry(ctx, p, dest_entry,
                    &src_entries, &mut dest_entries, dest_platform_differentiates_symlinks,
                    &mut to_delete, &mut to_copy),
                Response::EndOfEntries => dest_done = true,
                r => return Err(format!("Unexpected response getting entries from dest: {:?}", r)),
            },
            _ => panic!("Invalid index"),
        }
    }

    ctx.stats.num_src_entries = src_entries.len() as u32;
    ctx.stats.num_dest_entries = dest_entries.len() as u32;

    // Reverse the order of to_delete
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly would also have problems with
    // files being filtered so the folder is needed still as there are filtered-out files in there,
    // see test_remove_dest_folder_with_excluded_files())
    to_delete.reverse_order();

    Ok(Actions { to_delete, to_copy })
}

fn process_src_entry(ctx: &mut SyncContext, p: RootRelativePath, src_entry: EntryDetails,
    src_entries: &mut OrderedMap::<RootRelativePath, EntryDetails>,
    dest_entries: &OrderedMap::<RootRelativePath, EntryDetails>,
    dest_platform_differentiates_symlinks: bool,
    to_delete: &mut OrderedMap::<RootRelativePath, (EntryDetails, DeleteReason)>,
    to_copy: &mut OrderedMap::<RootRelativePath, (EntryDetails, CopyReason)>,
) {
    trace!("Source entry '{}': {:?}", p, src_entry);
    match src_entry {
        EntryDetails::File { size, .. } => {
            ctx.stats.num_src_files += 1;
            ctx.stats.src_total_bytes += size;
            ctx.stats.src_file_size_hist.add(size);
        }
        EntryDetails::Folder => ctx.stats.num_src_folders += 1,
        EntryDetails::Symlink { .. } => ctx.stats.num_src_symlinks += 1,
    }
    match dest_entries.lookup(&p) {
        None => to_copy.add(p.clone(), (src_entry.clone(), CopyReason::NotOnDest)),
        Some(dest_entry) => {
            // This entry will already be in to_delete, but we might need to remove it now
            if !needs_delete(ctx, &p, &src_entry, dest_entry, dest_platform_differentiates_symlinks) {
                to_delete.remove(&p);
                if let Some(r) = needs_copy(ctx, &p, &src_entry, dest_entry) {
                    to_copy.add(p.clone(), (src_entry.clone(), r));
                }
            } else {
                // even though the entry is already in to_delete, the *reason* will need updating
                to_delete.update(&p, (dest_entry.clone(), DeleteReason::Incompatible));

                // Dest is going to be deleted, so we will definitely be copying the source
                to_copy.add(p.clone(), (src_entry.clone(), CopyReason::NotOnDest));
            }
        }
    }

    src_entries.add(p, src_entry);
}

fn process_dest_entry(ctx: &mut SyncContext, p: RootRelativePath, dest_entry: EntryDetails,
    src_entries: &OrderedMap::<RootRelativePath, EntryDetails>,
    dest_entries: &mut OrderedMap::<RootRelativePath, EntryDetails>,
    dest_platform_differentiates_symlinks: bool,
    to_delete: &mut OrderedMap::<RootRelativePath, (EntryDetails, DeleteReason)>,
    to_copy: &mut OrderedMap::<RootRelativePath, (EntryDetails, CopyReason)>,
) {
    trace!("Dest entry '{}': {:?}", p, dest_entry);
    match dest_entry {
        EntryDetails::File { size, .. } => {
            ctx.stats.num_dest_files += 1;
            ctx.stats.dest_total_bytes += size;
        }
        EntryDetails::Folder => ctx.stats.num_dest_folders += 1,
        EntryDetails::Symlink { .. } => ctx.stats.num_dest_symlinks += 1,
    }
    dest_entries.add(p.clone(), dest_entry.clone());

    match src_entries.lookup(&p) {
        None => to_delete.add(p, (dest_entry, DeleteReason::NotOnSource)),
        Some(src_entry) => {
            if needs_delete(ctx, &p, src_entry, &dest_entry, dest_platform_differentiates_symlinks) {
                to_delete.add(p, (dest_entry, DeleteReason::Incompatible));
            } else
            {
                if let Some(r) = needs_copy(ctx, &p, src_entry, &dest_entry) {
                    //  even though the entry is already in to_copy, the *reason* will need updating
                    to_copy.update(&p, (src_entry.clone(), r));
                }
                else {
                    // This entry will already be in to_copy, but we might need to remove it now
                    to_copy.remove(&p);
                }
            }
        }
    }
}

/// Checks if a given existing dest entry could be updated to match the src, or if it needs
/// to be deleted and recreated instead.
fn needs_delete(_ctx: &SyncContext, _path: &RootRelativePath, src: &EntryDetails, dest: &EntryDetails, dest_platform_differentiates_symlinks: bool)
    -> bool
{
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

/// Checks if a given source entry needs to be copied over the top of the given dest entry.
fn needs_copy(ctx: &SyncContext, path: &RootRelativePath, src_details: &EntryDetails, dest_details: &EntryDetails)
    -> Option<CopyReason>
{
    // Dest already has this entry - check if it is up-to-date
    match src_details {
        EntryDetails::File { modified_time: src_modified_time, .. } => {
            let dest_modified_time = match dest_details {
                EntryDetails::File { modified_time, .. } => modified_time,
                _ => panic!("Wrong entry type"), // This should never happen as we check the type in the .find() above
            };
            match src_modified_time.cmp(&dest_modified_time) {
                Ordering::Equal => {
                    trace!("{} has same modified time as {}. Will not update.",
                        ctx.pretty_dest_kind(&path, "file"),
                        ctx.pretty_src_kind(&path, "file"));
                    None
                },
                Ordering::Greater => Some(CopyReason::DestOlder),
                Ordering::Less => Some(CopyReason::DestNewer),
            }
        },
        EntryDetails::Folder |  // Folders are always up-to-date
        EntryDetails::Symlink { .. }  // Symlinks are always up-to-date, if should_delete indicated that we shouldn't delete it
        => {
            trace!("{} already exists at {} - nothing to do",
                ctx.pretty_src(&path, &src_details),
                ctx.pretty_dest(&path, dest_details));
            None
        },
    }
}

fn confirm_actions(ctx: &mut SyncContext, actions: &mut Actions) -> Result<(), String> {
    let mut to_remove = vec![];
    for (path, (entry_to_delete, reason)) in actions.to_delete.iter() {
        let msg = format!(
            "{} needs deleting {}",
            ctx.pretty_dest(path, entry_to_delete),
            match reason {
                DeleteReason::NotOnSource => "as it doesn't exist on the src",
                DeleteReason::Incompatible => "to allow the source entry to be copied",
            });

        // Resolve any behaviour resulting from a prompt first
        let resolved_behaviour = match ctx.dest_entry_needs_deleting_behaviour {
            DestEntryNeedsDeletingBehaviour::Prompt => {
                let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                    None,
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
                to_remove.push(path.clone());
            }
            DestEntryNeedsDeletingBehaviour::Delete => {
                // Carry on
            }
        }
    }
    for p in to_remove {
        actions.to_delete.remove(&p);
    }

    let mut to_remove = vec![];
    for (path, (_entry_to_copy, reason)) in actions.to_copy.iter() {
        match reason {
            CopyReason::NotOnDest => {
                // Nothing to check
            }
            CopyReason::DestNewer => {
                let msg = format!(
                    "{} is newer than {}",
                    ctx.pretty_dest_kind(&path, "file"),
                    ctx.pretty_src_kind(&path, "file"));
                // Resolve any behaviour resulting from a prompt first
                let resolved_behaviour = match ctx.dest_file_newer_behaviour {
                    DestFileUpdateBehaviour::Prompt => {
                        let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                            None,
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
                        to_remove.push(path.clone());
                    }
                    DestFileUpdateBehaviour::Overwrite => {
                        trace!("{msg}. Overwriting anyway.");
                    }
                }
            },
            CopyReason::DestOlder => {
                let msg = format!(
                    "{} is older than {}",
                    ctx.pretty_dest_kind(&path, "file"),
                    ctx.pretty_src_kind(&path, "file"));
                // Resolve any behaviour resulting from a prompt first
                let resolved_behaviour = match ctx.dest_file_older_behaviour {
                    DestFileUpdateBehaviour::Prompt => {
                        let prompt_result = resolve_prompt(format!("{msg}. What do?"),
                            None,
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
                        to_remove.push(path.clone());
                    }
                    DestFileUpdateBehaviour::Overwrite => {
                        trace!("{msg}. Overwriting.");
                    }
                }
            }
        }
    }
    for p in to_remove {
        actions.to_copy.remove(&p);
    }

    Ok(())
}

fn show_post_query_stats(ctx: &SyncContext, query_elapsed_secs: f32) {
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
        info!("Queried in {:.2} seconds", query_elapsed_secs);
    }
}

fn delete_dest_entry(ctx: &mut SyncContext, dest_details: &EntryDetails,
    dest_path: &RootRelativePath) -> Result<(), String>
{
    trace!("{dest_path}. Deleting.");
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
    ctx.send_progress_marker_limited()?;

    let result = Ok(if !ctx.dry_run {
        ctx.dest_comms.send_command(c)?;
    } else {
        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be deleted
        info!("Would delete {}", ctx.pretty_dest(dest_path, dest_details));
    });

    ctx.progress.as_mut().unwrap().delete_sent(&dest_details);

    result
}

fn copy_entry(ctx: &mut SyncContext, path: &RootRelativePath, src_details: &EntryDetails) -> Result<(), String> {
    match src_details {
        EntryDetails::File { size, modified_time: src_modified_time } => {
            debug!("{} - copying", ctx.pretty_src(&path, &src_details));
            copy_file(&path, *size, *src_modified_time, ctx)?
        }
        EntryDetails::Folder => {
            debug!("{} doesn't exist on dest - creating", ctx.pretty_src(&path, &src_details));
            ctx.send_progress_marker_limited()?;
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
            ctx.progress.as_mut().unwrap().copy_sent(&src_details);
        },
        EntryDetails::Symlink { ref kind, ref target } => {
            debug!("{} doesn't exist on dest - copying", ctx.pretty_src(&path, &src_details));
            ctx.send_progress_marker_limited()?;
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
            ctx.progress.as_mut().unwrap().copy_sent(&src_details);
        }
    }
    Ok(())
}

fn copy_file(
    path: &RootRelativePath,
    size: u64,
    modified_time: SystemTime,
    ctx: &mut SyncContext) -> Result<(), String>
{
    ctx.send_progress_marker_limited()?;

    if !ctx.dry_run {
        trace!("Fetching from {}", ctx.pretty_src_kind(&path, "file"));
        ctx.src_comms
            .send_command(Command::GetFileContent {
                path: path.clone(),
            })?;
        // Large files are split into chunks, loop until all chunks are transferred.
        let mut chunk_offset: u64 = 0;
        loop {
            // Add progress markers during copies of large files, so we can see the progress (in bytes)
            ctx.send_progress_marker_limited()?;

            let (data, more_to_follow) = match ctx.src_comms.receive_response()? {
                Response::FileContent { data, more_to_follow } => (data, more_to_follow),
                x => return Err(format!(
                    "Unexpected response fetching {}: {:?}", ctx.pretty_src_kind(&path, "file"), x
                )),
            };
            trace!("Create/update {}", ctx.pretty_dest_kind(&path, "file"));
            let chunk_size = data.len();
            ctx.dest_comms
                .send_command(Command::CreateOrUpdateFile {
                    path: path.clone(),
                    data,
                    set_modified_time: if more_to_follow { None } else { Some(modified_time) }, // Only set the modified time after the final chunk
                    more_to_follow,
                })?;

            // This needs to be inside the chunking loop so we can update progress as the file is copied
            ctx.progress.as_mut().unwrap().copy_sent_partial(chunk_offset, chunk_size as u64, size);
            chunk_offset += chunk_size as u64;

            // For large files, it might be a while before process_dest_responses is called in the main sync function,
            // so check it periodically here too.
            process_dest_responses(ctx.dest_comms, &mut ctx.progress.as_mut().unwrap(), false)?;

            if !more_to_follow {
                break;
            }
        }
    } else {
        ctx.progress.as_mut().unwrap().copy_sent_partial(0, size, size);
        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
        info!("Would copy {} => {}",
            ctx.pretty_src_kind(&path, "file"),
            ctx.pretty_dest_kind(&path, "file"));
    }

    ctx.stats.num_files_copied += 1;
    ctx.stats.num_bytes_copied += size;
    ctx.stats.copied_file_size_hist.add(size);

    Ok(())
}

fn show_post_sync_stats(ctx: &SyncContext) {
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
}