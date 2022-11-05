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
        let r = src_comms.receive_response();
        if let Ok(Response::FileListEntry(d)) = r {
            debug!("{:?}", d);
            src_files.push(d);
        } else {
            break;
        }
    }
    let mut dest_files = Vec::new();
    loop {
        let r = dest_comms.receive_response();
        if let Ok(Response::FileListEntry(d)) = r {
            debug!("{:?}", d);
            dest_files.push(d);
        } else {
            break;
        }
    }
    debug!("Src files = {}, dest files = {}", src_files.len(), dest_files.len());

    for src_file in src_files {
        if src_file.file_type != FileType::File {
            continue;
        }

        if dest_files.iter().any(|f| *f == src_file) {
            warn!("{} is on both sides, not copying", src_file.path);
            //TODO: compare timestamp/size etc. to decide if need to copy
        } else {
            debug!("Fetching {}", src_file.path);
            src_comms.send_command(Command::GetFileContent { path: src_file.path.clone() }).unwrap();
            let data = match src_comms.receive_response() {
                Ok(Response::FileContent { data }) => data,
                _ => { 
                    error!("Wrong response");
                    return Err(());
                }
            };
            debug!("Writing {}", src_file.path);
            dest_comms.send_command(Command::CreateOrUpdateFile { path: src_file.path, data: data }).unwrap();
            match dest_comms.receive_response() {
                Ok(doer::Response::Ack) => (),
                _ => { 
                    error!("Wrong response");
                    return Err(());
                }
            };        
        }
    }

    return Ok(());
}