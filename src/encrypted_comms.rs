use std::{net::TcpStream, io::{Write, Read, ErrorKind}, thread::{JoinHandle, self}, fmt::{Display, Debug}};

use aead::{Key, KeyInit};
use bytes::{BytesMut, BufMut};
use aes_gcm::{Aes128Gcm, aead::{Nonce, Aead}, AeadInPlace};
use log::{trace, error};
use serde::{Deserialize, Serialize};

use crate::{profile_this, memory_bound_channel::{Sender, Receiver, self}, BOSS_DOER_CHANNEL_MEMORY_CAPACITY};

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

    sending_thread: JoinHandle<Result<(Aes128Gcm, u64, u64), String>>,
    pub sender: Sender<S>,

    receiving_thread: JoinHandle<Result<(), String>>,
    pub receiver: Receiver<R>,
}
impl<S: Serialize + Send + 'static + Debug, R: for<'a> Deserialize<'a> + Serialize + Send + 'static + Debug> AsyncEncryptedComms<S, R> {
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
                loop {
                    let s = match thread_receiver.recv() {
                        Ok(s) => s,
                        Err(_) => {
                            // The sender on the main thread has been dropped, which means that there are no more messages to send, 
                            // so we finish this background thread successfully (this is the expected clean shutdown process)
                            trace!("Sending thread '{sending_thread_name}' shutting down due to closed channel");
                            // Return stuff needed to send one more message from the main thread (needed for profiling)
                            return Ok((cipher, sending_nonce_counter, sending_nonce_lsb)); 
                        }
                    };
                    if let Err(e) = send(s, &mut tcp_connection_clone1, &cipher, &mut sending_nonce_counter, sending_nonce_lsb) {
                        // There was an error sending a message, which shouldn't happen in normal operation.
                        // Log an error, and stop this background thread, which will close the receiving side of the 
                        // channel. The main thread will detect this as a closed channel.
                        error!("Sending thread '{sending_thread_name}' shutting down due to error sending on TCP: {e}");
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
                loop {
                    let r = match receive(&mut tcp_connection_clone2, &cipher, &mut receiving_nonce_counter, receiving_nonce_lsb) {
                        Ok(r) => r,
                        Err(ReceiveError::TcpStreamClosed) => {
                            // The TCP connection was closed cleanly and there was nothing more to read from it.
                            // This is the expected clean shutdown path.
                            trace!("Receiving thread '{receiving_thread_name}' shutting down due to end of TCP stream");
                            return Ok(());
                        }
                        Err(ReceiveError::Other(e)) => {
                            // There was an error receiving a message, which shouldn't happen in normal operation.
                            // Log an error, and stop this background thread, which will close the sending side of the 
                            // channel. The main thread will detect this as a closed channel.
                            error!("Receiving thread '{receiving_thread_name}' shutting down due to error receiving from TCP: {e}");
                            return Err(e);
                        }
                    };
                    if thread_sender.send(r).is_err() {
                        // The main thread receiver has been dropped, which shouldn't happen during normal operation
                        error!("Receiving thread '{receiving_thread_name}' shutting down due to closed channel");
                        return Err("Communications with main thread broken".to_string());
                    };
                }
            }).expect("Failed to spawn thread");

        AsyncEncryptedComms { tcp_connection, sending_thread, sender, receiving_thread, receiver }
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
        // from windows.

        // Stop the sending thread, which will be blocked on the channel waiting for a new message to send,
        // and retrieve the cipher etc. needed to send one more final message
        drop(self.sender);
        trace!("Waiting for sending thread");
        let sending_thread_result = join_with_err_log(self.sending_thread);

        // Stop the receiving thread, which will be blocked on reading from the TCP connection
        drop(self.receiver);
        trace!("Closing TCP read");
        // Unblock the TCP read, note that this might fail if the other end has already closed the connection (at least it can on Linux), so we ignore the result
        let _ = self.tcp_connection.shutdown(std::net::Shutdown::Read);
        trace!("Waiting for receiving thread");
        join_with_err_log(self.receiving_thread);

        if let Some((cipher, mut sending_nonce_counter, sending_nonce_lsb)) = sending_thread_result {
            let s = message_generating_func();

            trace!("Sending final mesage {:?}", s);
            // There's not much we can do with an error here, as we're closing everything down anyway
            if let Err(e) = send(s, &mut self.tcp_connection, &cipher, &mut sending_nonce_counter, sending_nonce_lsb) {
                error!("Error sending final message: {e}");
            }
        } else {
            error!("Unable to send final message as sending thread didn't complete successfully");
        }
    }

    /// Clean shutdown which joins the background threads, making sure all messages are flushed etc.
    /// Prefer this to simply dropping the object, which will leave the threads to exit on their own.
    /// This version of shutdown receives one final message after closing down the sending half 
    /// of the TCP connection. This is necessary for profiling, where the boss needs to wait for profiling
    /// data from the remote doer, but needs to close its sending connection first so that the doer can join
    /// its receiving thread to collect profiling data from it, before it can send the profiling data.
    pub fn shutdown_with_final_message_received_after_closing_send(self) -> Option<R> {
        trace!("AsyncEncryptedComms::shutdown_with_final_message_received_after_closing_send");
        // The order here is important, so that both sides of the conenction exit cleanly and we don't deadlock.
        // I had some issues with windows -> remote linux when shutting down the writing half of the connection
        // from windows.

        // Stop the sending thread, which will be blocked on the channel waiting for a new message to send.
        drop(self.sender);
        trace!("Waiting for sending thread");
        join_with_err_log(self.sending_thread);

        trace!("Waiting for final message");
        let r = self.receiver.recv().ok();

        // Stop the receiving thread, which might be blocked on reading from the TCP connection
        drop(self.receiver);
        // Unblock the TCP read, note that this might fail if the other end has already closed the connection (at least it can on Linux), so we ignore the result
        let _ = self.tcp_connection.shutdown(std::net::Shutdown::Read);
        trace!("Waiting for receiving thread");
        join_with_err_log(self.receiving_thread);

        r
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

enum ReceiveError {
    TcpStreamClosed,
    Other(String)
}
impl From<String> for ReceiveError { // So that the '?' operator works nicely below
    fn from(s: String) -> Self {
        ReceiveError::Other(s)
    }
}

fn receive<T>(tcp_connection: &mut TcpStream, cipher: &Aes128Gcm,
    receiving_nonce_counter: &mut u64, nonce_lsb: u64) -> Result<T, ReceiveError>
    where T : for<'a> Deserialize<'a>
{
    profile_this!();

    let encrypted_data = {
        profile_this!("Tcp Read");

        let mut len_buf = [0_u8; 8];
        match tcp_connection.read_exact(&mut len_buf) {
            Ok(()) => (),
            // Distinguish an EOF error when reading the first part of a new message, as this is the expected clean shutdown path
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Err(ReceiveError::TcpStreamClosed),
            Err(e) => return Err(ReceiveError::Other("Error reading len: ".to_string() + &e.to_string())),
        }
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
