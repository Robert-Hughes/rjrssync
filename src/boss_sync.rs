use std::cmp::Ordering;

use log::{debug, error, info};

use crate::*;

#[derive(Default)]
struct Stats {
    pub num_src_files: u32,
    pub num_src_folders: u32,
    pub src_total_bytes: u64,

    pub num_dest_files: u32,
    pub num_dest_folders: u32,
    pub dest_total_bytes: u64,

    pub num_files_copied: u32,
    pub num_bytes_copied: u64,
    pub num_folders_created: u32,
}

pub fn sync(src_folder: String, dest_folder: String, mut src_comms: Comms, mut dest_comms: Comms) -> Result<(), ()> {
    src_comms.send_command(Command::GetEntries { root: src_folder }).unwrap();
    dest_comms.send_command(Command::GetEntries { root: dest_folder }).unwrap();

    //TODO: what about symlinks

    let mut stats = Stats::default();

    let mut src_entries = Vec::new();
    loop {
        match src_comms.receive_response() {
            Ok(Response::Entry(d)) => {
                debug!("{:?}", d);
                match d.entry_type {
                    EntryType::File => { 
                        stats.num_src_files += 1;
                        stats.src_total_bytes += d.size;
                    }
                    EntryType::Folder => stats.num_src_folders += 1,
                }
                src_entries.push(d);
            },
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
                debug!("{:?}", d);
                match d.entry_type {
                    EntryType::File => { 
                        stats.num_dest_files += 1;
                        stats.dest_total_bytes += d.size;
                    }
                    EntryType::Folder => stats.num_dest_folders += 1,
                }
                dest_entries.push(d);
            },
            Ok(Response::EndOfEntries) => break,
            r => {
                error!("Unexpected response: {:?}", r);
                return Err(());
            }
        }
    }
    info!("Source: {} file(s) totalling {} bytes and {} folder(s) => Dest: {} file(s) totalling {} bytes and {} folder(s)",
        stats.num_src_files, stats.src_total_bytes, stats.num_src_folders, 
        stats.num_dest_files, stats.dest_total_bytes, stats.num_dest_folders);


    // Delete dest entries that don't exist on the source. This needs to be done first in case there
    // are entries with the same name but different type (files vs folders).
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly also problems with files being filtered
    // so the folder is needed still as there are filtered-out files in there?)
    for dest_entry in dest_entries.iter().rev() {
        if !src_entries.iter().any(|f| f.path == dest_entry.path && f.entry_type == dest_entry.entry_type) {
            debug!("Deleting {}", dest_entry.path);
            let c = match dest_entry.entry_type {
                EntryType::File => Command::DeleteFile { path: dest_entry.path.to_string() },
                EntryType::Folder => Command::DeleteFolder { path: dest_entry.path.to_string() },
            };
            dest_comms.send_command(c).unwrap();
            match dest_comms.receive_response() {
                Ok(doer::Response::Ack) => (),
                _ => { 
                    error!("Wrong response");
                    return Err(());
                }
            };                   
        }
    }


    for src_entry in src_entries {
        match dest_entries.iter().find(|f| f.path == src_entry.path && f.entry_type == src_entry.entry_type) {
            Some(dest_entry) => {
                match src_entry.entry_type {
                    EntryType::File => {
                        match src_entry.modified_time.cmp(&dest_entry.modified_time) {
                            Ordering::Less => {
                                error!("{}: Dest file is newer - how did this happen!", src_entry.path);
                                return Err(());
                            }
                            Ordering::Equal => {
                                debug!("{}: Same modified time - skipping", src_entry.path);
                            }
                            Ordering::Greater => {
                                debug!("{}: source file newer - copying", src_entry.path);
                                copy_file(&src_entry, &mut src_comms, &mut dest_comms, &mut stats)?
                            }
                        }
                    },
                    EntryType::Folder => {
                        debug!("{}: folder already exists - nothing to do", src_entry.path)
                    }
                }        
            }
            None => {
                match src_entry.entry_type {
                    EntryType::File => {
                        debug!("{}: Dest file doesn't exist - copying", src_entry.path);
                        copy_file(&src_entry, &mut src_comms, &mut dest_comms, &mut stats)?
                    },
                    EntryType::Folder => {
                        debug!("{}: dest folder doesn't exists - creating", src_entry.path);
                        dest_comms.send_command(Command::CreateFolder { path: src_entry.path.to_string() }).unwrap();
                        match dest_comms.receive_response() {
                            Ok(doer::Response::Ack) => (),
                            _ => { 
                                error!("Wrong response");
                                return Err(());
                            }
                        };                    
                        stats.num_folders_created += 1;
                    }
                }
            }
        }
    }

    info!("Copied {} file(s) totalling {} bytes and created {} folder(s)", stats.num_files_copied, stats.num_bytes_copied, stats.num_folders_created);

    return Ok(());
}

fn copy_file(src_file: &EntryDetails, src_comms: &mut Comms, dest_comms: &mut Comms, stats: &mut Stats) -> Result<(), ()> {
    debug!("Fetching {}", src_file.path);
    src_comms.send_command(Command::GetFileContent { path: src_file.path.to_string() }).unwrap();
    let data = match src_comms.receive_response() {
        Ok(Response::FileContent { data }) => data,
        _ => { 
            error!("Wrong response");
            return Err(());
        }
    };
    debug!("Writing {}", src_file.path);
    dest_comms.send_command(Command::CreateOrUpdateFile { 
        path: src_file.path.to_string(), 
        data: data, 
        set_modified_time: Some(src_file.modified_time)
    }).unwrap();
    match dest_comms.receive_response() {
        Ok(doer::Response::Ack) => (),
        _ => { 
            error!("Wrong response");
            return Err(());
        }
    };

    stats.num_files_copied += 1;
    stats.num_bytes_copied += src_file.size;

    return Ok(());
}