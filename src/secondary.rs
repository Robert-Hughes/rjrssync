use std::{io::{Write, Read, stdout, stdin}};
use clap::Parser;
use log::{info}; 

use crate::*;

#[derive(clap::Parser)]
struct SecondaryCliArgs {
    #[arg(short, long)]
    secondary: bool,
}

pub fn secondary_main() -> ExitCode {
    info!("Running as secondary");

    let _args = SecondaryCliArgs::parse();

    // We take commands from our stdin and send responses on our stdout. These will be piped over ssh
    // back to the Primary.

    // The first thing we send is a special message that the Primary will recognise, to know that we've started up correctly
    // and to make sure we are running compatible versions etc.
    // We need to do this on both stdout and stderr, because both those streams need to be synchronised on the receiving end.
    let msg = format!("{}{}", SECONDARY_HANDSHAKE_MSG, VERSION);
    println!("{}", msg);
    eprintln!("{}", msg);

    // If the Primary isn't happy, they will stop us and deploy a new version. So at this point we can assume
    // they are happy and move on to processing commands they (might) send us

    // Message-processing loop, until Primary disconnects.
    let mut buf: [u8; 1] = [0];
    while stdin().read(&mut buf).unwrap_or(0) > 0 {
      //  info!("Received data from Primary: {}", buf[0]);

        // echo back
        stdout().write(&buf).unwrap();

        //TODO: if get a message telling us to start a transfer, setup_comms(false) with the dest.
        //        (false cos the Dest should already have been set up with new version if necessary, so don't do it again)
        //TODO:     get a list of all the files in the local dir, and ask the dest to do the same
        //TODO:     compare the lists
        //TODO:     send over any files that have changed

        //TODO: if get a message telling us to provide a file list, do so

        //TODO: if get a message telling us to receive a file, do so
    }

   // info!("Primary disconnected!");
    return ExitCode::from(22);
}

fn secondary_thread_running_on_primary() {

}