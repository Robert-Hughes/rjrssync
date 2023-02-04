use std::{net::TcpStream, io::{Write, Read}, thread::{JoinHandle, self}, fmt::{Display, Debug}};

use aead::{Key, KeyInit};
use aes_gcm::{Aes128Gcm, aead::{Nonce}, AeadInPlace};
use log::{trace, error};
use serde::{Deserialize, Serialize};

use crate::{profile_this, memory_bound_channel::{Sender, Receiver, self}, BOSS_DOER_CHANNEL_MEMORY_CAPACITY};

//TODO: tidy up the crappy SliceBuffer stuff

pub trait IsFinalMessage {
    fn is_final_message(&self) -> bool;
}

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

    sending_thread: JoinHandle<Result<(Aes128Gcm, u64, u64, Vec<u8>), String>>,
    pub sender: Sender<S>,

    receiving_thread: JoinHandle<Result<(), String>>,
    pub receiver: Receiver<R>,
}
impl<S: Serialize + Send + 'static + Debug, R: for<'a> Deserialize<'a> + Serialize + Send + 'static + Debug + IsFinalMessage> AsyncEncryptedComms<S, R> {
    pub fn new(tcp_connection: TcpStream, secret_key: Key<Aes128Gcm>, sending_nonce_lsb: u64, receiving_nonce_lsb: u64,
        debug_local_remote_name: (&str, &str)) -> AsyncEncryptedComms<S, R>
    {
        let mut tcp_connection_clone1 = tcp_connection.try_clone().expect("Failed to clone TCP stream");
        let mut tcp_connection_clone2 = tcp_connection.try_clone().expect("Failed to clone TCP stream");

        let sending_thread_name = format!("{} -> {}", debug_local_remote_name.0, debug_local_remote_name.1);
        let (sender, thread_receiver) = memory_bound_channel::new(BOSS_DOER_CHANNEL_MEMORY_CAPACITY);
        let sending_thread = thread::Builder::new()
            .name(sending_thread_name.clone())
            .spawn(move || {
                let mut sending_nonce_counter = sending_nonce_lsb;
                let cipher = Aes128Gcm::new(&secret_key);
                // Allocate a buffer up front, to be used for all serialization, encryption etc. on this thread.
                // This avoids having to allocate new buffers each time we send a message, which should give better performance.
                // 8MB should be plenty, as the max message size should be 4MB (max chunk size of a file)
                let mut buffer = vec![0u8; 8192 * 1024];
                loop {
                    let s = match thread_receiver.recv() {
                        Ok(s) => s,
                        Err(_) => {
                            // The sender on the main thread has been dropped, which means that there are no more messages to send,
                            // so we finish this background thread successfully (this is the expected clean shutdown process)
                            trace!("Sending thread '{sending_thread_name}' shutting down due to closed channel");
                            // Return stuff needed to send one more message from the main thread (needed for profiling)
                            return Ok((cipher, sending_nonce_counter, sending_nonce_lsb, buffer));
                        }
                    };
                    if let Err(e) = send(s, &mut tcp_connection_clone1, &cipher, &mut sending_nonce_counter,
                        sending_nonce_lsb, &mut buffer) {
                        // There was an error sending a message, which shouldn't happen in normal operation.
                        // Stop this background thread, which will close the receiving side of the
                        // channel. The main thread will detect this as a closed channel.
                        // Note we don't log this as an error, as we leave this up to the main thread to report when this thread
                        // gets joined, otherwise we might report the error at an inappropriate time, e.g. if the sync is cancelled due
                        // to an error and the connection drops, we don't want to report the connection dropping as well as the original error.
                        trace!("Sending thread '{sending_thread_name}' shutting down due to error sending on TCP: {e}");
                        return Err(e);
                    }
                }
            }).expect("Failed to spawn thread");

        let receiving_thread_name = format!("{} -> {}", debug_local_remote_name.1, debug_local_remote_name.0);
        let (thread_sender, receiver) = memory_bound_channel::new(BOSS_DOER_CHANNEL_MEMORY_CAPACITY);
        let receiving_thread = thread::Builder::new()
            .name(receiving_thread_name.clone())
            .spawn(move || {
                let mut receiving_nonce_counter = receiving_nonce_lsb;
                let cipher = Aes128Gcm::new(&secret_key);
                // Allocate a buffer up front, to be used for all serialization, encryption etc. on this thread.
                // This avoids having to allocate new buffers each time we send a message, which should give better performance.
                // 8MB should be plenty, as the max message size should be 4MB (max chunk size of a file)
                let mut buffer = vec![0u8; 8192 * 1024];
                loop {
                    let r: R = match receive(&mut tcp_connection_clone2, &cipher, &mut receiving_nonce_counter,
                        receiving_nonce_lsb, &mut buffer) {
                        Ok(r) => r,
                        Err(e) => {
                            // There was an error receiving a message, which shouldn't happen in normal operation.
                            // Log an error, and stop this background thread, which will close the sending side of the
                            // channel. The main thread will detect this as a closed channel.
                            // Note we don't log this as an error, as we leave this up to the main thread to report when this thread
                            // gets joined, otherwise we might report the error at an inappropriate time, e.g. if the sync is cancelled due
                            // to an error and the connection drops, we don't want to report the connection dropping as well as the original error.
                            trace!("Receiving thread '{receiving_thread_name}' shutting down due to error receiving from TCP: {e}");
                            return Err(e);
                        }
                    };
                    let is_final_message = r.is_final_message();
                    if thread_sender.send(r).is_err() {
                        // The main thread receiver has been dropped, which shouldn't happen during normal operation
                        // Note we don't log this as an error, as we leave this up to the main thread to report when this thread
                        // gets joined, otherwise we might report the error at an inappropriate time, e.g. if the sync is cancelled due
                        // to an error and the connection drops, we don't want to report the connection dropping as well as the original error.
                        trace!("Receiving thread '{receiving_thread_name}' shutting down due to closed channel");
                        return Err("Communications with main thread broken".to_string());
                    };
                    // Stop this thread cleanly if that was the final message
                    if is_final_message {
                        trace!("Receiving thread '{receiving_thread_name}' shutting down due to receiving final message");
                        return Ok(());
                    }
                }
            }).expect("Failed to spawn thread");

        AsyncEncryptedComms { tcp_connection, sending_thread, sender, receiving_thread, receiver }
    }

    /// Clean shutdown which joins the background threads, making sure all messages are flushed etc.
    /// Prefer this to simply dropping the object, which will leave the threads to exit on their own.
    pub fn shutdown(self) {
        trace!("AsyncEncryptedComms::shutdown");
        // The order here is important, so that both sides of the conenction exit cleanly and we don't deadlock.
        // I had some issues with windows -> remote linux when shutting down the writing half of the connection
        // from windows. The new approach of stopping the receiving thread using IsFinalMessage seems to be working better.

        // Stop the sending thread, which will be blocked on the channel waiting for a new message to send.
        drop(self.sender);
        trace!("Waiting for sending thread");
        join_with_err_log(self.sending_thread);

        // The receiving thread should already have been stopped once it saw the final message
        trace!("Waiting for receiving thread");
        join_with_err_log(self.receiving_thread);
    }

    /// Clean shutdown which joins the background threads, making sure all messages are flushed etc.
    /// Prefer this to simply dropping the object, which will leave the threads to exit on their own.
    /// This version of shutdown provides an opportunity for the caller to generate and send
    /// one final message _after_ both background threads have finished. This is used for profiling,
    /// as the profiling data is only flushed from the thread-local storage once these threads have finished.
    pub fn shutdown_with_final_message_sent_after_threads_joined<F: FnOnce() -> S>(mut self, message_generating_func: F) {
        trace!("AsyncEncryptedComms::shutdown_with_send_final_message_after_threads_joined");
        // The order here is important, so that both sides of the conenction exit cleanly and we don't deadlock.
        // I had some issues with windows -> remote linux when shutting down the writing half of the connection
        // from windows. The new approach of stopping the receiving thread using IsFinalMessage seems to be working better.

        // Stop the sending thread, which will be blocked on the channel waiting for a new message to send,
        // and retrieve the cipher etc. needed to send one more final message
        drop(self.sender);
        trace!("Waiting for sending thread");
        let sending_thread_result = join_with_err_log(self.sending_thread);

        // The receiving thread should already have been stopped once it saw the final message
        trace!("Waiting for receiving thread");
        join_with_err_log(self.receiving_thread);

        if let Some((cipher, mut sending_nonce_counter, sending_nonce_lsb, mut buffer)) = sending_thread_result {
            let s = message_generating_func();

            trace!("Sending final mesage {:?}", s);
            // There's not much we can do with an error here, as we're closing everything down anyway
            if let Err(e) = send(s, &mut self.tcp_connection, &cipher, &mut sending_nonce_counter,
                sending_nonce_lsb, &mut buffer) {
                error!("Error sending final message: {e}");
            }
        } else {
            error!("Unable to send final message as sending thread didn't complete successfully");
        }
    }
}

/// Helper func to join on a thread that returns a Result<T>, and log any errors
fn join_with_err_log<T, E: Display>(t: JoinHandle<Result<T, E>>) -> Option<T> {
    let name = t.thread().name().expect("Failed to get thread name").to_string();
    match t.join().expect(&format!("Failed to join thread '{}'", name)) {
        Ok(x) => Some(x),
        Err(e) => {
            error!("Thread '{name}' failed with error: {e}");
            None
        }
    }
}

fn send<T>(x: T, tcp_connection: &mut TcpStream, cipher: &Aes128Gcm,
    sending_nonce_counter: &mut u64, nonce_lsb: u64, mut buffer: &mut [u8]) -> Result<(), String>
    where T : Serialize,
{
    profile_this!();

    let unencrypted_len = {
        profile_this!("Serialize");
        let l = buffer.len();
        let mut s = SliceBuffer { slice : &mut buffer, new_len: l, write_offset: 8 };
        bincode::serialize_into(&mut s, &x).map_err(|e| "Error serializing command: ".to_string() + &e.to_string())?;
        s.write_offset - 8
    };

    // Nonces for boss -> doer should always be even, and odd for vice versa. They can't be reused between them.
    assert!(*sending_nonce_counter % 2 == nonce_lsb);
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[0..8].copy_from_slice(&sending_nonce_counter.to_le_bytes());
    let nonce = Nonce::<Aes128Gcm>::from_slice(&nonce_bytes);
    sending_nonce_counter.checked_add(2).unwrap(); // Increment by two so that it never overlaps with the nonce used by the doer
    let encrypted_len = {
        profile_this!("Encrypt");
        let mut s = SliceBuffer { slice : &mut buffer[8..], new_len: unencrypted_len, write_offset: 0 };
        cipher.encrypt_in_place(nonce, &[], &mut s).unwrap();
        s.new_len
    };
    buffer[0..8].copy_from_slice(&encrypted_len.to_le_bytes());

    {
        profile_this!("Tcp Write");
        tcp_connection.write_all(&buffer[0..8+encrypted_len]).map_err(|e| "Error sending length: ".to_string() + &e.to_string())?;

        // Flush to make sure that we don't deadlock (other side waiting for data that never comes)
        tcp_connection.flush().map_err(|e| "Error flushing: ".to_string() + &e.to_string())?;
    }

    Ok(())
}

fn receive<T>(tcp_connection: &mut TcpStream, cipher: &Aes128Gcm,
    receiving_nonce_counter: &mut u64, nonce_lsb: u64, buffer: &mut [u8]) -> Result<T, String>
    where T : for<'a> Deserialize<'a>
{
    // Note we don't profile this entire function, for the same reason as below (see profile_this!("Tcp Read"))

    let mut len_buf = [0_u8; 8];
    tcp_connection.read_exact(&mut len_buf).map_err(|e| "Error reading len: ".to_string() + &e.to_string())?;
    let encrypted_len = usize::from_le_bytes(len_buf);

    // Note we only profile this after reading the length, as most of the time will likely be spent reading the length
    // because it will be waiting for a new message. This means the profiling trace will be filled with with bar,
    // which isn't very useful. Instead we start profiling when we read the actual message contents which will be much
    // larger and more interesting
    let mut encrypted_data = {
        profile_this!("Tcp Read");
        let buffer = &mut buffer[0..encrypted_len];
        tcp_connection.read_exact(buffer).map_err(|e| "Error reading encrypted data: ".to_string() + &e.to_string())?;
        buffer
    };

    // Nonces for doer -> boss should always be odd, and even for vice versa. They can't be reused between them.
    assert!(*receiving_nonce_counter % 2 == nonce_lsb);
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[0..8].copy_from_slice(&receiving_nonce_counter.to_le_bytes());
    let nonce = Nonce::<Aes128Gcm>::from_slice(&nonce_bytes);
    receiving_nonce_counter.checked_add(2).unwrap(); // Increment by two so that it never overlaps with the nonce used by the boss

    let unencrypted_data = {
        profile_this!("Decrypt");
        let mut s = SliceBuffer { slice: &mut encrypted_data, new_len: encrypted_len, write_offset: 0 };
        cipher.decrypt_in_place(nonce, &[], &mut s).map_err(|e| "Error decrypting: ".to_string() + &e.to_string())?;
        let t = s.new_len;
        &encrypted_data[0..t]
    };

    let response = {
        profile_this!("Deserialize");
        bincode::deserialize(&unencrypted_data).map_err(|e| "Error deserializing: ".to_string() + &e.to_string())?
    };

    Ok(response)
}

struct SliceBuffer<'a> {
    slice: &'a mut [u8],
    new_len: usize,
    write_offset: usize,
}

impl<'a ,'b> aead::Buffer for SliceBuffer<'a> {
    fn len(&self) -> usize {
        self.new_len
    }

    fn is_empty(&self) -> bool {
        self.new_len == 0
    }

    fn extend_from_slice(&mut self, other: &[u8]) -> aead::Result<()> {
    //    println!("extend_from_slice to {}", _other.len());
        self.slice[self.new_len..self.new_len+other.len()].copy_from_slice(other);
        self.new_len += other.len();
        aead::Result::Ok(())
    }

    fn truncate<'c>(&mut self, len: usize) {
       // self = &mut selfslice[0..len];
       // println!("Truncate to {}", len);
     //  panic!()
        self.new_len = len;
    }
}

impl<'a> AsRef<[u8]> for SliceBuffer<'a> {
    fn as_ref(&self) -> &[u8] {
        &self.slice[0..self.new_len]
    }
}

impl<'a> AsMut<[u8]> for SliceBuffer<'a> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.slice[0..self.new_len]
    }
}

//TODO: does this need to be in the same thing, it could probably be split out?
impl<'a> Write for &mut SliceBuffer<'a> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.slice[self.write_offset..self.write_offset+data.len()].copy_from_slice(data);
        self.write_offset += data.len();
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
