use std::collections::HashSet;
use std::net::SocketAddr;

use tokio::sync::broadcast::Sender;

use crate::errors::*;
use crate::models::*;
//use crate::tools::*;
use chacha20::cipher::KeyIvInit;
use chacha20::ChaCha20;
use getrandom::getrandom;
use rand_core::OsRng;
use rmp_serde::{Deserializer, Serializer};
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};

const PEERS_BACKUP_FILE: &str = "peers.dump";

// #[derive(Debug)]
// pub struct Peer {
//     addr: SocketAddr,
//     time_connected: u64,
//     last_sent_message: u64,
//     last_response: u64,
// }

pub struct Node {
    peers: HashSet<SocketAddr>,
    listener: TcpListener,
    shutdown: Sender<u8>,
}

impl Node {
    pub async fn new(addr: &str, shutdown: Sender<u8>) -> ResultSmall<Node> {
        Ok(Node {
            listener: TcpListener::bind(addr).await?,
            peers: HashSet::with_capacity(100),
            shutdown,
        })
    }

    pub fn load_peers(&mut self) -> ResultSmall<()> {
        let file = File::open(PEERS_BACKUP_FILE)?;

        let mut decoder = zstd::Decoder::new(file)?;

        let mut decoded_data: Vec<u8> = Vec::new();

        decoder.read_to_end(&mut decoded_data)?;

        let peers =
            peers_dump::Peers::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

        if let Some(dump) = peers.ipv4 {
            let parsed = parse_ipv4(&dump)?;
            for addr in parsed {
                self.peers.insert(addr);
            }
        }

        if let Some(dump) = peers.ipv6 {
            let parsed = parse_ipv6(&dump)?;
            for addr in parsed {
                self.peers.insert(addr);
            }
        }

        Ok(())
    }

    pub fn dump_peers(&self) -> ResultSmall<()> {
        let target = File::create(PEERS_BACKUP_FILE)?;

        let mut encoder = zstd::Encoder::new(target, 21)?;

        let mut peers: Vec<SocketAddr> = Vec::with_capacity(self.peers.len());

        for peer in self.peers.iter() {
            peers.push(*peer);
        }

        let (ipv4, ipv6) = dump_addresses(&peers);

        let mut buf: Vec<u8> = Vec::new();

        let peers = peers_dump::Peers { ipv4, ipv6 };

        peers.serialize(&mut Serializer::new(&mut buf))?;

        encoder.write_all(&buf)?;

        encoder.finish()?;

        Ok(())
    }

    pub async fn start(&mut self) -> ResultSmall<()> {
        let mut rx = self.shutdown.subscribe();
        loop {
            let (sock, addr) = tokio::select! {
                res = self.listener.accept() => res?,
                _ = rx.recv() => {
                    break;
                }
            };

            // let time_connected = current_time();

            // let peer = Peer{
            //     addr,
            //     time_connected,
            //     last_response:time_connected,
            //     last_sent_message:time_connected
            // };

            self.peers.insert(addr);

            tokio::spawn(Node::handle_incoming(sock, self.shutdown.clone()));
        }

        Ok(())
    }

    async fn handle_incoming(
        mut socket: TcpStream,
        shutdown: Sender<u8>,
    ) -> Result<(), node_errors::NodeError> {
        let (nonce, shared) = Node::exchange_keys(&mut socket).await?;
        let cipher = ChaCha20::new(shared.as_bytes().into(), &nonce.into());
        Ok(())
    }

    async fn exchange_keys(
        socket: &mut TcpStream,
    ) -> Result<([u8; 12], SharedSecret), node_errors::NodeError> {
        let mut buf = [0; 32];
        let secret = EphemeralSecret::new(OsRng);
        let public = PublicKey::from(&secret);

        if let Err(e) = socket.write(public.as_bytes()).await {
            return Err(node_errors::NodeError::new(e.to_string()));
        };

        if let Err(e) = socket.read(&mut buf).await {
            return Err(node_errors::NodeError::new(e.to_string()));
        };

        let other_public = PublicKey::from(buf);
        let shared = secret.diffie_hellman(&other_public);

        let mut nonce = [0u8; 12];

        match getrandom(&mut nonce) {
            Ok(_) => {}
            Err(e) => {
                return Err(node_errors::NodeError::new(
                    node_errors::GetRandomError::new(e).to_string(),
                ));
            }
        }

        Ok((nonce, shared))
    }
}
