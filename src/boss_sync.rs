use std::{
    cmp::Ordering,
    fmt::{Display, Write}, time::{Instant, SystemTime}, collections::HashMap,
};

use log::{debug, info, trace};
use thousands::Separable;

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
    pub src_total_bytes: u64,
    pub src_file_size_hist: FileSizeHistogram,

    pub num_dest_files: u32,
    pub num_dest_folders: u32,
    pub dest_total_bytes: u64,

    pub num_files_copied: u32,
    pub num_bytes_copied: u64,
    pub num_folders_created: u32,
    pub num_files_deleted: u32,
    pub num_bytes_deleted: u64,
    pub num_folders_deleted: u32,
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

pub fn sync(
    src_root: &str,
    mut dest_root: String,
    filters: &[Filter],
    dry_run: bool,
    show_stats: bool,
    src_comms: &mut Comms,
    dest_comms: &mut Comms,
) -> Result<(), String> {
    let mut stats = Stats::default();

    let sync_start = Instant::now();

    // First get details of the root file/folder etc. of each side, as this might affect the sync
    // before we start it (e.g. errors, or changing the dest root)

    // Source SetRoot
    src_comms.send_command(Command::SetRoot { root: src_root.to_string() })?;
    let src_root_details = match src_comms.receive_response() {
        Ok(Response::RootDetails(d)) => {
            match d {
                RootDetails::None => return Err(format!("src path '{src_root}' doesn't exist!")),
                RootDetails::File => {
                    // Referring to an existing file with a trailing slash is an error, because it implies
                    // that the user thinks it is a folder, and so could lead to unwanted behaviour.
                    // In some environments (e.g. Linux), this is caught on the doer side when it attempts to
                    // get the metadata for the root, but on some environments it isn't caught (Windows, depending on the drive)
                    // so we have to do our own check here.
                    // Note that we can't use std::path::is_separator because this might be a remote path, so the current platform
                    // is irrelevant
                    if src_root.chars().last().unwrap() == '/' || src_root.chars().last().unwrap() == '\\' {
                        return Err(format!("src path '{}' is a file but is referred to with a trailing slash.", src_root));
                    }
                },
                RootDetails::Folder => (),  // Nothing special to do
            };
            d
        }
        r => return Err(format!("Unexpected response getting root details from src: {:?}", r)),
    };

    // Dest SetRoot
    dest_comms.send_command(Command::SetRoot { root: dest_root.to_string() })?;
    let mut dest_root_details = match dest_comms.receive_response() {
        Ok(Response::RootDetails(d)) => {
            match d {
                RootDetails::None => (), // Dest root doesn't exist, but that's fine (we will create it later)
                RootDetails::File => {
                    // Referring to an existing file with a trailing slash is an error, because it implies
                    // that the user thinks it is a folder, and so could lead to unwanted behaviour
                    // In some environments (e.g. Linux), this is caught on the doer side when it attempts to
                    // get the metadata for the root, but on some environments it isn't caught (Windows, depending on the drive)
                    // so we have to do our own check here.
                    // Note that we can't use std::path::is_separator because this might be a remote path, so the current platform
                    // is irrelevant
                    if dest_root.chars().last().unwrap() == '/' || dest_root.chars().last().unwrap() == '\\' {
                        return Err(format!("dest path '{}' is a file but is referred to with a trailing slash.", dest_root));
                    }
                }
                RootDetails::Folder => (), // Nothing special to do
            }
            d
        }
        r => return Err(format!("Unexpected response getting root details from dest: {:?}", r)),
    };

    // If src is a file, and the dest path ends in a slash, then we want to sync the file
    // _inside_ the folder, rather then replacing the folder with the file (see README for reasoning).
    // To do this, we modify the dest path and then continue as if that was the path provided by the
    // user.
    let last_dest_char = dest_root.chars().last();
    // Note that we can't use std::path::is_separator (or similar) because this might be a remote path, so the current platform
    // isn't appropriate.
    let dest_trailing_slash = last_dest_char == Some('/') || last_dest_char == Some('\\');
    if src_root_details == RootDetails::File && dest_trailing_slash {
        let src_filename = src_root.split(|c| c == '/' || c == '\\').last();
        if let Some(c) = src_filename {
            dest_root = dest_root.to_string() + c;
            debug!("Modified dest path to {}", dest_root);

            dest_comms.send_command(Command::SetRoot { root: dest_root.clone() })?;
            dest_root_details = match dest_comms.receive_response() {
                Ok(Response::RootDetails(t)) => t,
                r => return Err(format!("Unexpected response getting root details from dest: {:?}", r)),
            }
        }
    }

    // If the dest doesn't yet exist, make sure that all its ancestors are created, so that
    // when we come to create the dest path itself, it can succeed
    if dest_root_details == RootDetails::None {
        dest_comms.send_command(Command::CreateRootAncestors)?;
        match dest_comms.receive_response() {
            Ok(Response::Ack) => (),
            r => return Err(format!("Unexpected response from creating root ancestors on dest: {:?}", r)),
        }
    }

    // Fetch all the entries for the source path
    let mut src_entries = Vec::new();
    let mut src_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();
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
                }
                src_entries.push((p.clone(), d.clone()));
                src_entries_lookup.insert(p, d);
            }
            Ok(Response::EndOfEntries) => break,
            r => return Err(format!("Unexpected response getting entries from src: {:?}", r)),
        }
    }

    // Fetch all the entries for the dest path
    let mut dest_entries = Vec::new();
    let mut dest_entries_lookup = HashMap::<RootRelativePath, EntryDetails>::new();
    // The dest might not exist yet, which is fine - continue anyway with an empty array of dest entries
    // and we will create the dest as part of the sync.
    if dest_root_details != RootDetails::None {
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
                    }
                    dest_entries.push((p.clone(), d.clone()));
                    dest_entries_lookup.insert(p, d);
                },
                Ok(Response::EndOfEntries) => break,
                r => return Err(format!("Unexpected response getting entries from dest: {:?}", r)),
            }
        }
    }

    let query_elapsed = sync_start.elapsed().as_secs_f32();

    if show_stats {
        info!("Source: {} file(s) totalling {} bytes and {} folder(s) => Dest: {} file(s) totalling {} bytes and {} folder(s)",
            stats.num_src_files.separate_with_commas(),
            stats.src_total_bytes.separate_with_commas(),
            stats.num_src_folders.separate_with_commas(),
            stats.num_dest_files.separate_with_commas(),
            stats.dest_total_bytes.separate_with_commas(),
            stats.num_dest_folders.separate_with_commas());
        info!("Source file size distribution:");
        info!("{}", stats.src_file_size_hist);
        info!("Queried in {} seconds", query_elapsed);
    }

    // Delete dest entries that don't exist on the source. This needs to be done first in case there
    // are entries with the same name but different type (files vs folders).
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly would also have problems with
    // files being filtered so the folder is needed still as there are filtered-out files in there,
    // see test_remove_dest_folder_with_excluded_files())
    let delete_start = Instant::now();
    for (dest_path, dest_details) in dest_entries.iter().rev() {
        let s = src_entries_lookup.get(dest_path) ;
        if !s.is_some() || !s.unwrap().is_same_type(dest_details) {
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
    }
    let delete_elapsed = delete_start.elapsed().as_secs_f32();

    // Copy entries that don't exist, or do exist but are out-of-date.
    let copy_start = Instant::now();
    for (path, src_details) in src_entries {
        let dest_details = dest_entries_lookup.get(&path);
        match dest_details {
            Some(dest_details) if dest_details.is_same_type(&src_details) => {
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
                    }
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
                }
            },
        }
    }

    let copy_elapsed = copy_start.elapsed().as_secs_f32();

    // Note that we print all the stats at the end (even though we could print the delete stats earlier),
    // so that they are together in the output (e.g. for dry run or --verbose, they could be a lot of other
    // messages between them)
    if (stats.num_files_deleted + stats.num_folders_deleted > 0) || show_stats {
        info!(
            "{} {} file(s){} and {} folder(s){}",
            if !dry_run { "Deleted" } else { "Would delete" },
            stats.num_files_deleted.separate_with_commas(),
            if show_stats { format!(" totalling {} bytes", stats.num_bytes_deleted.separate_with_commas()) } else { "".to_string() },
            stats.num_folders_deleted.separate_with_commas(),
            if !dry_run && show_stats {
                format!(", in {:.1} seconds", delete_elapsed)
            } else { "".to_string() },
        );
    }
    if (stats.num_files_copied + stats.num_folders_created > 0) || show_stats {
        info!(
            "{} {} file(s){} and {} {} folder(s){}",
            if !dry_run { "Copied" } else { "Would copy" },
            stats.num_files_copied.separate_with_commas(),
            if show_stats { format!(" totalling {} bytes", stats.num_bytes_copied.separate_with_commas()) } else { "".to_string() },
            if !dry_run { "created" } else { "would create" },
            stats.num_folders_created.separate_with_commas(),
            if !dry_run && show_stats {
                format!(", in {:.1} seconds ({} bytes/s)",
                    copy_elapsed, (stats.num_bytes_copied as f32 / copy_elapsed as f32).round().separate_with_commas())
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
        + stats.num_files_copied
        + stats.num_folders_created
        == 0
    {
        info!("Nothing to do!");
    }

    Ok(())
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
        let data = match src_comms.receive_response() {
            Ok(Response::FileContent { data }) => data,
            x => return Err(format!("Unexpected response fetching {} from src: {:?}", format_root_relative(&path, &src_root), x)),
        };
        trace!("Create/update on dest {}", format_root_relative(&path, &dest_root));
        dest_comms
            .send_command(Command::CreateOrUpdateFile {
                path: path.clone(),
                data,
                set_modified_time: Some(modified_time),
            })?;
        match dest_comms.receive_response() {
            Ok(doer::Response::Ack) => (),
            x => return Err(format!("Unexpected response response creeating/updating on dest {}: {:?}", format_root_relative(&path, &dest_root), x)),
        };
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
