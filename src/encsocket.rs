use crate::errors::enc_socket_errors::EncSocketError;
use chacha20::cipher::KeyIvInit;
use chacha20::cipher::StreamCipher;
use chacha20::cipher::StreamCipherSeek;
use chacha20::ChaCha20;
use rand_core::OsRng;
use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::prelude::*;
use std::io::Cursor;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Duration;
use x25519_dalek::{EphemeralSecret, PublicKey};

static MAX_PACKET_SIZE: usize = 5242880; // size in bytes

pub struct EncSocket {
    socket: TcpStream,
    cipher: ChaCha20,
    pub addr: SocketAddr,
}

impl EncSocket {
    #[allow(dead_code)]
    pub fn new(socket: TcpStream, cipher: ChaCha20, addr: SocketAddr) -> EncSocket {
        EncSocket {
            socket,
            cipher,
            addr,
        }
    }

    /// Generate nonce for the ChaCha20
    ///
    /// Takes arbitrary data, for encryption in this case shared key is used
    ///
    /// create sha256 hash out of this data
    ///
    /// sum chunks of generated hash 12+12+8 starting from the first byte
    ///
    /// return generated nonce
    fn generate_nonce(data: &[u8]) -> [u8; 12] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();

        let mut nonce = [0u8; 12];

        for chunk in result.chunks(12) {
            for (index, number) in chunk.iter().enumerate() {
                nonce[index] += number;
            }
        }

        nonce
    }

    pub async fn new_connection(
        mut socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<EncSocket, EncSocketError> {
        let mut buf = [0; 32];
        let secret = EphemeralSecret::new(OsRng);
        let public = PublicKey::from(&secret);

        socket
            .write(public.as_bytes())
            .await
            .map_err(|e| EncSocketError::WriteSocketError(e))?;

        socket
            .read_exact(&mut buf)
            .await
            .map_err(|e| EncSocketError::ReadSocketError(e))?;

        let other_public = PublicKey::from(buf);
        let shared = secret.diffie_hellman(&other_public);
        let nonce = EncSocket::generate_nonce(shared.as_bytes());

        let cipher = ChaCha20::new(shared.as_bytes().into(), &nonce.into());

        Ok(EncSocket {
            socket,
            cipher,
            addr,
        })
    }

    pub async fn establish_new_connection(
        mut socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<EncSocket, EncSocketError> {
        let mut buf = [0; 32];
        let secret = EphemeralSecret::new(OsRng);
        let public = PublicKey::from(&secret);

        socket
            .read_exact(&mut buf)
            .await
            .map_err(|e| EncSocketError::ReadSocketError(e))?;

        socket
            .write(public.as_bytes())
            .await
            .map_err(|e| EncSocketError::WriteSocketError(e))?;

        let other_public = PublicKey::from(buf);
        let shared = secret.diffie_hellman(&other_public);
        let nonce = EncSocket::generate_nonce(shared.as_bytes());

        let cipher = ChaCha20::new(shared.as_bytes().into(), &nonce.into());

        Ok(EncSocket {
            socket,
            cipher,
            addr,
        })
    }

    pub async fn create_new_connection(
        addr: SocketAddr,
        timeout: Duration,
    ) -> Result<EncSocket, EncSocketError> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| EncSocketError::TimeoutError)?
            .map_err(|e| EncSocketError::ConnectError {
                address: addr,
                reason: e,
            })?;

        EncSocket::establish_new_connection(stream, addr).await
    }

    pub async fn recv_raw(&mut self) -> Result<(usize, Vec<u8>), EncSocketError> {
        // read size of the packet
        let mut recv_buffer = [0u8; 4];
        self.socket
            .read_exact(&mut recv_buffer)
            .await
            .map_err(|e| EncSocketError::ReadSocketError(e))?;
        let packet_size = u32::from_be_bytes(recv_buffer) as usize;

        if packet_size > MAX_PACKET_SIZE {
            return Err(EncSocketError::TooBigPacketError(packet_size));
        }

        // read actual packet
        let mut recv_buffer = vec![0u8; packet_size];
        self.socket
            .read_exact(&mut recv_buffer)
            .await
            .map_err(|e| EncSocketError::ReadSocketError(e))?;

        // decrypt packet
        self.cipher.apply_keystream(&mut recv_buffer);
        self.cipher.seek(0);

        // uncompress packet
        let mut decoded_data: Vec<u8> = Vec::with_capacity(packet_size);
        let cur = Cursor::new(recv_buffer);
        let mut decoder =
            zstd::Decoder::new(cur).map_err(|e| EncSocketError::DecompressError(e))?;
        decoder
            .read_to_end(&mut decoded_data)
            .map_err(|e| EncSocketError::DecompressError(e))?;

        Ok((packet_size, decoded_data))
    }

    pub async fn send_raw(&mut self, data: &[u8]) -> Result<(), EncSocketError> {
        let mut encoded_data: Vec<u8> = Vec::with_capacity(data.len());

        let cur = Cursor::new(&mut encoded_data);
        let mut encoder =
            zstd::Encoder::new(cur, 21).map_err(|e| EncSocketError::CompressError(e))?;
        encoder
            .write_all(&data)
            .map_err(|e| EncSocketError::CompressError(e))?;
        encoder
            .finish()
            .map_err(|e| EncSocketError::CompressError(e))?;

        self.cipher.apply_keystream(&mut encoded_data);
        self.cipher.seek(0);

        let packet_size = &(encoded_data.len() as u32).to_be_bytes();
        self.socket
            .write_all(packet_size)
            .await
            .map_err(|e| EncSocketError::WriteSocketError(e))?;
        self.socket
            .write_all(&encoded_data)
            .await
            .map_err(|e| EncSocketError::WriteSocketError(e))?;

        Ok(())
    }

    pub async fn recv<'a, PT: Deserialize<'a>>(&mut self) -> Result<PT, EncSocketError> {
        Ok(PT::deserialize(&mut Deserializer::new(Cursor::new(
            self.recv_raw().await?.1,
        )))
        .map_err(|e| EncSocketError::DeserializeError(e))?)
    }

    pub async fn send<PT: Serialize>(&mut self, packet: PT) -> Result<(), EncSocketError> {
        let mut buf: Vec<u8> = Vec::with_capacity(100);
        packet.serialize(&mut Serializer::new(&mut buf)).unwrap();

        self.send_raw(&buf).await
    }
}