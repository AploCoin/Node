use std::collections::HashSet;
use std::net::SocketAddr;

use crate::errors::*;
use crate::models::*;
use chacha20::cipher::KeyIvInit;
use chacha20::ChaCha20;
use getrandom::getrandom;
use rand_core::OsRng;
use rmp_serde::{Deserializer, Serializer};
use serde::Deserialize;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};
use zstd;

use crate::tools;

const PEERS_BACKUP_FILE: &str = "peers.dump";

#[derive(Debug)]
pub struct Peer {
    addr: SocketAddr,
    time_connected: u64,
    last_sent_message: u64,
    last_response: u64,
}

pub struct Node {
    peers: HashSet<SocketAddr>,
    listener: TcpListener,
}

impl Node {
    pub async fn new(addr: &str) -> Result<Node> {
        Ok(Node {
            listener: TcpListener::bind(addr).await?,
            peers: HashSet::with_capacity(100),
        })
    }

    pub fn load_peers(&mut self) -> Result<()> {
        let file = File::open(PEERS_BACKUP_FILE)?;

        let mut decoder = zstd::Decoder::new(file)?;

        let mut decoded_data: Vec<u8> = Vec::new();

        decoder.read_to_end(&mut decoded_data)?;

        let peers =
            peers_dump::Peers::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

        if let Some(dump) = peers.ipv4{
            let parsed = parse_ipv4(&dump)?;
            for addr in parsed{
                self.peers.insert(addr);
            }
        }

        if let Some(dump) = peers.ipv6{
            let parsed = parse_ipv6(&dump)?;
            for addr in parsed{
                self.peers.insert(addr);
            }
        }

        Ok(())
    }

    pub async fn start(&mut self) -> Result<()> {
        Ok(())
    }
}
