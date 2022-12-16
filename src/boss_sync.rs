use std::{
    cmp::Ordering,
    fmt::{Display, Write}, time::{Instant, SystemTime, Duration}, collections::HashMap, thread,
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
    pub num_src_files: u32,
    pub num_src_folders: u32,
    pub num_src_symlinks: u32,
    pub src_total_bytes: u64,
    pub src_file_size_hist: FileSizeHistogram,

    pub num_dest_files: u32,
    pub num_dest_folders: u32,
    pub num_dest_symlinks: u32,
    pub dest_total_bytes: u64,

    pub num_files_deleted: u32,
    pub num_bytes_deleted: u64,
    pub num_folders_deleted: u32,
    pub num_symlinks_deleted: u32,

    pub num_files_copied: u32,
    pub num_bytes_copied: u64,
    pub num_folders_created: u32,
    pub num_symlinks_copied: u32,
    pub copied_file_size_hist: FileSizeHistogram,
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

pub fn sync(
    src_root: &str,
    mut dest_root: String,
    filters: &[Filter],
    dry_run: bool,
    show_stats: bool,
    src_comms: &mut Comms,
    dest_comms: &mut Comms,
) -> Result<(), String> {
    profile_this!();

    let mut stats = Stats::default();

    let sync_start = Instant::now();

    // First get details of the root file/folder etc. of each side, as this might affect the sync
    // before we start it (e.g. errors, or changing the dest root)

    let progress = ProgressBar::new_spinner().with_message("Querying...");
    progress.enable_steady_tick(Duration::from_millis(250));

    // Source SetRoot
    let timer = start_timer("SetRoot src");
    src_comms.send_command(Command::SetRoot { root: src_root.to_string() })?;
    let src_root_details = match src_comms.receive_response() {
        Ok(Response::RootDetails { root_details, platform_differentiates_symlinks: _ }) => {
            match &root_details {
                None => return Err(format!("src path '{src_root}' doesn't exist!")),
                Some(d) => if let Err(e) = validate_trailing_slash(src_root, &d) {
                    return Err(format!("src path {}", e));
                }
            };
            root_details
        }
        r => return Err(format!("Unexpected response getting root details from src: {:?}", r)),
    };
    let src_root_details = src_root_details.unwrap();
    stop_timer(timer);

    // Dest SetRoot
    let timer = start_timer("SetRoot dest");
    dest_comms.send_command(Command::SetRoot { root: dest_root.to_string() })?;
    let (mut dest_root_details, dest_platform_differentiates_symlinks) = match dest_comms.receive_response() {
        Ok(Response::RootDetails { root_details, platform_differentiates_symlinks }) => {
            match &root_details {
                None => (), // Dest root doesn't exist, but that's fine (we will create it later)
                Some(d) => if let Err(e) = validate_trailing_slash(&dest_root, &d) {
                    return Err(format!("dest path {}", e));
                }
            }
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
    let last_dest_char = dest_root.chars().last();
    // Note that we can't use std::path::is_separator (or similar) because this might be a remote path, so the current platform
    // isn't appropriate.
    let dest_trailing_slash = last_dest_char == Some('/') || last_dest_char == Some('\\');
    if matches!(src_root_details, EntryDetails::File {..} | EntryDetails::Symlink { .. }) && dest_trailing_slash {
        let src_filename = src_root.split(|c| c == '/' || c == '\\').last();
        if let Some(c) = src_filename {
            dest_root = dest_root.to_string() + c;
            debug!("Modified dest path to {}", dest_root);

            dest_comms.send_command(Command::SetRoot { root: dest_root.clone() })?;
            dest_root_details = match dest_comms.receive_response() {
                Ok(Response::RootDetails { root_details, platform_differentiates_symlinks: _ }) => root_details,
                r => return Err(format!("Unexpected response getting root details from dest: {:?}", r)),
            }
        }
    }

    // If the dest doesn't yet exist, make sure that all its ancestors are created, so that
    // when we come to create the dest path itself, it can succeed
    if dest_root_details.is_none() {
        dest_comms.send_command(Command::CreateRootAncestors)?;
        match dest_comms.receive_response() {
            Ok(Response::Ack) => (),
            r => return Err(format!("Unexpected response from creating root ancestors on dest: {:?}", r)),
        }
    }

    // Fetch all the entries for the source path and the dest path, if they are folders
    // Do these each on a separate thread so they can be done in parallel with each other
    let timer = start_timer("GetEntries x 2");
    let thread_result : Result<_, String> = thread::scope(|scope| {
        // Source GetEntries
        let src_thread = thread::Builder::new()
            .name("src_entries_fetching_thread".to_string())
            .spawn_scoped(scope, ||
        {
            let mut src_entries = Vec::new();
            let mut src_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();

            // Add the root entry - we already got the details for this before
            src_entries.push((RootRelativePath::root(), src_root_details.clone()));
            src_entries_lookup.insert(RootRelativePath::root(), src_root_details.clone());

            if matches!(src_root_details, EntryDetails::Folder) {
                src_comms.send_command(Command::GetEntries { filters: filters.to_vec() })?;
                loop {
                    match src_comms.receive_response() {
                        Ok(Response::Entry((p, d))) => {
                            trace!("Source entry '{}': {:?}", p, d);
                            match d {
                                EntryDetails::File { size, .. } => {
                                    stats.num_src_files += 1;
                                    stats.src_total_bytes += size;
                                    stats.src_file_size_hist.add(size);
                                }
                                EntryDetails::Folder => stats.num_src_folders += 1,
                                EntryDetails::Symlink { .. } => stats.num_src_symlinks += 1,
                            }
                            src_entries.push((p.clone(), d.clone()));
                            src_entries_lookup.insert(p, d);
                        }
                        Ok(Response::EndOfEntries) => break,
                        r => return Err(format!("Unexpected response getting entries from src: {:?}", r)),
                    }
                }
            }
            Ok((src_entries, src_entries_lookup, src_comms))
        }).expect("OS error spawning a thread");

        // Dest GetEntries
        let dest_thread = thread::Builder::new()
            .name("dest_entries_fetching_thread".to_string())
            .spawn_scoped(scope, ||
        {
            let mut dest_entries = Vec::new();
            let mut dest_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();

            // Add the root entry - we already got the details for this before
            if dest_root_details.is_some() {
                dest_entries.push((RootRelativePath::root(), dest_root_details.clone().unwrap()));
                dest_entries_lookup.insert(RootRelativePath::root(), dest_root_details.clone().unwrap());
            }

            if matches!(dest_root_details, Some(EntryDetails::Folder)) {
                dest_comms.send_command(Command::GetEntries { filters: filters.to_vec() })?;
                loop {
                    match dest_comms.receive_response() {
                        Ok(Response::Entry((p, d))) => {
                            trace!("Dest entry '{}': {:?}", p, d);
                            match d {
                                EntryDetails::File { size, .. } => {
                                    stats.num_dest_files += 1;
                                    stats.dest_total_bytes += size;
                                }
                                EntryDetails::Folder => stats.num_dest_folders += 1,
                                EntryDetails::Symlink { .. } => stats.num_dest_symlinks += 1,
                            }
                            dest_entries.push((p.clone(), d.clone()));
                            dest_entries_lookup.insert(p, d);
                        }
                        Ok(Response::EndOfEntries) => break,
                        r => return Err(format!("Unexpected response getting entries from dest: {:?}", r)),
                    }
                }
            }
            Ok((dest_entries, dest_entries_lookup, dest_comms))
        }).expect("OS error spawning a thread");

        // Wait for both threads to finish, and pass the results back to the main thread.
        let (src_entries, src_entries_lookup, src_comms) = src_thread.join().expect("Thread panicked")?;
        let (dest_entries, dest_entries_lookup, dest_comms) = dest_thread.join().expect("Thread panicked")?;

        Ok((src_entries, src_entries_lookup, src_comms, dest_entries, dest_entries_lookup, dest_comms))
    });
    let (src_entries, src_entries_lookup, src_comms, dest_entries, dest_entries_lookup, dest_comms) = thread_result?;
    stop_timer(timer);

    let query_elapsed = sync_start.elapsed().as_secs_f32();
    progress.finish_and_clear();

    if show_stats {
        info!("Source: {} file(s) totalling {}, {} folder(s) and {} symlink(s)",
            HumanCount(stats.num_src_files as u64),
            HumanBytes(stats.src_total_bytes),
            HumanCount(stats.num_src_folders as u64),
            HumanCount(stats.num_src_symlinks as u64),
        );
        info!("  =>");
        info!("Dest: {} file(s) totalling {}, {} folder(s) and {} symlink(s)",
            HumanCount(stats.num_dest_files as u64),
            HumanBytes(stats.dest_total_bytes),
            HumanCount(stats.num_dest_folders as u64),
            HumanCount(stats.num_dest_symlinks as u64),
        );
        info!("Source file size distribution:");
        info!("{}", stats.src_file_size_hist);
        info!("Queried in {:.2} seconds", query_elapsed);
    }

    // Delete dest entries that don't exist on the source. This needs to be done first in case there
    // are entries with the same name but different type (files vs folders).
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly would also have problems with
    // files being filtered so the folder is needed still as there are filtered-out files in there,
    // see test_remove_dest_folder_with_excluded_files())
    let progress = ProgressBar::new(dest_entries.len() as u64).with_message("Deleting...")
        .with_style(ProgressStyle::with_template("[{elapsed}] {bar:40.green/black} {human_pos:>7}/{human_len:7} {msg}").unwrap());
    progress.enable_steady_tick(Duration::from_millis(250));
    let timer = start_timer("Deleting");
    let delete_start = Instant::now();
    for (dest_path, dest_details) in dest_entries.iter().rev() {
        let s = src_entries_lookup.get(dest_path);
        if !s.is_some() || should_delete(s.unwrap(), dest_details, dest_platform_differentiates_symlinks) {
            debug!("Deleting from dest {}", format_root_relative(&dest_path, &dest_root));
            let c = match dest_details {
                EntryDetails::File { size, .. } => {
                    stats.num_files_deleted += 1;
                    stats.num_bytes_deleted += size;
                    Command::DeleteFile {
                        path: dest_path.clone(),
                    }
                }
                EntryDetails::Folder => {
                    stats.num_folders_deleted += 1;
                    Command::DeleteFolder {
                        path: dest_path.clone(),
                    }
                }
                EntryDetails::Symlink { kind, .. } => {
                    stats.num_symlinks_deleted += 1;
                    Command::DeleteSymlink {
                        path: dest_path.clone(),
                        kind: *kind,
                    }
                }
            };
            if !dry_run {
                dest_comms.send_command(c)?;
                match dest_comms.receive_response() {
                    Ok(doer::Response::Ack) => (),
                    r => return Err(format!("Unexpected response from deletion of {} on dest: {:?}",
                        format_root_relative(&dest_path, &dest_root), r)),
                };
            } else {
                // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be deleted
                info!("Would delete from dest {}", format_root_relative(&dest_path, &dest_root));
            }
        }
        progress.inc(1);
    }
    let delete_elapsed = delete_start.elapsed().as_secs_f32();
    stop_timer(timer);
    progress.finish_and_clear();

    // Copy entries that don't exist, or do exist but are out-of-date.
    let progress = ProgressBar::new(src_entries.len() as u64).with_message("Copying...")
        .with_style(ProgressStyle::with_template("[{elapsed}] {bar:40.green/black} {human_pos:>7}/{human_len:7} {msg}").unwrap());
    progress.enable_steady_tick(Duration::from_millis(250));
    let timer = start_timer("Copying");
    let copy_start = Instant::now();
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
                        match src_modified_time.cmp(&dest_modified_time) {
                            Ordering::Less => {
                                return Err(format!(
                                    "Dest file {} is newer than src file {}. Will not overwrite.",
                                    format_root_relative(&path, &dest_root),
                                    format_root_relative(&path, src_root)
                                ));
                            }
                            Ordering::Equal => {
                                trace!("Dest file {} has same modified time as src file {}. Will not update.",
                                    format_root_relative(&path, &dest_root),
                                    format_root_relative(&path, src_root));
                            }
                            Ordering::Greater => {
                                debug!("Source file {} is newer than dest file {}. Will copy.",
                                    format_root_relative(&path, &src_root),
                                    format_root_relative(&path, &dest_root));
                                copy_file(&path, size, src_modified_time, src_comms, dest_comms, &mut stats, dry_run, &src_root, &dest_root)?
                            }
                        }
                    },
                    EntryDetails::Folder => {
                        // Folders are always up-to-date
                        trace!("Source folder {} already exists on dest {} - nothing to do",
                            format_root_relative(&path, &src_root),
                            format_root_relative(&path, &dest_root))
                    },
                    EntryDetails::Symlink { .. } => {
                        // Symlinks are always up-to-date, if should_delete indicated that we shouldn't delete it
                        trace!("Source symlink {} already exists on dest {} - nothing to do",
                            format_root_relative(&path, &src_root),
                            format_root_relative(&path, &dest_root))
                    },
                }
            },
            _ => match src_details {
                EntryDetails::File { size, modified_time: src_modified_time } => {
                    debug!("Source file {} file doesn't exist on dest - copying", format_root_relative(&path, &src_root));
                    copy_file(&path, size, src_modified_time, src_comms, dest_comms, &mut stats, dry_run, &src_root, &dest_root)?
                }
                EntryDetails::Folder => {
                    debug!("Source folder {} doesn't exist on dest - creating", format_root_relative(&path, &src_root));
                    stats.num_folders_created += 1;
                    if !dry_run {
                        dest_comms
                            .send_command(Command::CreateFolder {
                                path: path.clone(),
                            })
                            ?;
                        match dest_comms.receive_response() {
                            Ok(doer::Response::Ack) => (),
                            x => return Err(format!("Unexpected response creating on dest {}: {:?}", format_root_relative(&path, &dest_root), x)),
                        };
                    } else {
                        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                        info!("Would create dest folder {}", format_root_relative(&path, &dest_root));
                    }
                },
                EntryDetails::Symlink { kind, target } => {
                    debug!("Source {} symlink doesn't exist on dest - copying", format_root_relative(&path, &src_root));
                    stats.num_symlinks_copied += 1;
                    if !dry_run {
                        dest_comms
                            .send_command(Command::CreateSymlink {
                                path: path.clone(),
                                kind,
                                target,
                            })?;
                        match dest_comms.receive_response() {
                            Ok(doer::Response::Ack) => (),
                            x => return Err(format!("Unexpected response creating symlink on dest {}: {:?}", format_root_relative(&path, &dest_root), x)),
                        };
                    } else {
                        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                        info!("Would create dest symlink {}", format_root_relative(&path, &dest_root));
                    }
                }
            },
        }
        progress.inc(1);
    }
    stop_timer(timer);
    let copy_elapsed = copy_start.elapsed().as_secs_f32();
    progress.finish_and_clear();

    // Note that we print all the stats at the end (even though we could print the delete stats earlier),
    // so that they are together in the output (e.g. for dry run or --verbose, they could be a lot of other
    // messages between them)
    if (stats.num_files_deleted + stats.num_folders_deleted + stats.num_symlinks_deleted > 0) || show_stats {
        info!(
            "{} {} file(s){}, {} folder(s) and {} symlink(s){}",
            if !dry_run { "Deleted" } else { "Would delete" },
            HumanCount(stats.num_files_deleted as u64),
            if show_stats { format!(" totalling {}", HumanBytes(stats.num_bytes_deleted)) } else { "".to_string() },
            HumanCount(stats.num_folders_deleted as u64),
            HumanCount(stats.num_symlinks_deleted as u64),
            if !dry_run && show_stats {
                format!(", in {:.1} seconds", delete_elapsed)
            } else { "".to_string() },
        );
    }
    if (stats.num_files_copied + stats.num_folders_created + stats.num_symlinks_copied > 0) || show_stats {
        info!(
            "{} {} file(s){}, {} {} folder(s) and {} {} symlink(s){}",
            if !dry_run { "Copied" } else { "Would copy" },
            HumanCount(stats.num_files_copied as u64),
            if show_stats { format!(" totalling {}", HumanBytes(stats.num_bytes_copied)) } else { "".to_string() },
            if !dry_run { "created" } else { "would create" },
            HumanCount(stats.num_folders_created as u64),
            if !dry_run { "copied" } else { "would copy" },
            HumanCount(stats.num_symlinks_copied as u64),
            if !dry_run && show_stats {
                format!(", in {:.1} seconds ({}/s)",
                    copy_elapsed, HumanBytes((stats.num_bytes_copied as f32 / copy_elapsed as f32).round() as u64))
            } else { "".to_string() },
        );
        if show_stats {
            info!("{} file size distribution:",
                if !dry_run { "Copied" } else { "Would copy" },
            );
            info!("{}", stats.copied_file_size_hist);
        }
    }
    if stats.num_files_deleted
        + stats.num_folders_deleted
        + stats.num_symlinks_deleted
        + stats.num_files_copied
        + stats.num_folders_created
        + stats.num_symlinks_copied
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

fn copy_file(
    path: &RootRelativePath,
    size: u64,
    modified_time: SystemTime,
    src_comms: &mut Comms,
    dest_comms: &mut Comms,
    stats: &mut Stats,
    dry_run: bool,
    src_root: &str,
    dest_root: &str,
) -> Result<(), String> {
    if !dry_run {
        trace!("Fetching from src {}", format_root_relative(&path, &src_root));
        src_comms
            .send_command(Command::GetFileContent {
                path: path.clone(),
            })?;
        // Large files are split into chunks, loop until all chunks are transferred.
        loop {
            let (data, more_to_follow) = match src_comms.receive_response() {
                Ok(Response::FileContent { data, more_to_follow }) => (data, more_to_follow),
                x => return Err(format!("Unexpected response fetching {} from src: {:?}", format_root_relative(&path, &src_root), x)),
            };
            trace!("Create/update on dest {}", format_root_relative(&path, &dest_root));
            dest_comms
                .send_command(Command::CreateOrUpdateFile {
                    path: path.clone(),
                    data,
                    set_modified_time: if more_to_follow { None } else { Some(modified_time) }, // Only set the modified time after the final chunk
                    more_to_follow,
                })?;
            match dest_comms.receive_response() {
                Ok(doer::Response::Ack) => (),
                x => return Err(format!("Unexpected response response creeating/updating on dest {}: {:?}", format_root_relative(&path, &dest_root), x)),
            };

            if !more_to_follow {
                break;
            }
        }
    } else {
        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
        info!("Would copy {} => {}",
            format_root_relative(&path, &src_root),
            format_root_relative(&path, &dest_root));
    }

    stats.num_files_copied += 1;
    stats.num_bytes_copied += size;
    stats.copied_file_size_hist.add(size);

    Ok(())
}
