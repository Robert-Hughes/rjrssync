use std::{io::{BufReader, BufWriter, Read, Write}, time::Instant};

use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short='b', long)]
    buffer_size: i32,
    #[arg(short='i', long)]
    stdin_buffer_size: i32,
    #[arg(short='o', long)]
    stdout_buffer_size: i32,
    #[arg(short='q', long)]
    quiet: bool,
}

fn main() {
    let args = Args::parse();
    
    let mut stdin_reader = match args.stdin_buffer_size {
        x if x <= 0 => None,
        x => Some(BufReader::with_capacity(x as usize, std::io::stdin())),
    };
    let mut stdout_writer = match args.stdout_buffer_size {
        x if x <= 0 => None,
        x => Some(BufWriter::with_capacity(x as usize, std::io::stdout())),
    };

    let mut measure_start = Instant::now();
    let mut num_bytes_copied = 0;
    let mut buf = vec![0; args.buffer_size as usize];
    let mut measure_granularity = 1;
    loop {
        let num_bytes_in_buffer = match stdin_reader {
            Some(ref mut r) => {
                match r.read(&mut buf) {
                    Ok(x) if x > 0 => x,
                    _ => break,
                }
            },
            None => args.buffer_size as usize,
        };

        if let Some(ref mut w) = stdout_writer {
            w.write_all(&buf[0..num_bytes_in_buffer]).unwrap();
        }

        num_bytes_copied += num_bytes_in_buffer;
        // Getting time is slow, so only do this once we've copied a certain number of bytes.
        // This amount is adjusted dynamically based on the speed.
        if num_bytes_copied % measure_granularity == 0 && !args.quiet {
            let elapsed = Instant::now().duration_since(measure_start).as_secs_f32();            
            if elapsed > 1.0 {
                eprintln!("{}: {:.2}MB/s", std::process::id(), (num_bytes_copied as f32 / elapsed) / 1000000.0);
                num_bytes_copied = 0;
                measure_start = Instant::now();
            }

            if elapsed > 2.0 {
                measure_granularity = std::cmp::max(measure_granularity / 2, 1);
            } else if elapsed < 0.5 {
                measure_granularity *= 2;
            }
        }
    }
}