use std::collections::HashSet;
use std::net::SocketAddr;

use crate::errors::node_errors::NodeError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::Sender;

use crate::config::*;
use crate::encsocket::EncSocket;
use crate::errors::*;
use crate::models;
use crate::models::*;
use lazy_static::lazy_static;
use rmp_serde::{Deserializer, Serializer};
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::Duration;

lazy_static! {
    static ref PEER_TIMEOUT: Duration = Duration::from_secs(15);
}

const PEERS_BACKUP_FILE: &str = "peers.dump";

pub async fn load_peers(peers_mut: Arc<Mutex<HashSet<SocketAddr>>>) -> ResultSmall<()> {
    let file = File::open(PEERS_BACKUP_FILE)?;

    let mut decoder = zstd::Decoder::new(file)?;

    let mut decoded_data: Vec<u8> = Vec::new();

    decoder.read_to_end(&mut decoded_data)?;

    let peers = peers_dump::Peers::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

    let mut peers_storage = peers_mut.lock().await;
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

pub async fn dump_peers(peers_mut: Arc<Mutex<HashSet<SocketAddr>>>) -> ResultSmall<()> {
    let target = File::create(PEERS_BACKUP_FILE)?;

    let mut encoder = zstd::Encoder::new(target, 21)?;

    let peers_storage = peers_mut.lock().await;

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

/// Start node, entry point
pub async fn start(
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
) -> Result<(), node_errors::NodeError> {
    let mut rx = shutdown.subscribe();

    tokio::select! {
        _ = connect_to_peers(
            peers_mut.clone(),
            shutdown.clone(),
            propagate.clone(),
            new_peers_tx.clone()
        ) => {},
        _ = rx.recv() => {
            return Ok(());
        }
    };

    let listener = TcpListener::bind(*SERVER_ADDRESS)
        .await
        .map_err(|e| NodeError::BindSocketError(e))?;

    loop {
        let (sock, addr) = tokio::select! {
            res = listener.accept() => {
                res.map_err(|e| NodeError::AcceptConnectionError(e))?
            },
            _ = rx.recv() => {
                shutdown.send(0).unwrap();
                break;
            }
        };

        println!("New connection from: {}", addr);
        tokio::spawn(handle_incoming(
            sock,
            addr,
            shutdown.clone(),
            propagate.clone(),
            peers_mut.clone(),
            new_peers_tx.clone(),
        ));
    }

    Ok(())
}

/// Handle incoming connection wrapper
async fn handle_incoming(
    socket: TcpStream,
    addr: SocketAddr,
    shutdown: Sender<u8>,
    propagate: Sender<packet_models::Packet>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    new_peers_tx: Sender<SocketAddr>,
) -> Result<(), node_errors::NodeError> {
    let mut rx = shutdown.subscribe();
    tokio::select! {
        res = handle_incoming_wrapped(
            socket,
            addr,
            propagate,
            peers,
            new_peers_tx) => res,
        _ = rx.recv() => {
            Ok(())
        }
    }
}

/// Wrapped main body of incoming connection handler
async fn handle_incoming_wrapped(
    socket: TcpStream,
    addr: SocketAddr,
    mut propagate: Sender<packet_models::Packet>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    mut new_peers_tx: Sender<SocketAddr>,
) -> Result<(), NodeError> {
    let mut rx_propagate = propagate.subscribe();

    let mut waiting_response: HashSet<u64> = HashSet::with_capacity(20);

    let mut socket = EncSocket::new_connection(socket, addr)
        .await
        .map_err(|e| NodeError::ConnectToPeerError(addr, e))?;

    // main loop
    loop {
        let packet = tokio::select! {
            propagate_message = rx_propagate.recv() => {
                socket.send(propagate_message.map_err(|e| NodeError::PropagationReadError(addr, e))?).await.map_err(|e| NodeError::SendPacketError(addr, e))?;
                continue;
            },
            packet = socket.recv::<packet_models::Packet>() => {
                packet.map_err(|e| NodeError::ReceievePacketError(addr, e))?
            }
        };

        // handle packet
        if process_packet(
            &mut socket,
            packet,
            &mut waiting_response,
            peers.clone(),
            &mut propagate,
            &mut new_peers_tx,
        )
        .await
        .is_err()
        {
            break;
        }
    }

    Ok(())
}

async fn connect_to_peers(
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
) {
    let peers = peers_mut.lock().await;

    for peer in peers.iter() {
        tokio::spawn(connect_to_peer(
            *peer,
            peers_mut.clone(),
            shutdown.clone(),
            propagate.clone(),
            new_peers_tx.clone(),
        ));
    }
}

pub async fn connect_to_peer(
    addr: SocketAddr,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
) {
    let mut rx = shutdown.subscribe();
    tokio::select! {
        _ = rx.recv() => {},
        _ = handle_peer(
            &addr,
            peers_mut.clone(),
            propagate,
            new_peers_tx
        ) => {}
    };

    // remove peer from active peers
    let mut peers = peers_mut.lock().await;
    peers.remove(&addr);
}

pub async fn handle_peer(
    addr: &SocketAddr,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    mut propagate: Sender<packet_models::Packet>,
    mut new_peers_tx: Sender<SocketAddr>,
) -> Result<(), NodeError> {
    // set up
    let mut rx_propagate = propagate.subscribe();

    let mut socket = EncSocket::create_new_connection(addr.clone(), *PEER_TIMEOUT)
        .await
        .map_err(|e| NodeError::ConnectToPeerError(*addr, e))?;

    let mut waiting_response: HashSet<u64> = HashSet::with_capacity(20);

    // announce
    let id: u64 = rand::random();

    let body = models::addr2bin(&SERVER_ADDRESS);
    let packet = packet_models::Packet::request(packet_models::Request::announce(
        packet_models::AnnounceRequest { id, addr: body },
    ));

    socket
        .send(packet)
        .await
        .map_err(|e| NodeError::SendPacketError(*addr, e))?;

    // main loop
    loop {
        let packet = tokio::select! {
            propagate_message = rx_propagate.recv() => {
                socket.send(propagate_message.map_err(|e| NodeError::PropagationReadError(*addr, e))?).await.map_err(|e| NodeError::SendPacketError(*addr, e))?;
                continue;
            },
            packet = socket.recv::<packet_models::Packet>() => {
                packet.map_err(|e| NodeError::ReceievePacketError(*addr, e))?
            }
        };

        // handle packet
        if process_packet(
            &mut socket,
            packet,
            &mut waiting_response,
            peers_mut.clone(),
            &mut propagate,
            &mut new_peers_tx,
        )
        .await
        .is_err()
        {
            break;
        }
    }

    Ok(())
}

async fn process_packet(
    socket: &mut EncSocket,
    packet: packet_models::Packet,
    waiting_response: &mut HashSet<u64>,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    propagate: &mut Sender<packet_models::Packet>,
    new_peers_tx: &mut Sender<SocketAddr>,
) -> Result<(), NodeError> {
    match &packet {
        packet_models::Packet::request(r) => match r {
            packet_models::Request::announce(p) => {
                let addr = bin2addr(&p.addr).map_err(|e| NodeError::BinToAddressError(e))?;

                // verify address is not loopback
                if (addr.ip().is_loopback() && addr.port() == SERVER_ADDRESS.port())
                    || addr.ip().is_unspecified()
                {
                    let response_packet = packet_models::Packet::error(packet_models::ErrorR {
                        code: packet_models::ErrorCode::BadAddress,
                    });
                    socket
                        .send(response_packet)
                        .await
                        .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;

                    return Ok(());
                }

                let mut peers = peers_mut.lock().await;
                let res = peers.insert(addr);
                drop(peers);

                if res {
                    propagate
                        .send(packet)
                        .map_err(|e| NodeError::PropagationSendError(addr, e.to_string()))?;
                    new_peers_tx
                        .send(addr)
                        .map_err(|e| NodeError::PropagationSendError(addr, e.to_string()))?;
                }
            }
            packet_models::Request::get_amount(p) => {}
            packet_models::Request::get_nodes(p) => {
                let mut peers_cloned: Box<[SocketAddr]>;
                {
                    // clone peers into vec
                    let peers = peers_mut.lock().await;
                    peers_cloned = vec![*SERVER_ADDRESS; peers.len()].into_boxed_slice();
                    for (index, peer) in peers.iter().enumerate() {
                        let cell = unsafe { peers_cloned.get_unchecked_mut(index) };
                        *cell = *peer;
                    }
                    drop(peers);
                }

                // dump ipv4 and ipv6 addresses in u8 vecs separately
                let (ipv4, ipv6) = dump_addresses(&peers_cloned);

                let packet = packet_models::Packet::response(packet_models::Response::get_nodes(
                    packet_models::GetNodesReponse {
                        id: p.id,
                        ipv4,
                        ipv6,
                    },
                ));

                socket
                    .send(packet)
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?
            }
            packet_models::Request::get_transaction(p) => {}
        },
        packet_models::Packet::response(r) => {}
        packet_models::Packet::error(e) => {}
    }

    Ok(())
}

pub async fn connect_new_peers(
    shutdown: Sender<u8>,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
) {
    let mut shutdown_watcher = shutdown.subscribe();
    let mut new_peers_rx = new_peers_tx.subscribe();

    tokio::select! {
        _ = shutdown_watcher.recv() => {},
        _ = connect_new_peers_wrapped(
            peers_mut,
            propagate,
            shutdown.clone(),
            &mut new_peers_rx,
            new_peers_tx
        ) => {}
    }
}

async fn connect_new_peers_wrapped(
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    propagate: Sender<packet_models::Packet>,
    shutdown: Sender<u8>,
    new_peers_rx: &mut Receiver<SocketAddr>,
    new_peers_tx: Sender<SocketAddr>,
) {
    loop {
        let peer_addr = match new_peers_rx.recv().await {
            Ok(p) => p,
            Err(_) => {
                continue;
            }
        };

        // // probably need to properly check if peer has
        // // already being connected to
        // let mut peers = peers_mut.lock().unwrap();
        // if !peers.insert(peer_addr) {
        //     continue;
        // }
        // drop(peers);

        tokio::spawn(connect_to_peer(
            peer_addr,
            peers_mut.clone(),
            shutdown.clone(),
            propagate.clone(),
            new_peers_tx.clone(),
        ));
    }
}
