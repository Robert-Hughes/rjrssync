use log::info;

use crate::*;

pub fn sync(src_folder: String, dest_folder: String, mut src_comms: Comms, mut dest_comms: Comms) -> Result<(), ()> {

    src_comms.send_command(Command::GetFiles { root: src_folder }).unwrap();
    dest_comms.send_command(Command::GetFiles { root: dest_folder }).unwrap();

    loop {
        let r = src_comms.receive_response();
        if let Ok(Response::File(s)) = r {
            info!("{}", s);
        } else {
            break;
        }
    }
    loop {
        let r = dest_comms.receive_response();
        if let Ok(Response::File(s)) = r {
            info!("{}", s);
        } else {
            break;
        }
    }

    return Ok(());
}