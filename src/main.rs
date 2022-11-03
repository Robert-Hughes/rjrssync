use std::{io::{Write, Read, stdout, stdin, BufReader, BufRead}, path::PathBuf, process::{Stdio, ExitCode, ChildStdout, ChildStdin, ChildStderr}, sync::mpsc::RecvError, fmt::{Display, self}};
use std::sync::mpsc;
use std::sync::mpsc::{Sender, Receiver};
use clap::Parser;
use log::{info, error, warn};
use rust_embed::RustEmbed;
use tempdir::TempDir;
use std::process::{Command};

const VERSION: i32 = 1;

fn main() -> ExitCode {
    // Configure logging
    simple_logger::SimpleLogger::new().env().init().unwrap();

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
const SECONDARY_HANDSHAKE_MSG : &str = "rjrssync secondary v"; // Version number will be appended



// use clap::Parser;
// use walkdir::WalkDir;
// use std::time::Instant;

// #[derive(Parser)]
// struct Cli {
//     path: std::path::PathBuf,
// }

// fn main() {
//     let args = Cli::parse();
//     {
//         let start = Instant::now();
//         let walker = WalkDir::new(&args.path).into_iter();
//         let mut count = 0;
//         for _entry in walker.filter_entry(|e| e.file_name() != ".git" && e.file_name() != "dependencies") {
//             count += 1;
//         }
//         let elapsed = start.elapsed().as_millis();
//         println!("Walked {} in {} ({}/s)", count, elapsed, 1000.0 * count as f32 / elapsed as f32);
//     }

//     {
//         let start = Instant::now();
//         let walker = WalkDir::new(&args.path).into_iter();
//         let mut hash_sum: u8 = 0;        
//         let mut count = 0;
//         for entry in walker.filter_entry(|e| e.file_name() != ".git" && e.file_name() != "dependencies") {
//             let e = entry.unwrap();
//             if e.file_type().is_file() {
//                 let bytes = std::fs::read(e.path()).unwrap();
//                 let hash = md5::compute(&bytes);
//                 hash_sum += hash.into_iter().sum::<u8>();
//                 count += 1;
//             }
//         }
//         let elapsed = start.elapsed().as_millis();
//         println!("Hashed {} ({}) in {} ({}/s)", count, hash_sum, elapsed, 1000.0 * count as f32 / elapsed as f32);
//     }

// }

//  Host:           Windows     Linux
//  Filesystem:
//    Windows        100k        9k
//     Linux          1k         500k