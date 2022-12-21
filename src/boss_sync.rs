use std::{
    cmp::Ordering,
    fmt::{Display, Write}, time::{Instant, SystemTime, Duration}, collections::HashMap,
};

use indicatif::{ProgressBar, ProgressStyle, HumanCount, HumanBytes};
use log::{debug, info, trace};

use crate::*;

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

/// Formats a path which is relative to the root, so that it is easier to understand for the user.
/// Especially if path is empty (i.e. referring to the root itself)
fn format_root_relative(path: &RootRelativePath, root: &str) -> String {
    //TODO: include something that says whether this is the source or dest, rather than relying on outside code to do it?
    //TODO: limit length of string using ellipses e.g. "Copying T:\work\...\bob\folder\...\thing.txt to X:\backups\...\newbackup\folder\...\thing.txt"
    //TODO: the formatting here isn't quite right yet. e.g. the root might already end with a trailing slash, but we add another
    //TODO: could use bold/italic/colors etc. to highlight the root, rather than brackets?
    if path.is_root() {
        format!("'({root})'")
    } else {
        format!("'({root}/){path}'")
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
/// If block_until_marker_value is set, then this function will keep processing messages (blocking as necessary)
/// until it finds a response with the given marker. If not set, this function won't block and will return
/// once it's processed all pending responses from the doer.
fn process_dest_responses(dest_comms: &mut Comms, progress: &ProgressBar, stats: &mut Stats, 
    block_until_marker_value: Option<u64>) -> Result<(), Vec<String>> 
{
    // To make the rest of this function consistent for both cases of block_until_marker_value,
    // this helper function will block or not as appropriate.
    let next_fn = || {
        match block_until_marker_value {
            Some(m) => match dest_comms.receive_response() { // Blocking, as we need to wait until we find the requested marker value
                Response::Marker(m2) if m == m2 => None, // Marker found, stop iterating and return
                x => Some(x), // Something else - needs processing
            },
            None => dest_comms.try_receive_response() // Non-blocking
        }
    };

    let mut errors = vec![]; // There might be multiple errors reported before we get round to checking for them
    while let Some(x) = next_fn() {
        match x {
            Response::Error(e) => errors.push(e),
            Response::Marker(m) => {
                //TODO: This is a bit yucky
                // Update the progress bar
                if m < stats.num_dest_entries as u64 {
                    progress.set_position(m);
                } else {
                    if progress.message().contains("Deleting") {
                        // Replace the first progress bar with the second one
                        progress.set_message("Copying...");
                        progress.set_length(stats.num_src_entries as u64);
                        stats.delete_end_time = Some(Instant::now());
                        stats.copy_start_time = Some(Instant::now());
                    }
                    progress.set_position(m - stats.num_dest_entries as u64);
                }
            }
            _ => errors.push(format!("Unexpected response (expected Error or Marker): {:?}", x)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
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
    progress_bar: ProgressBar,
    progress_count: u64,
}

//TODO: maybe this should take a SyncSpec to provide most of these params?
pub fn sync(
    src_root: &str,
    dest_root: &str,
    filters: Filters,
    dry_run: bool,
    dest_file_newer_behaviour: DestFileUpdateBehaviour,
    dest_file_older_behaviour: DestFileUpdateBehaviour,
    dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour,
    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour,
    show_stats: bool,
    src_comms: &mut Comms,
    dest_comms: &mut Comms,
) -> Result<(), Vec<String>> {
    // Make context object, to avoid having to pass around a bunch of individual variables everywhere
    let context = SyncContext {
        src_comms,
        dest_comms,
        filters,
        stats: Stats::default(),
        dry_run,
        show_stats,
        dest_file_newer_behaviour,
        dest_file_older_behaviour,
        dest_entry_needs_deleting_behaviour,
        dest_root_needs_deleting_behaviour,
        src_root: String::from(src_root),
        dest_root: String::from(dest_root),
        progress_bar: ProgressBar::new_spinner().with_message("Querying..."),
        progress_count: 0,    
    };
    // Call into separate function, to avoid the original function parameters being mis-used instead
    // of the context fields
    sync_impl(context)
}

fn sync_impl(mut ctx: SyncContext) -> Result<(), Vec<String>> {
    profile_this!();

    let sync_start = Instant::now();

    // First get details of the root file/folder etc. of each side, as this might affect the sync
    // before we start it (e.g. errors, or changing the dest root)

    ctx.progress_bar.enable_steady_tick(Duration::from_millis(250)); // So that the progress bar gets redrawn after any log messages, even if no progress has been made in the meantime

    // Source SetRoot
    let timer = start_timer("SetRoot src");
    ctx.src_comms.send_command(Command::SetRoot { root: ctx.src_root.to_string() });
    let src_root_details = match ctx.src_comms.receive_response() {
        Response::RootDetails { root_details, platform_differentiates_symlinks: _ } => {
            match &root_details {
                None => return Err(vec![format!("src path '{}' doesn't exist!", ctx.src_root)]),
                Some(d) => if let Err(e) = validate_trailing_slash(&ctx.src_root, &d) {
                    return Err(vec![format!("src path {}", e)]);
                }
            };
            root_details
        }
        r => return Err(vec![format!("Unexpected response getting root details from src: {:?}", r)]),
    };
    let src_root_details = src_root_details.unwrap();
    stop_timer(timer);

    // Dest SetRoot
    let timer = start_timer("SetRoot dest");
    ctx.dest_comms.send_command(Command::SetRoot { root: ctx.dest_root.clone() });
    let (mut dest_root_details, dest_platform_differentiates_symlinks) = match ctx.dest_comms.receive_response() {
        Response::RootDetails { root_details, platform_differentiates_symlinks } => {
            match &root_details {
                None => (), // Dest root doesn't exist, but that's fine (we will create it later)
                Some(d) => if let Err(e) = validate_trailing_slash(&ctx.dest_root, &d) {
                    return Err(vec![format!("dest path {}", e)]);
                }
            }
            (root_details, platform_differentiates_symlinks)
        }
        r => return Err(vec![format!("Unexpected response getting root details from dest: {:?}", r)]),
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

            ctx.dest_comms.send_command(Command::SetRoot { root: ctx.dest_root.clone() });
            dest_root_details = match ctx.dest_comms.receive_response() {
                Response::RootDetails { root_details, platform_differentiates_symlinks: _ } => root_details,
                r => return Err(vec![format!("Unexpected response getting root details from dest: {:?}", r)]),
            }
        }
    }

    // Check if the dest root will need deleting, and potentially prompt the user.
    // We need to do this explicitly before starting to delete anything 
    // because we delete in reverse order, and we would end up deleting everything inside the 
    // dest root folder before getting to the root itself, so the prompt/error would be too late!
    if let Some(d) = &dest_root_details {
        if should_delete(&src_root_details, d, dest_platform_differentiates_symlinks) {
            let resolved_behaviour = match ctx.dest_root_needs_deleting_behaviour {
                DestRootNeedsDeletingBehaviour::Prompt => {
                    let prompt_result = resolve_prompt(format!(
                        "Dest root {} needs deleting as it is incompatible with the source. What do?",
                        format_root_relative(&RootRelativePath::root(), &ctx.dest_root)),
                        &ctx.progress_bar, 
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
                DestRootNeedsDeletingBehaviour::Error => return Err(vec![format!("Dest root {} needs deleting as it is incompatible with the source. Will not delete. See --dest-root-needs-deleting",
                    format_root_relative(&RootRelativePath::root(), &ctx.dest_root))]),
                DestRootNeedsDeletingBehaviour::Skip => return Ok(()), // Don't raise an error, but we can't continue as it will fail, so skip the entire sync
                DestRootNeedsDeletingBehaviour::Delete => (), // We will delete it anyway later on
            }
        }
    }


    // If the dest doesn't yet exist, make sure that all its ancestors are created, so that
    // when we come to create the dest path itself, it can succeed
    if dest_root_details.is_none() {
        ctx.dest_comms.send_command(Command::CreateRootAncestors);
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

    if matches!(src_root_details, EntryDetails::Folder) {
        ctx.src_comms.send_command(Command::GetEntries { filters: ctx.filters.clone() });
        src_done = false;
    }

    // Dest GetEntries
    let mut dest_entries = Vec::new();
    let mut dest_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();
    let mut dest_done = true;

    // Add the root entry - we already got the details for this before
    if dest_root_details.is_some() {
        dest_entries.push((RootRelativePath::root(), dest_root_details.clone().unwrap()));
        dest_entries_lookup.insert(RootRelativePath::root(), dest_root_details.clone().unwrap());
    }

    if matches!(dest_root_details, Some(EntryDetails::Folder)) {
        ctx.dest_comms.send_command(Command::GetEntries { filters: ctx.filters.clone() });
        dest_done = false;
    }

    while !src_done || !dest_done {
        if !src_done {
            //TODO: receive_response will block, perhaps should check it instead (try_receive_response), 
            // so we can service the other src/dest
            //TODO: if we use crossbeam, then we can select() on both channels rather than busy-waiting
            match ctx.src_comms.receive_response() {
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
                    src_entries.push((p.clone(), d.clone()));
                    src_entries_lookup.insert(p, d);
                }
                Response::EndOfEntries => src_done = true,
                r => return Err(vec![format!("Unexpected response getting entries from src: {:?}", r)]),
            }    
        }
        if !dest_done {
            match ctx.dest_comms.receive_response() {
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
                    dest_entries.push((p.clone(), d.clone()));
                    dest_entries_lookup.insert(p, d);
                }
                Response::EndOfEntries => dest_done = true,
                r => return Err(vec![format!("Unexpected response getting entries from dest: {:?}", r)]),
            }    
        }
    }
    ctx.stats.num_src_entries = src_entries.len() as u32;
    ctx.stats.num_dest_entries = dest_entries.len() as u32;

    stop_timer(timer);

    let query_elapsed = sync_start.elapsed().as_secs_f32();
    ctx.progress_bar.finish_and_clear();

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
    ctx.progress_bar = ProgressBar::new(dest_entries.len() as u64).with_message("Deleting...")
        .with_style(ProgressStyle::with_template("[{elapsed}] {bar:40.green/black} {human_pos:>7}/{human_len:7} {msg}").unwrap());
        ctx.progress_bar.enable_steady_tick(Duration::from_millis(250));

    {
        profile_this!("Sending delete commands");
        ctx.stats.delete_start_time = Some(Instant::now());
        for (dest_path, dest_details) in dest_entries.iter().rev() {
            let s = src_entries_lookup.get(dest_path);
            if !s.is_some() || should_delete(s.unwrap(), dest_details, dest_platform_differentiates_symlinks) {
                delete_dest_entry(&mut ctx, dest_details, dest_path)?;
            }
            ctx.progress_count += 1;
        }
        ctx.dest_comms.send_command(Command::Marker(ctx.progress_count)); // Mark the exact end of deletion, rather then having to wait for the first file to be copied
    }

    // Copy entries that don't exist, or do exist but are out-of-date.
    {
        profile_this!("Sending copy commands");
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
                            handle_existing_file(&path, size, &mut ctx, src_modified_time,
                                *dest_modified_time)?;
                        },
                        EntryDetails::Folder => {
                            // Folders are always up-to-date
                            trace!("Source folder {} already exists on dest {} - nothing to do",
                                format_root_relative(&path, &ctx.src_root),
                                format_root_relative(&path, &ctx.dest_root))
                        },
                        EntryDetails::Symlink { .. } => {
                            // Symlinks are always up-to-date, if should_delete indicated that we shouldn't delete it
                            trace!("Source symlink {} already exists on dest {} - nothing to do",
                                format_root_relative(&path, &ctx.src_root),
                                format_root_relative(&path, &ctx.dest_root))
                        },
                    }
                },
                _ => match src_details {
                    EntryDetails::File { size, modified_time: src_modified_time } => {
                        debug!("Source file {} file doesn't exist on dest - copying", format_root_relative(&path, &ctx.src_root));
                        ctx.dest_comms.send_command(Command::Marker(ctx.progress_count));
                        copy_file(&path, size, src_modified_time, &mut ctx)?
                    }
                    EntryDetails::Folder => {
                        debug!("Source folder {} doesn't exist on dest - creating", format_root_relative(&path, &ctx.src_root));
                        ctx.stats.num_folders_created += 1;
                        if !ctx.dry_run {
                            ctx.dest_comms
                                .send_command(Command::CreateFolder {
                                    path: path.clone(),
                                });
                        } else {
                            // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                            info!("Would create dest folder {}", format_root_relative(&path, &ctx.dest_root));
                        }
                    },
                    EntryDetails::Symlink { kind, target } => {
                        debug!("Source {} symlink doesn't exist on dest - copying", format_root_relative(&path, &ctx.src_root));
                        ctx.stats.num_symlinks_copied += 1;
                        if !ctx.dry_run {
                            ctx.dest_comms
                                .send_command(Command::CreateSymlink {
                                    path: path.clone(),
                                    kind,
                                    target,
                                });
                        } else {
                            // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                            info!("Would create dest symlink {}", format_root_relative(&path, &ctx.dest_root));
                        }
                    }
                },
            }
            ctx.progress_count += 1;

            process_dest_responses(ctx.dest_comms, &ctx.progress_bar, &mut ctx.stats, None)?;
        }
    }

    // Wait for the dest doer to finish processing all its Commands so that everything is finished.
    // We don't need to wait for the src doer, because the dest doer is always last to finish.
    ctx.dest_comms.send_command(Command::Marker(u64::MAX));
    {
        profile_this!("Waiting for dest to finish");
        process_dest_responses(ctx.dest_comms, &ctx.progress_bar, &mut ctx.stats, Some(u64::MAX))?;
    }
    ctx.stats.copy_end_time = Some(Instant::now());
    ctx.progress_bar.finish_and_clear();

    // Note that we print all the stats at the end (even though we could print the delete stats earlier),
    // so that they are together in the output (e.g. for dry run or --verbose, they could be a lot of other
    // messages between them)
    if (ctx.stats.num_files_deleted + ctx.stats.num_folders_deleted + ctx.stats.num_symlinks_deleted > 0) || ctx.show_stats {
        let delete_elapsed = ctx.stats.delete_end_time.unwrap() - ctx.stats.delete_start_time.unwrap();
        info!(
            "{} {} file(s){}, {} folder(s) and {} symlink(s){}",
            if !ctx.dry_run { "Deleted" } else { "Would delete" },
            HumanCount(ctx.stats.num_files_deleted as u64),
            if ctx.show_stats { format!(" totalling {}", HumanBytes(ctx.stats.num_bytes_deleted)) } else { "".to_string() },
            HumanCount(ctx.stats.num_folders_deleted as u64),
            HumanCount(ctx.stats.num_symlinks_deleted as u64),
            if !ctx.dry_run && ctx.show_stats {
                format!(", in {:.1} seconds", delete_elapsed.as_secs_f32())
            } else { "".to_string() },
        );
    }
    if (ctx.stats.num_files_copied + ctx.stats.num_folders_created + ctx.stats.num_symlinks_copied > 0) || ctx.show_stats {
        let copy_elapsed = ctx.stats.copy_end_time.unwrap() - ctx.stats.copy_start_time.unwrap();
        info!(
            "{} {} file(s){}, {} {} folder(s) and {} {} symlink(s){}",
            if !ctx.dry_run { "Copied" } else { "Would copy" },
            HumanCount(ctx.stats.num_files_copied as u64),
            if ctx.show_stats { format!(" totalling {}", HumanBytes(ctx.stats.num_bytes_copied)) } else { "".to_string() },
            if !ctx.dry_run { "created" } else { "would create" },
            HumanCount(ctx.stats.num_folders_created as u64),
            if !ctx.dry_run { "copied" } else { "would copy" },
            HumanCount(ctx.stats.num_symlinks_copied as u64),
            if !ctx.dry_run && ctx.show_stats {
                format!(", in {:.1} seconds ({}/s)",
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

fn delete_dest_entry(ctx: &mut SyncContext, dest_details: &EntryDetails, dest_path: &RootRelativePath) -> Result<(), Vec<String>> {
    if ctx.progress_count % 100 == 0 {
        // Deletes are quite quick, so reduce the overhead of marking progress by only marking it occasionally
        ctx.dest_comms.send_command(Command::Marker(ctx.progress_count));
    }

    // Resolve any behaviour resulting from a prompt first
    let resolved_behaviour = match ctx.dest_entry_needs_deleting_behaviour {
        DestEntryNeedsDeletingBehaviour::Prompt => {
            let prompt_result = resolve_prompt(format!(
                "Dest entry {} needs deleting as it doesn't exist on the src. What do?",
                format_root_relative(dest_path, &ctx.dest_root)),
                &ctx.progress_bar, 
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
        DestEntryNeedsDeletingBehaviour::Error => return Err(vec![format!(
            "Dest entry {} needs deleting as it doesn't exist on the src (or is incompatible). Will not delete. See --dest-entry-needs-deleting.",
            format_root_relative(&dest_path, &ctx.dest_root),
        )]),
        DestEntryNeedsDeletingBehaviour::Skip => {
            trace!("Dest entry {} needs deleting as it doesn't exist on the src (or is incompatible). Skipping.",
                format_root_relative(&dest_path, &ctx.dest_root));
            Ok(())
        }
        DestEntryNeedsDeletingBehaviour::Delete => {
            trace!("Dest entry {} needs deleting as it doesn't exist on the src (or is incompatible). Deleting.",
            format_root_relative(&dest_path, &ctx.dest_root));
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
            Ok(if !ctx.dry_run {
                ctx.dest_comms.send_command(c);
                process_dest_responses(ctx.dest_comms, &ctx.progress_bar, &mut ctx.stats, None)?;
            } else {
                // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be deleted
                info!("Would delete from dest {}", format_root_relative(&dest_path, &ctx.dest_root));
            })     
        }
    }
}

fn handle_existing_file(
    path: &RootRelativePath,
    size: u64,
    ctx: &mut SyncContext,
    src_modified_time: SystemTime, dest_modified_time: SystemTime,
) -> Result<(), Vec<String>> {
    let copy = match src_modified_time.cmp(&dest_modified_time) {
        Ordering::Less => {
            // Resolve any behaviour resulting from a prompt first
            let resolved_behaviour = match ctx.dest_file_newer_behaviour {
                DestFileUpdateBehaviour::Prompt => {
                    let prompt_result = resolve_prompt(format!(
                        "Dest file {} is newer than src file {}. What do?",
                        format_root_relative(&path, &ctx.dest_root),
                        format_root_relative(&path, &ctx.src_root)),
                        &ctx.progress_bar, 
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
                DestFileUpdateBehaviour::Error => return Err(vec![format!(
                    "Dest file {} is newer than src file {}. Will not overwrite. See --dest-file-newer.",
                    format_root_relative(&path, &ctx.dest_root),
                    format_root_relative(&path, &ctx.src_root)
                )]),
                DestFileUpdateBehaviour::Skip => {
                    trace!("Dest file {} is newer than src file {}. Skipping.",
                    format_root_relative(&path, &ctx.dest_root),
                    format_root_relative(&path, &ctx.src_root));
                    false
                }
                DestFileUpdateBehaviour::Overwrite => {
                    trace!("Dest file {} is newer than src file {}. Overwriting anyway.",
                        format_root_relative(&path, &ctx.dest_root),
                        format_root_relative(&path, &ctx.src_root));
                    true
                }
            }
        }
        Ordering::Equal => {
            trace!("Dest file {} has same modified time as src file {}. Will not update.",
                format_root_relative(&path, &ctx.dest_root),
                format_root_relative(&path, &ctx.src_root));
            false
        }
        Ordering::Greater => {
            // Resolve any behaviour resulting from a prompt first
            let resolved_behaviour = match ctx.dest_file_older_behaviour {
                DestFileUpdateBehaviour::Prompt => {
                    let prompt_result = resolve_prompt(format!(
                        "Dest file {} is older than src file {}. What do?",
                        format_root_relative(&path, &ctx.dest_root),
                        format_root_relative(&path, &ctx.src_root)),
                        &ctx.progress_bar, 
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
                DestFileUpdateBehaviour::Error => return Err(vec![format!(
                    "Source file {} is newer than dest file {}. Will not overwrite. See --dest-file-older.",
                    format_root_relative(&path, &ctx.src_root),
                    format_root_relative(&path, &ctx.dest_root)
                )]),
                DestFileUpdateBehaviour::Skip => {
                    trace!("Source file {} is newer than dest file {}. Skipping.",
                        format_root_relative(&path, &ctx.src_root),
                        format_root_relative(&path, &ctx.dest_root));
                    false
                }
                DestFileUpdateBehaviour::Overwrite => {
                    trace!("Source file {} is newer than dest file {}. Overwriting.",
                        format_root_relative(&path, &ctx.src_root),
                        format_root_relative(&path, &ctx.dest_root));
                    true
                }
            }
        }
    };
    if copy {                    
        ctx.dest_comms.send_command(Command::Marker(ctx.progress_count));
        copy_file(&path, size, src_modified_time, ctx)?
    }
    Ok(())
}

/// For testing purposes, this env var can be set to a list of responses to prompts
/// that we might display, which we use immediately rather than waiting for a real user
/// to respond.
const TEST_PROMPT_RESPONSE_ENV_VAR: &str = "RJRSSYNC_TEST_PROMPT_RESPONSE";

#[derive(Clone, Copy)]
struct ResolvePromptResult<B> {
    /// The decision that was made for this occurence.
    immediate_behaviour: B,
    /// The decision that was made to be remembered for future occurences (if any)
    remembered_behaviour: Option<B>,
}
impl<B: Copy> ResolvePromptResult<B> {
    fn once(b: B) -> Self {
        Self { immediate_behaviour: b, remembered_behaviour: None }
    }
    fn always(b: B) -> Self {
        Self { immediate_behaviour: b, remembered_behaviour: Some(b) }
    }
}

fn resolve_prompt<B: Copy>(prompt: String, progress_bar: &ProgressBar,
    options: &[(&str, B)], include_always_versions: bool, cancel_behaviour: B) -> ResolvePromptResult<B> {

    let mut items = vec![];
    for o in options {
        if include_always_versions {
            items.push((format!("{} (just this occurence)", o.0), ResolvePromptResult::once(o.1)));
            items.push((format!("{} (all occurences)", o.0), ResolvePromptResult::always(o.1)));
        } else {
            items.push((String::from(o.0), ResolvePromptResult::once(o.1)));
        }
    }
    items.push((String::from("Cancel sync"), ResolvePromptResult::once(cancel_behaviour)));

    // Allow overriding the prompt response for testing
    let mut response_idx = None;
    if let Ok(all_responses) = std::env::var(TEST_PROMPT_RESPONSE_ENV_VAR) {
        let mut iter = all_responses.splitn(2, ',');
        let next_response = iter.next().unwrap(); // It always has at least one entry, even if the input string is empty
        if !next_response.is_empty() {
            // Print the prompt anyway, so the test can confirm that it was hit
            println!("{}", prompt);
            response_idx = Some(items.iter().position(|i| i.0 == next_response).expect("Invalid response"));
            std::env::set_var(TEST_PROMPT_RESPONSE_ENV_VAR, iter.next().unwrap_or(""));
        }
    }
    let response_idx = match response_idx {
        Some(r) => r,
        None => {
            if !dialoguer::console::user_attended() {
                debug!("Unattended terminal, behaving as if prompt cancelled");
                items.len() - 1 // Last entry is always cancel
            } else {
                progress_bar.suspend(|| { // Hide the progress bar while showing this message, otherwise the background tick will redraw it over our prompt!
                    let r = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
                        .with_prompt(prompt)
                        .items(&items.iter().map(|i| &i.0).collect::<Vec<&String>>())
                        .default(0).interact_opt(); //TODO: remember default from previous time the same prompt was shown?
                    let response = match r {
                        Ok(Some(i)) => i,
                        _ => items.len() - 1 // Last entry is always cancel, e.g. if user presses q or Esc
                    };
                    response
                })
            }
        }
    };

    items[response_idx].1
}

fn copy_file(
    path: &RootRelativePath,
    size: u64,
    modified_time: SystemTime,
    ctx: &mut SyncContext) -> Result<(), Vec<String>> {
    if !ctx.dry_run {
        trace!("Fetching from src {}", format_root_relative(&path, &ctx.src_root));
        ctx.src_comms
            .send_command(Command::GetFileContent {
                path: path.clone(),
            });
        // Large files are split into chunks, loop until all chunks are transferred.
        loop {
            let (data, more_to_follow) = match ctx.src_comms.receive_response() {
                Response::FileContent { data, more_to_follow } => (data, more_to_follow),
                x => return Err(vec![format!("Unexpected response fetching {} from src: {:?}", format_root_relative(&path, &ctx.src_root), x)]),
            };
            trace!("Create/update on dest {}", format_root_relative(&path, &ctx.dest_root));
            ctx.dest_comms
                .send_command(Command::CreateOrUpdateFile {
                    path: path.clone(),
                    data,
                    set_modified_time: if more_to_follow { None } else { Some(modified_time) }, // Only set the modified time after the final chunk
                    more_to_follow,
                });

            // For large files, it might be a while before process_dest_responses is called in the main sync function,
            // so check it periodically here too. 
            process_dest_responses(ctx.dest_comms, &ctx.progress_bar, &mut ctx.stats, None)?;

            if !more_to_follow {
                break;
            }
        }
    } else {
        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
        info!("Would copy {} => {}",
            format_root_relative(&path, &ctx.src_root),
            format_root_relative(&path, &ctx.dest_root));
    }

    ctx.stats.num_files_copied += 1;
    ctx.stats.num_bytes_copied += size;
    ctx.stats.copied_file_size_hist.add(size);

    Ok(())
}
