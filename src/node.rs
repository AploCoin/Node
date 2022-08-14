use std::collections::HashSet;
use std::net::SocketAddr;

use chacha20::cipher::StreamCipher;
use chacha20::cipher::StreamCipherSeek;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::Sender;

use crate::errors::*;
use crate::models::*;
//use crate::tools::*;
use chacha20::cipher::KeyIvInit;
use chacha20::ChaCha20;
use getrandom::getrandom;
use lazy_static::lazy_static;
use rand_core::OsRng;
use rmp_serde::{Deserializer, Serializer};
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Duration;
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};

lazy_static! {
    static ref PEER_TIMEOUT: Duration = Duration::from_secs(15);
}

const PEERS_BACKUP_FILE: &str = "peers.dump";

macro_rules! read_exact {
    ($sock:expr, $buf:expr, $propagate:expr) => {
        loop {
            let _ = tokio::select! {
                _msg = $propagate.recv() =>{
                    continue;
                },
                res = $sock.read_exact(&mut $buf) => {
                    res?;
                    break;
                }
            };
        }
    };
}

// #[derive(Debug)]
// pub struct Peer {
//     addr: SocketAddr,
//     time_connected: u64,
//     last_sent_message: u64,
//     last_response: u64,
// }

pub fn load_peers(peers_mut: Arc<Mutex<HashSet<SocketAddr>>>) -> ResultSmall<()> {
    let file = File::open(PEERS_BACKUP_FILE)?;

    let mut decoder = zstd::Decoder::new(file)?;

    let mut decoded_data: Vec<u8> = Vec::new();

    decoder.read_to_end(&mut decoded_data)?;

    let peers = peers_dump::Peers::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

    let mut peers_storage = peers_mut.lock().unwrap();
    if let Some(dump) = peers.ipv4 {
        let parsed = parse_ipv4(&dump)?;
        for addr in parsed {
            peers_storage.insert(addr);
        }
    }

    if let Some(dump) = peers.ipv6 {
        let parsed = parse_ipv6(&dump)?;
        for addr in parsed {
            peers_storage.insert(addr);
        }
    }

    Ok(())
}

pub fn dump_peers(peers_mut: Arc<Mutex<HashSet<SocketAddr>>>) -> ResultSmall<()> {
    let target = File::create(PEERS_BACKUP_FILE)?;

    let mut encoder = zstd::Encoder::new(target, 21)?;

    let peers_storage = peers_mut.lock().unwrap();

    let mut peers: Vec<SocketAddr> = Vec::with_capacity(peers_storage.len());

    for peer in peers_storage.iter() {
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

pub async fn start(
    addr: &str,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<Vec<u8>>,
) -> Result<(), node_errors::NodeError> {
    let mut rx = shutdown.subscribe();

    tokio::select! {
        _ = connect_to_peers(peers_mut.clone(),shutdown.clone(),propagate.clone()) => {},
        _ = rx.recv() => {
            return Ok(());
        }
    };

    let listener = match TcpListener::bind(addr).await {
        Ok(s) => s,
        Err(e) => {
            return Err(node_errors::NodeError::new(e.to_string()));
        }
    };

    println!("Node started");

    loop {
        let (sock, addr) = tokio::select! {
            res = listener.accept() => {
                match res{
                    Ok(s) => s,
                    Err(e) => {
                        return Err(node_errors::NodeError::new(e.to_string()));
                    }
                }
            },
            _ = rx.recv() => {
                println!("Received stop signal");
                shutdown.send(0).unwrap();
                break;
            }
        };

        // let mut peers = match peers_mut.lock() {
        //     Ok(p) => p,
        //     Err(e) => {
        //         return Err(node_errors::NodeError::new(e.to_string()));
        //     }
        // };
        // peers.insert(addr);
        // drop(peers);

        tokio::spawn(handle_incoming(
            sock,
            addr,
            shutdown.clone(),
            propagate.clone(),
            peers_mut.clone(),
        ));
    }

    Ok(())
}

async fn handle_incoming(
    mut socket: TcpStream,
    addr: SocketAddr,
    shutdown: Sender<u8>,
    propagate: Sender<Vec<u8>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
) -> Result<(), node_errors::NodeError> {
    let mut rx = shutdown.subscribe();
    let mut rx_propagate = propagate.subscribe();

    let (nonce, shared) = tokio::select! {
        res = exchange_keys(&mut socket) => res?,
        _ = rx.recv() => {
            return Ok(());
        }
    };
    let mut cipher = ChaCha20::new(shared.as_bytes().into(), &nonce.into());

    // main loop
    loop {
        let _packet = tokio::select! {
            _ = rx.recv() => {
                // stop connection
                break;
            }
            pack = receive_packet(&mut socket, &mut cipher, &mut rx_propagate) => {
                match pack{
                    Ok(p) => p,
                    Err(e) => {
                        return Err(node_errors::NodeError::new(e.to_string()));
                    }
                }
            }
        };

        // handle packet
    }

    let mut peers_unwrapped = peers.lock().unwrap();
    peers_unwrapped.remove(&addr);

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

    if let Err(e) = socket.read_exact(&mut buf).await {
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

async fn receive_packet(
    socket: &mut TcpStream,
    cipher: &mut ChaCha20,
    propagate: &mut Receiver<Vec<u8>>,
) -> ResultSmall<packet_models::Packet> {
    // read size of the packet
    let mut recv_buffer = [0u8; 4];
    read_exact!(socket, recv_buffer, propagate);
    let packet_size = u32::from_be_bytes(recv_buffer) as usize;

    // read actual packet
    let mut recv_buffer = vec![0u8; packet_size];
    read_exact!(socket, recv_buffer, propagate);

    // decrypt packet
    cipher.apply_keystream(&mut recv_buffer);
    cipher.seek(0);

    // uncompress packet
    let mut decoded_data: Vec<u8> = Vec::with_capacity(packet_size);
    let cur = Cursor::new(recv_buffer);
    let mut decoder = zstd::Decoder::new(cur)?;
    decoder.read_to_end(&mut decoded_data)?;

    // deserialize packet
    let packet =
        packet_models::Packet::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

    Ok(packet)
}

async fn connect_to_peers(
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<Vec<u8>>,
) {
    let peers = peers_mut.lock().unwrap();

    for peer in peers.iter() {
        tokio::spawn(connect_to_peer(
            *peer,
            peers_mut.clone(),
            shutdown.clone(),
            propagate.clone(),
        ));
    }
}

async fn exchange_keys_client(
    socket: &mut TcpStream,
) -> Result<([u8; 12], SharedSecret), node_errors::NodeError> {
    let mut buf = [0; 32];
    let secret = EphemeralSecret::new(OsRng);
    let public = PublicKey::from(&secret);

    if let Err(e) = socket.read_exact(&mut buf).await {
        return Err(node_errors::NodeError::new(e.to_string()));
    };

    if let Err(e) = socket.write(public.as_bytes()).await {
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

pub async fn connect_to_peer(
    addr: SocketAddr,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<Vec<u8>>,
) {
    let mut rx = shutdown.subscribe();
    tokio::select! {
        _ = rx.recv() => {},
        _ = handle_peer(&addr, peers_mut, propagate) => {}
    };
}

pub async fn handle_peer(
    addr: &SocketAddr,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    propagate: Sender<Vec<u8>>,
) -> Result<(), node_errors::NodeError> {
    let mut rx_propagate = propagate.subscribe();

    let mut socket =
        if let Ok(Ok(s)) = tokio::time::timeout(*PEER_TIMEOUT, TcpStream::connect(addr)).await {
            s
        } else {
            // might be a bug with mutex being block forever
            let mut peers = peers_mut.lock().unwrap();
            peers.remove(addr);

            return Err(node_errors::NodeError::new("Connection error".to_string()));
        };

    let (nonce, shared) = match exchange_keys_client(&mut socket).await {
        Ok(d) => d,
        Err(e) => {
            let mut peers = peers_mut.lock().unwrap();
            peers.remove(addr);

            return Err(e);
        }
    };

    let mut cipher = ChaCha20::new(shared.as_bytes().into(), &nonce.into());

    Ok(())
}
