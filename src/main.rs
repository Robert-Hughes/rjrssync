use std::process::ExitCode;

mod boss_frontend;
mod boss_launch;
mod boss_deploy;
mod embedded_binaries;
mod exe_utils;
mod boss_sync;
mod ordered_map;
mod histogram;
mod boss_progress;
mod doer;
mod boss_doer_interface;
mod root_relative_path;
mod encrypted_comms;
mod memory_bound_channel;
mod profiling;
mod parallel_walk_dir;

use boss_frontend::*;
use boss_launch::*;
use doer::*;
use profiling::*;

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
