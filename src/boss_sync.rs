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
    //TODO: if a file/folder exists already but we need to make the opposite kind (replace file with folder etc.)
    // then what happens?

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

    for src_file in src_files {
        if src_file.file_type != FileType::File {
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