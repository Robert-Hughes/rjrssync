use std::{io::{BufReader, BufWriter, Read, Write}, time::Instant, net::{TcpListener, TcpStream}};

use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_encrypt::{traits::SerdeEncryptSharedKey, serialize::impls::BincodeSerializer, shared_key::SharedKey, EncryptedMessage};

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

#[derive(Serialize, Deserialize)]
struct Payload {
    data: Vec<u8>,
}
impl SerdeEncryptSharedKey for Payload {
    type S = BincodeSerializer<Self>;
}


fn main() {
    let args = Args::parse();

    let shared_key = SharedKey::new([7u8; 32]);

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
                w.write(&buf[0..num_bytes_in_buffer]).unwrap();
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
                    measure_granularity = measure_granularity * 2;
                }
            }
        }
    } else {
        loop {
            let mut unencrypted_data = vec![0 as u8; args.buffer_size as usize];
            if let Some(ref mut r) = reader {
                let mut buf = [0 as u8; 8];            
                r.read_exact(&mut buf).unwrap();
                let encrypted_len = usize::from_le_bytes(buf);

                let mut buf = vec![0 as u8; encrypted_len];
                r.read_exact(&mut buf).unwrap();
                let enc_msg = EncryptedMessage::deserialize(buf).unwrap();
                let payload = Payload::decrypt_owned(&enc_msg, &shared_key).unwrap();
                unencrypted_data = payload.data;
            }
    
            num_bytes_copied += unencrypted_data.len();

            if let Some(ref mut w) = writer {
                let payload = Payload { data: unencrypted_data };
                let enc_msg = payload.encrypt(&shared_key).unwrap();
                let buf = enc_msg.serialize();
                w.write(&buf.len().to_le_bytes()).unwrap();
                w.write(&buf).unwrap();
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
                    measure_granularity = measure_granularity * 2;
                }
            }
        }
    }
}