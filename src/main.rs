use std::process::ExitCode;

mod boss;
mod boss_sync;
mod doer;

use boss::*;
use doer::*;

pub const VERSION: i32 = 24;

// Message printed by a doer copy of the program to indicate that it has loaded and is ready
// to receive commands over its stdin. Also identifies its version, so the boss side can decide
// if it can continue to communicate or needs to copy over an updated copy of the doer program.
// Note that this format needs to always be backwards-compatible, so is very basic.
pub const HANDSHAKE_MSG : &str = "rjrssync doer v"; // Version number will be appended

pub const REMOTE_TEMP_FOLDER : &str = "/tmp/rjrssync/";

fn main() -> ExitCode {
    // The process can run as either a CLI which takes input from command line arguments, performs
    // a transfer and then exits once complete ("boss"), or as a remote process on either the source
    // or destination computer which responds to commands from the boss (this is a "doer").
    // The boss (CLI) and doer modes have different command-line arguments, so handle them separately.
    if std::env::args().any(|a| a == "--doer") {
        return doer_main();
    } else {
        return boss_main();
    }
}

