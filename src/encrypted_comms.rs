use std::{net::TcpStream, io::{Write, Read}};

use aes_gcm::{Aes128Gcm, aead::{Nonce, Aead}};
use serde::{Deserialize, Serialize};

pub fn send<T>(x: T, tcp_connection: &mut TcpStream, cipher: &Aes128Gcm, 
    sending_nonce_counter: &mut u64, nonce_lsb: u8) -> Result<(), String> 
    where T : Serialize,
{
    let unencrypted_data = bincode::serialize(&x).map_err(|e| "Error serializing command: ".to_string() + &e.to_string())?;

    // Nonces for boss -> doer should always be even, and odd for vice versa. They can't be reused between them.
    assert!(*sending_nonce_counter % 2 == nonce_lsb.into()); 
    let mut nonce_bytes = sending_nonce_counter.to_le_bytes().to_vec();
    nonce_bytes.resize(12, 0); // pad it
    let nonce = Nonce::<Aes128Gcm>::from_slice(&nonce_bytes);
    sending_nonce_counter.checked_add(2).unwrap(); // Increment by two so that it never overlaps with the nonce used by the doer
    
    let ciphertext = cipher.encrypt(nonce, unencrypted_data.as_ref()).unwrap();

    tcp_connection.write_all(&ciphertext.len().to_le_bytes()).map_err(|e| "Error sending length: ".to_string() + &e.to_string())?;
    tcp_connection.write_all(&ciphertext).map_err(|e| "Error sending length: ".to_string() + &e.to_string())?;

    // Flush to make sure that we don't deadlock (other side waiting for data that never comes)
    tcp_connection.flush().map_err(|e| "Error flushing: ".to_string() + &e.to_string())?;

    Ok(())
}

pub fn receive<T>(tcp_connection: &mut TcpStream, cipher: &Aes128Gcm, 
    receiving_nonce_counter: &mut u64, nonce_lsb: u8) -> Result<T, String> 
    where T : for<'a> Deserialize<'a>
{
    let mut len_buf = [0_u8; 8];            
    tcp_connection.read_exact(&mut len_buf).map_err(|e| "Error reading len: ".to_string() + &e.to_string())?;
    let encrypted_len = usize::from_le_bytes(len_buf);

    let mut encrypted_data = vec![0_u8; encrypted_len];
    tcp_connection.read_exact(&mut encrypted_data).map_err(|e| "Error reading encrypted data: ".to_string() + &e.to_string())?;

    // Nonces for doer -> boss should always be odd, and even for vice versa. They can't be reused between them.
    assert!(*receiving_nonce_counter % 2 == nonce_lsb.into());
    let mut nonce_bytes = receiving_nonce_counter.to_le_bytes().to_vec();
    nonce_bytes.resize(12, 0); // pad it
    let nonce = Nonce::<Aes128Gcm>::from_slice(&nonce_bytes);
    receiving_nonce_counter.checked_add(2).unwrap(); // Increment by two so that it never overlaps with the nonce used by the boss

    let unencrypted_data = cipher.decrypt(nonce, encrypted_data.as_ref()).map_err(|e| "Error decrypting: ".to_string() + &e.to_string())?;

    let response = bincode::deserialize(&unencrypted_data).map_err(|e| "Error deserializing: ".to_string() + &e.to_string())?;

    Ok(response)
}
