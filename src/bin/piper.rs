use std::{io::{BufReader, BufWriter, Read, Write}, time::Instant};

use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short='i', long)]
    stdin_buffer_size: i32,
    #[arg(short='o', long)]
    stdout_buffer_size: i32,
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
    let mut measure_count = 0;
    loop {
        let x = match stdin_reader {
            Some(ref mut r) => {
                let mut buf = [0];
                match r.read(&mut buf) {
                    Ok(x) if x > 0 => (),
                    _ => break,
                }
                buf[0]
            },
            None => 'x' as u8,
        };

        if let Some(ref mut w) = stdout_writer {
            let buf = [x];
            w.write(&buf).unwrap();
        }

        measure_count += 1;
        if measure_count % 1000 == 0 {
            let elapsed = Instant::now().duration_since(measure_start).as_secs_f32();
            if elapsed > 1.0 {
                eprintln!("{}: {:.2}MB/s", std::process::id(), (measure_count as f32 / elapsed) / 1000000.0);
                measure_count = 0;
                measure_start = Instant::now();
            }
        }
    }
}