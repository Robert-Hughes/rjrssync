use std::{
    cmp::Ordering,
    fmt::{Display, Write}, time::Instant,
};

use log::{debug, error, info, trace};
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
        let max = *self.buckets.iter().max().unwrap(); //TODO: could be empty! (everything filtered)
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

pub fn sync(
    src_folder: String,
    dest_folder: String,
    exclude_filters: Vec<String>,
    dry_run: bool,
    show_stats: bool,
    mut src_comms: Comms,
    mut dest_comms: Comms,
) -> Result<(), ()> {
    src_comms
        .send_command(Command::GetEntries { root: src_folder, exclude_filters: exclude_filters.clone() })
        .unwrap();
    dest_comms
        .send_command(Command::GetEntries { root: dest_folder, exclude_filters })
        .unwrap();

    let mut stats = Stats::default();

    let mut src_entries = Vec::new();
    loop {
        match src_comms.receive_response() {
            Ok(Response::Entry(d)) => {
                trace!("{:?}", d);
                match d.entry_type {
                    EntryType::File => {
                        stats.num_src_files += 1;
                        stats.src_total_bytes += d.size;
                        stats.src_file_size_hist.add(d.size);
                    }
                    EntryType::Folder => stats.num_src_folders += 1,
                }
                src_entries.push(d);
            }
            Ok(Response::EndOfEntries) => break,
            r => {
                error!("Unexpected response: {:?}", r);
                return Err(());
            }
        }
    }
    let mut dest_entries = Vec::new();
    loop {
        match dest_comms.receive_response() {
            Ok(Response::Entry(d)) => {
                trace!("{:?}", d);
                match d.entry_type {
                    EntryType::File => {
                        stats.num_dest_files += 1;
                        stats.dest_total_bytes += d.size;
                    }
                    EntryType::Folder => stats.num_dest_folders += 1,
                }
                dest_entries.push(d);
            }
            Ok(Response::EndOfEntries) => break,
            r => {
                error!("Unexpected response: {:?}", r);
                return Err(());
            }
        }
    }
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
    }

    // Delete dest entries that don't exist on the source. This needs to be done first in case there
    // are entries with the same name but different type (files vs folders).
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly also problems with files being filtered
    // so the folder is needed still as there are filtered-out files in there?)
    for dest_entry in dest_entries.iter().rev() {
        if !src_entries
            .iter()
            .any(|f| f.path == dest_entry.path && f.entry_type == dest_entry.entry_type)
        {
            debug!("Deleting {}", dest_entry.path);
            let c = match dest_entry.entry_type {
                EntryType::File => {
                    stats.num_files_deleted += 1;
                    stats.num_bytes_deleted += dest_entry.size;
                    Command::DeleteFile {
                        path: dest_entry.path.to_string(),
                    }
                }
                EntryType::Folder => {
                    stats.num_folders_deleted += 1;
                    Command::DeleteFolder {
                        path: dest_entry.path.to_string(),
                    }
                }
            };
            if !dry_run {
                dest_comms.send_command(c).unwrap();
                match dest_comms.receive_response() {
                    Ok(doer::Response::Ack) => (),
                    _ => {
                        error!("Wrong response");
                        return Err(());
                    }
                };
            } else {
                // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                info!("Would delete {}", dest_entry.path);
            }
        }
    }

    let start = Instant::now();

    for src_entry in src_entries {
        match dest_entries
            .iter()
            .find(|f| f.path == src_entry.path && f.entry_type == src_entry.entry_type)
        {
            Some(dest_entry) => match src_entry.entry_type {
                EntryType::File => match src_entry.modified_time.cmp(&dest_entry.modified_time) {
                    Ordering::Less => {
                        error!(
                            "{}: Dest file is newer - how did this happen!",
                            src_entry.path
                        );
                        return Err(());
                    }
                    Ordering::Equal => {
                        trace!("{}: Same modified time - skipping", src_entry.path);
                    }
                    Ordering::Greater => {
                        debug!("{}: source file newer - copying", src_entry.path);
                        copy_file(&src_entry, &mut src_comms, &mut dest_comms, &mut stats, dry_run)?
                    }
                },
                EntryType::Folder => {
                    trace!("{}: folder already exists - nothing to do", src_entry.path)
                }
            },
            None => match src_entry.entry_type {
                EntryType::File => {
                    debug!("{}: Dest file doesn't exist - copying", src_entry.path);
                    copy_file(&src_entry, &mut src_comms, &mut dest_comms, &mut stats, dry_run)?
                }
                EntryType::Folder => {
                    debug!("{}: dest folder doesn't exists - creating", src_entry.path);
                    stats.num_folders_created += 1;
                    if !dry_run {
                        dest_comms
                            .send_command(Command::CreateFolder {
                                path: src_entry.path.to_string(),
                            })
                            .unwrap();
                        match dest_comms.receive_response() {
                            Ok(doer::Response::Ack) => (),
                            _ => {
                                error!("Wrong response");
                                return Err(());
                            }
                        };
                    } else {
                        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
                        info!("Would create {}", src_entry.path);
                    }
                }
            },
        }
    }

    let elapsed = start.elapsed().as_secs_f32();

    // Note that we print all the stats at the end (even though we could print the delete stats earlier),
    // so that they are together in the output (e.g. for dry run or --verbose, they could be a lot of other
    // messages between them)    
    if stats.num_files_deleted + stats.num_folders_deleted > 0 {
        info!(
            "{} {} file(s){} and {} folder(s)",
            if !dry_run { "Deleted" } else { "Would delete" },
            stats.num_files_deleted.separate_with_commas(), 
            if show_stats { format!(" totalling {} bytes", stats.num_bytes_deleted.separate_with_commas()) } else { "".to_string() },
            stats.num_folders_deleted.separate_with_commas()
        );
    }
    if stats.num_files_copied + stats.num_folders_created > 0 {
        info!(
            "{} {} file(s){} and {} {} folder(s){}",
            if !dry_run { "Copied" } else { "Would copy" },           
            stats.num_files_copied.separate_with_commas(),
            if show_stats { format!(" totalling {} bytes", stats.num_bytes_copied.separate_with_commas()) } else { "".to_string() },
            if !dry_run { "created" } else { "would create" },
            stats.num_folders_created.separate_with_commas(),
            if !dry_run && show_stats { 
                format!(", in {:.1} seconds ({} bytes/s)", 
                    elapsed, (stats.num_bytes_copied as f32 / elapsed as f32).round().separate_with_commas())
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
    src_file: &EntryDetails,
    src_comms: &mut Comms,
    dest_comms: &mut Comms,
    stats: &mut Stats,
    dry_run: bool,
) -> Result<(), ()> {
    if !dry_run {
        trace!("Fetching {}", src_file.path);
        src_comms
            .send_command(Command::GetFileContent {
                path: src_file.path.to_string(),
            })
            .unwrap();
        let data = match src_comms.receive_response() {
            Ok(Response::FileContent { data }) => data,
            _ => {
                error!("Wrong response");
                return Err(());
            }
        };
        trace!("Writing {}", src_file.path);
        dest_comms
            .send_command(Command::CreateOrUpdateFile {
                path: src_file.path.to_string(),
                data,
                set_modified_time: Some(src_file.modified_time),
            })
            .unwrap();
        match dest_comms.receive_response() {
            Ok(doer::Response::Ack) => (),
            _ => {
                error!("Wrong response");
                return Err(());
            }
        };
    } else {
        // Print dry-run as info level, as presumably the user is interested in exactly _what_ will be copied
        info!("Would copy {}", src_file.path);
    }

    stats.num_files_copied += 1;
    stats.num_bytes_copied += src_file.size;
    stats.copied_file_size_hist.add(src_file.size);

    Ok(())
}
