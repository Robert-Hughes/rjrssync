use std::{io::{BufReader, BufWriter, Read, Write}, time::Instant, net::{TcpListener, TcpStream}};

use clap::Parser;
use aes_gcm::{
    aead::{Aead, KeyInit, generic_array::GenericArray},
    Aes128Gcm, Nonce
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short='b', long)]
    buffer_size: i32,
    #[arg(short='s', long)]
    stream_buffer_size: i32,
    #[arg(short='l', long)]
    listen: bool,
    address: String,
    #[arg(short='e', long)]
    encrypted: bool,
}

fn main() {
    let args = Args::parse();

    //let shared_key = Aes128Gcm::generate_key(&mut OsRng);
    let shared_key = GenericArray::from_slice(b"secret key.12345");
    let cipher = Aes128Gcm::new(shared_key);
    let nonce = Nonce::from_slice(b"unique nonce"); // 96-bits; unique per message

    let mut reader = None;
    let mut writer = None;
    if args.listen {
        let listener = TcpListener::bind(args.address).unwrap();
        let stream = listener.accept().unwrap().0;

        reader = Some(BufReader::with_capacity(args.stream_buffer_size as usize, stream));
        println!("Accepted connection!");
    } else {
        let stream = TcpStream::connect(args.address).unwrap();
        writer = Some(BufWriter::with_capacity(args.stream_buffer_size as usize, stream));
    }
    
    let mut measure_start = Instant::now();
    let mut num_bytes_copied = 0;
    let mut measure_granularity = 1;
    
    if !args.encrypted {
        let mut buf = vec![0; args.buffer_size as usize];
        loop {
            let num_bytes_in_buffer = match reader {
                Some(ref mut r) => {
                    match r.read(&mut buf) {
                        Ok(x) if x > 0 => x,
                        _ => break,
                    }
                },
                None => args.buffer_size as usize,
            };

            if let Some(ref mut w) = writer {
                w.write_all(&buf[0..num_bytes_in_buffer]).unwrap();
            }

            num_bytes_copied += num_bytes_in_buffer;
            // Getting time is slow, so only do this once we've copied a certain number of bytes.
            // This amount is adjusted dynamically based on the speed.
            if num_bytes_copied >= measure_granularity {
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
    } else {
        loop {
            let mut unencrypted_data = vec![0_u8; args.buffer_size as usize];
            if let Some(ref mut r) = reader {
                let mut buf = [0_u8; 8];            
                r.read_exact(&mut buf).unwrap();
                let encrypted_len = usize::from_le_bytes(buf);

                let mut buf = vec![0_u8; encrypted_len];
                r.read_exact(&mut buf).unwrap();
                unencrypted_data = cipher.decrypt(nonce, buf.as_ref()).unwrap();
            }
    
            num_bytes_copied += unencrypted_data.len();

            if let Some(ref mut w) = writer {
                let ciphertext = cipher.encrypt(nonce, unencrypted_data.as_ref()).unwrap();

                w.write_all(&ciphertext.len().to_le_bytes()).unwrap();
                w.write_all(&ciphertext).unwrap();
            }
    
            // Getting time is slow, so only do this once we've copied a certain number of bytes.
            // This amount is adjusted dynamically based on the speed.
            if num_bytes_copied >= measure_granularity {
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
}