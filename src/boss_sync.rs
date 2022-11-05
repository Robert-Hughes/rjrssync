use log::{debug};

use crate::*;

pub fn sync(src_folder: String, dest_folder: String, mut src_comms: Comms, mut dest_comms: Comms) -> Result<(), ()> {
    src_comms.send_command(Command::GetFiles { root: src_folder }).unwrap();
    dest_comms.send_command(Command::GetFiles { root: dest_folder }).unwrap();

    let mut num_files_src = 0;
    loop {
        let r = src_comms.receive_response();
        if let Ok(Response::File(s)) = r {
            debug!("{}", s);
            num_files_src = num_files_src + 1;
        } else {
            break;
        }
    }
    let mut num_files_dest = 0;
    loop {
        let r = dest_comms.receive_response();
        if let Ok(Response::File(s)) = r {
            debug!("{}", s);
            num_files_dest = num_files_dest + 1;
        } else {
            break;
        }
    }
    debug!("Src files = {}, dest files = {}", num_files_src, num_files_dest);

    return Ok(());
}