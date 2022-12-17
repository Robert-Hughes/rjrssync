use std::{net::TcpStream, io::{Write, Read}, thread::{JoinHandle, self}, sync::mpsc::{Sender, Receiver, self}};

use aead::{Key, KeyInit};
use bytes::{BytesMut, BufMut};
use aes_gcm::{Aes128Gcm, aead::{Nonce, Aead}, AeadInPlace, AesGcm};
use log::{error, trace};
use serde::{Deserialize, Serialize};

use crate::profile_this;

/// Provides asynchronous, encrypted communication over a TcpStream, sending messages of type S
/// and receiving messages of type R.
/// A background thread is spawned for each sending and receiving, and a cross-thread channel is used
/// for communication with these threads. These channels are the public interface to this object.
/// The channels are buffered and so new messages can be queued
/// up for sending instantly, even if a previous message is still being encrypted or the network
/// is blocking. Similary, received messages don't need to be retrieved immediately as the background
/// thread will keep receiving and decrypting messages and storing them in the channel for later processing.
pub struct AsyncEncryptedComms<S: Serialize, R: for<'a> Deserialize<'a>> {
    tcp_connection: TcpStream,

    sending_thread: JoinHandle<(Aes128Gcm, u64, u64)>,
    pub sender: Sender<S>,

    receiving_thread: JoinHandle<()>,
    pub receiver: Receiver<R>,
}
impl<S: Serialize + Send + 'static, R: for<'a> Deserialize<'a> + Send + 'static> AsyncEncryptedComms<S, R> {
    pub fn new(mut tcp_connection: TcpStream, secret_key: Key<Aes128Gcm>, sending_nonce_lsb: u64, receiving_nonce_lsb: u64,
        debug_name: &str) -> AsyncEncryptedComms<S, R> 
    {
        let mut tcp_connection_clone1 = tcp_connection.try_clone().expect("Failed to clone TCP stream");
        let mut tcp_connection_clone2 = tcp_connection.try_clone().expect("Failed to clone TCP stream");

        let (sender, thread_receiver) = mpsc::channel();
        let sending_thread = thread::Builder::new()
            .name(format!("Sending to {debug_name}"))
            .spawn(move || {
                let mut sending_nonce_counter = sending_nonce_lsb;
                let cipher = Aes128Gcm::new(&secret_key);
                loop {
                    let s = match thread_receiver.recv() {
                        Ok(s) => s,
                        Err(_) => {
                            trace!("{}: shutting down due to closed channel", std::thread::current().name().unwrap());
                            break; // The sender must have been dropped, so stop this background thread
                        }
                    };
                    if let Err(e) = send(s, &mut tcp_connection_clone1, &cipher, &mut sending_nonce_counter, sending_nonce_lsb) {
                        //TODO: handle errors
                        trace!("{}: shutting down due to error sending on TCP", std::thread::current().name().unwrap());
                        break;
                    }                    
                }

                (cipher, sending_nonce_counter, sending_nonce_lsb)
            }).expect("Failed to spawn thread");

        let (thread_sender, receiver) = mpsc::channel();
        let receiving_thread = thread::Builder::new()
            .name(format!("Receiving from {debug_name}"))
            .spawn(move || {
                let mut receiving_nonce_counter = receiving_nonce_lsb;
                let cipher = Aes128Gcm::new(&secret_key);
                loop {
                    let r = receive(&mut tcp_connection_clone2, &cipher, &mut receiving_nonce_counter, receiving_nonce_lsb);
                    if let Err(e) = r {
                        //TODO: handle errors
                        trace!("{}: shutting down due to error receiving from TCP", std::thread::current().name().unwrap());
                        break;
                    }
                    let r = r.expect("oh dear");
                    if thread_sender.send(r).is_err() {
                        trace!("{}: shutting down due to closed channel", std::thread::current().name().unwrap());
                        break; // The sender must have been dropped, so stop this background thread
                    };
                }
            }).expect("Failed to spawn thread");

        AsyncEncryptedComms { tcp_connection, sending_thread, sender, receiving_thread, receiver }
    }

    pub fn shutdown_with_final_message_sent_after_threads_joined<F: FnOnce() -> S>(mut self, chance_to_send_final_message_after_threads_joined: F) {
        trace!("AsyncEncryptedComms::shutdown_with_send_final_message_after_threads_joined");
        // Join the threads so that their profiling data is captured
        // Drop the channels first so that the thread will break out of its loop
        drop(self.sender);
        trace!("Waiting for sending thread");
        let (cipher, mut sending_nonce_counter, sending_nonce_lsb) = self.sending_thread.join().unwrap();

        drop(self.receiver);
        trace!("Waiting for receiving thread");
        self.receiving_thread.join();

        trace!("Closing TCP read");
        self.tcp_connection.shutdown(std::net::Shutdown::Read);

        // Chance to send a final message once threads dropped, i.e. profiling (needs to happen after all
        // threads joined)
        let s = chance_to_send_final_message_after_threads_joined();
        trace!("Sending final mesage");
        send(s, &mut self.tcp_connection, &cipher, &mut sending_nonce_counter, sending_nonce_lsb);
        //TODO: check error?

        trace!("Closing TCP write");
        self.tcp_connection.shutdown(std::net::Shutdown::Write);
    }

    pub fn shutdown_with_final_message_received_after_closing_send(mut self) -> R {
        trace!("AsyncEncryptedComms::shutdown_with_final_message_received_after_closing_send");

        drop(self.sender);
        trace!("Waiting for sending thread");
        let (cipher, mut sending_nonce_counter, sending_nonce_lsb) = self.sending_thread.join().unwrap();
        trace!("Closing TCP write");
        self.tcp_connection.shutdown(std::net::Shutdown::Write);

        let r = self.receiver.recv().expect("Failed to receive final message");

        drop(self.receiver);
        trace!("Waiting for receiving thread");
        self.receiving_thread.join();

        trace!("Closing TCP read");
        self.tcp_connection.shutdown(std::net::Shutdown::Read);

        r
    }
}

fn send<T>(x: T, tcp_connection: &mut TcpStream, cipher: &Aes128Gcm,
    sending_nonce_counter: &mut u64, nonce_lsb: u64) -> Result<(), String>
    where T : Serialize,
{
    profile_this!();
    // Put the cipher and the length into a single buffer so we only do 1 tcp write.
    // Allocate the size of the cipher + 8 bytes (for the cipher length at the start)
    // Serialize into expands the buffer so with_capabity is used to reserve the buffer
    // Allocate 1000 bytes extra to try and minimize allocations
    let mut buffer = BytesMut::with_capacity((bincode::serialized_size(&x).unwrap() + 1008) as usize);
    buffer.put_u64_le(0);
    let mut cipher_len = BytesMut::split_to(&mut buffer, 8);
    let mut writer = buffer.writer();
    {
        profile_this!("Serialize");
        bincode::serialize_into(&mut writer, &x).map_err(|e| "Error serializing command: ".to_string() + &e.to_string())?;
    }
    let mut buffer = writer.into_inner();

    // Nonces for boss -> doer should always be even, and odd for vice versa. They can't be reused between them.
    assert!(*sending_nonce_counter % 2 == nonce_lsb);
    let mut nonce_bytes = sending_nonce_counter.to_le_bytes().to_vec();
    nonce_bytes.resize(12, 0); // pad it
    let nonce = Nonce::<Aes128Gcm>::from_slice(&nonce_bytes);
    sending_nonce_counter.checked_add(2).unwrap(); // Increment by two so that it never overlaps with the nonce used by the doer
    {
        profile_this!("Encrypt");
        cipher.encrypt_in_place(nonce, &[], &mut buffer).unwrap();
    }
    cipher_len.copy_from_slice(&buffer.len().to_le_bytes());
    cipher_len.unsplit(buffer);

    {
        profile_this!("Tcp Write");
        tcp_connection.write_all(&cipher_len).map_err(|e| "Error sending length: ".to_string() + &e.to_string())?;

        // Flush to make sure that we don't deadlock (other side waiting for data that never comes)
        tcp_connection.flush().map_err(|e| "Error flushing: ".to_string() + &e.to_string())?;
    }

    Ok(())
}

fn receive<T>(tcp_connection: &mut TcpStream, cipher: &Aes128Gcm,
    receiving_nonce_counter: &mut u64, nonce_lsb: u64) -> Result<T, String>
    where T : for<'a> Deserialize<'a>
{
    profile_this!();

    let encrypted_data = {
        profile_this!("Tcp Read");

        let mut len_buf = [0_u8; 8];
        tcp_connection.read_exact(&mut len_buf).map_err(|e| "Error reading len: ".to_string() + &e.to_string())?;
        let encrypted_len = usize::from_le_bytes(len_buf);

        let mut encrypted_data = vec![0_u8; encrypted_len];
        tcp_connection.read_exact(&mut encrypted_data).map_err(|e| "Error reading encrypted data: ".to_string() + &e.to_string())?;
        encrypted_data
    };

    // Nonces for doer -> boss should always be odd, and even for vice versa. They can't be reused between them.
    assert!(*receiving_nonce_counter % 2 == nonce_lsb);
    let mut nonce_bytes = receiving_nonce_counter.to_le_bytes().to_vec();
    nonce_bytes.resize(12, 0); // pad it
    let nonce = Nonce::<Aes128Gcm>::from_slice(&nonce_bytes);
    receiving_nonce_counter.checked_add(2).unwrap(); // Increment by two so that it never overlaps with the nonce used by the boss

    let unencrypted_data = {
        profile_this!("Decrypt");
        cipher.decrypt(nonce, encrypted_data.as_ref()).map_err(|e| "Error decrypting: ".to_string() + &e.to_string())?
    };

    let response = {
        profile_this!("Deserialize");
        bincode::deserialize(&unencrypted_data).map_err(|e| "Error deserializing: ".to_string() + &e.to_string())?
    };

    Ok(response)
}
