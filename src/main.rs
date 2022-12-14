use std::process::ExitCode;
use const_format::concatcp;

mod boss_frontend;
mod boss_launch;
mod boss_sync;
mod doer;
mod encrypted_comms;
mod profiling;

use boss_frontend::*;
use boss_launch::*;
use doer::*;
use profiling::*;

// We include the profiling config in the version number, as profiling and non-profiling builds are not compatible
// (both because the Command struct is different and because a non-profiling doer won't record any events).
pub const VERSION: &str = concatcp!("76", if cfg!(feature="profiling") { "+profiling"} else { "" });

// Message printed by a doer copy of the program to indicate that it has loaded and is ready
// to receive data over its stdin. Once the boss receives this, it knows that ssh has connected
// correctly etc. It also identifies its version, so the boss side can decide
// if it can continue to communicate or needs to copy over an updated copy of the doer program.
// Note that this format needs to always be backwards-compatible, so is very basic.
pub const HANDSHAKE_STARTED_MSG: &str = "rjrssync doer v"; // Version number will be appended

// Message sent by the doer back to the boss to indicate that it has received the secret key and
// is listening on a network port for a connection.
pub const HANDSHAKE_COMPLETED_MSG: &str = "Waiting for incoming network connection on port "; // Port number will be appended.

pub const REMOTE_TEMP_UNIX: &str = "/tmp/";
pub const REMOTE_TEMP_WINDOWS: &str = r"%TEMP%\";

fn main() -> ExitCode {
    // The process can run as either a CLI which takes input from command line arguments, performs
    // a transfer and then exits once complete ("boss"), or as a remote process on either the source
    // or destination computer which responds to commands from the boss (this is a "doer").
    // The boss (CLI) and doer modes have different command-line arguments, so handle them separately.
    if std::env::args().any(|a| a == "--doer") {
        doer_main()
    } else {
        boss_main()
    }
}
