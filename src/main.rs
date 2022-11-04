use std::{process::{ExitCode}};

mod primary;
mod secondary;

use primary::*;
use secondary::*;

pub const VERSION: i32 = 5;

fn main() -> ExitCode {
    // The process can run as either a CLI which takes input from the command line, performs
    // a transfer and then exits once complete ("primary"), or as a remote process on either the source
    // or destination computer which responds to commands from the primary (this is a "secondary").
    // The primary (CLI) and secondary modes have different command-line arguments, so handle them separately.
    if std::env::args().any(|a| a == "--secondary") {
        return secondary_main();
    } else {
        return primary_main();
    }
}

// Message printed by a secondary copy of the program to indicate that it has loaded and is ready
// to receive commands over its stdin. Also identifies its version, so the primary side can decide
// if it can continue to communicate or needs to copy over an updated copy of the secondary program.
// Note that this format needs to always be backwards-compatible, so is very basic.
pub const SECONDARY_HANDSHAKE_MSG : &str = "rjrssync secondary v"; // Version number will be appended
