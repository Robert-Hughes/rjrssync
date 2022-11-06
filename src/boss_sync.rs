use std::cmp::Ordering;

use log::{debug, warn, error};

use crate::*;

pub fn sync(src_folder: String, dest_folder: String, mut src_comms: Comms, mut dest_comms: Comms) -> Result<(), ()> {
    src_comms.send_command(Command::GetFileList { root: src_folder }).unwrap();
    dest_comms.send_command(Command::GetFileList { root: dest_folder }).unwrap();

    //TODO: create folders on the other side, even if there's nothing in them
    //TODO: delete files that don't exist on the source
    //TODO: delete folders that don't exist on the source
    //TODO: what about symlinks

    let mut src_files = Vec::new();
    loop {
        match src_comms.receive_response() {
            Ok(Response::FileListEntry(d)) => {
                debug!("{:?}", d);
                src_files.push(d);
            },
            Ok(Response::EndOfFileList) => break,
            r => {
                error!("Unexpected response: {:?}", r);
                return Err(());
            }
        }
    }
    let mut dest_files = Vec::new();
    loop {
        match dest_comms.receive_response() {
            Ok(Response::FileListEntry(d)) => {
                debug!("{:?}", d);
                dest_files.push(d);
            },
            Ok(Response::EndOfFileList) => break,
            r => {
                error!("Unexpected response: {:?}", r);
                return Err(());
            }
        }
    }
    debug!("Src files = {}, dest files = {}", src_files.len(), dest_files.len());

    // Delete dest files that don't exist on the source. This needs to be done first in case there
    // are files/folders with the same name but different type (files vs folders).
    // We do this in reverse to make sure that files are deleted before their parent folder
    // (otherwise deleting the parent is harder/more risky - possibly also problems with files being filtered
    // so the folder is needed still as there are filtered-out files in there?)
    for dest_file in dest_files.iter().rev() {
        if !src_files.iter().any(|f| f.path == dest_file.path && f.file_type == dest_file.file_type) {
            debug!("Deleting {}", dest_file.path);
            dest_comms.send_command(Command::DeleteFileOrFolder { path: dest_file.path.to_string() }).unwrap();
            match dest_comms.receive_response() {
                Ok(doer::Response::Ack) => (),
                _ => { 
                    error!("Wrong response");
                    return Err(());
                }
            };                   
        }
    }


    for src_file in src_files {
        if src_file.file_type != FileType::File {
            warn!("not supported folder");
            continue;
        }

        match dest_files.iter().find(|f| f.path == src_file.path && f.file_type == src_file.file_type) {
            Some(dest_file) => {
                match src_file.modified_time.cmp(&dest_file.modified_time) {
                    Ordering::Less => {
                        error!("{}: Dest file is newer - how did this happen!", src_file.path);
                        return Err(());
                    }
                    Ordering::Equal => {
                        debug!("{}: Same modified time - skipping", src_file.path);
                    }
                    Ordering::Greater => {
                        debug!("{}: source file newer - copying", src_file.path);
                        copy_file(&src_file.path, &mut src_comms, &mut dest_comms)?
                    }
                }
            }
            None => {
                debug!("Dest file doesn't exist - copying");
                copy_file(&src_file.path, &mut src_comms, &mut dest_comms)?
            }
        }
    }

    return Ok(());
}

fn copy_file(path: &str, src_comms: &mut Comms, dest_comms: &mut Comms) -> Result<(), ()> {
    debug!("Fetching {}", path);
    src_comms.send_command(Command::GetFileContent { path: path.to_string() }).unwrap();
    let data = match src_comms.receive_response() {
        Ok(Response::FileContent { data }) => data,
        _ => { 
            error!("Wrong response");
            return Err(());
        }
    };
    debug!("Writing {}", path);
    dest_comms.send_command(Command::CreateOrUpdateFile { path: path.to_string(), data: data }).unwrap();
    match dest_comms.receive_response() {
        Ok(doer::Response::Ack) => (),
        _ => { 
            error!("Wrong response");
            return Err(());
        }
    };

    return Ok(());
}