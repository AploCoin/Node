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
use blockchaintree::blockchaintree::BlockChainTree;
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{debug, error, info};

lazy_static! {
    static ref PEER_TIMEOUT: Duration = Duration::from_secs(15);
}

/// Start node, entry point
pub async fn start(
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
    blockchain: Arc<BlockChainTree>,
) -> Result<(), node_errors::NodeError> {
    let mut rx = shutdown.subscribe();

    tokio::select! {
        _ = connect_to_peers(
            peers_mut.clone(),
            shutdown.clone(),
            propagate.clone(),
            new_peers_tx.clone(),
            blockchain.clone()
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
            blockchain.clone(),
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
    blockchain: Arc<BlockChainTree>,
) -> Result<(), node_errors::NodeError> {
    let mut rx = shutdown.subscribe();
    tokio::select! {
        res = handle_incoming_wrapped(
            socket,
            addr,
            propagate,
            peers,
            new_peers_tx,
            blockchain) => res,
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
    blockchain: Arc<BlockChainTree>,
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
            &blockchain,
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
    blockchain: Arc<BlockChainTree>,
) {
    let peers = peers_mut.lock().await;

    for peer in peers.iter() {
        tokio::spawn(connect_to_peer(
            *peer,
            peers_mut.clone(),
            shutdown.clone(),
            propagate.clone(),
            new_peers_tx.clone(),
            blockchain.clone(),
        ));
    }
}

pub async fn connect_to_peer(
    addr: SocketAddr,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    shutdown: Sender<u8>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
    blockchain: Arc<BlockChainTree>,
) {
    let mut rx = shutdown.subscribe();
    tokio::select! {
        _ = rx.recv() => {},
        _ = handle_peer(
            &addr,
            peers_mut.clone(),
            propagate,
            new_peers_tx,
            blockchain
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
    blockchain: Arc<BlockChainTree>,
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
    let packet = packet_models::Packet::Request(packet_models::Request::Announce(
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
            &blockchain,
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
    blockchain: &Arc<BlockChainTree>,
) -> Result<(), NodeError> {
    match &packet {
        packet_models::Packet::Request(r) => match r {
            packet_models::Request::Ping(p) => socket
                .send(packet_models::Packet::Response(
                    packet_models::Response::Ping(packet_models::PingResponse { id: p.id }),
                ))
                .await
                .map_err(|e| NodeError::SendPacketError(socket.addr, e))?,
            packet_models::Request::Announce(p) => {
                let addr = bin2addr(&p.addr).map_err(|e| NodeError::BinToAddressError(e))?;

                // verify address is not loopback
                if (addr.ip().is_loopback() && addr.port() == SERVER_ADDRESS.port())
                    || addr.ip().is_unspecified()
                {
                    let response_packet = packet_models::Packet::Error(packet_models::ErrorR {
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
            packet_models::Request::GetAmount(p) => {
                if p.address.len() != 33 {
                    socket
                        .send(packet_models::Packet::Error(packet_models::ErrorR {
                            code: packet_models::ErrorCode::BadBlockchainAddress,
                        }))
                        .await
                        .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
                    return Err(NodeError::BadBlockchainAddressSizeError(p.address.len()));
                }

                let address: [u8; 33] =
                    unsafe { p.to_owned().address.try_into().unwrap_unchecked() };

                let funds = match blockchain.get_funds(&address) {
                    Err(e) => {
                        socket
                            .send(packet_models::Packet::Error(packet_models::ErrorR {
                                code: packet_models::ErrorCode::UnexpectedInternalError,
                            }))
                            .await
                            .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;

                        return Err(NodeError::GetFundsError(e.to_string()));
                    }
                    Ok(funds) => funds,
                };

                let mut funds_dumped: Vec<u8> = Vec::new();
                if let Err(e) = blockchaintree::tools::dump_biguint(&funds, &mut funds_dumped) {
                    socket
                        .send(packet_models::Packet::Error(packet_models::ErrorR {
                            code: packet_models::ErrorCode::UnexpectedInternalError,
                        }))
                        .await
                        .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;

                    return Err(NodeError::GetFundsError(e.to_string()));
                };

                socket
                    .send(packet_models::Response::GetAmount(
                        packet_models::GetAmountResponse {
                            id: p.id,
                            amount: funds_dumped,
                        },
                    ))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::GetNodes(p) => {
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

                let packet = packet_models::Packet::Response(packet_models::Response::GetNodes(
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
            packet_models::Request::GetTransaction(p) => {}
            packet_models::Request::GetBlockByHash(p) => {
                let block_dump = match blockchain.get_main_chain().find_by_hash(&p.hash).await {
                    Err(e) => {
                        // socket
                        //     .send(packet_models::ErrorR {
                        //         code: packet_models::ErrorCode::UnexpectedInternalError,
                        //     })
                        //     .await
                        //     .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
                        return Err(NodeError::GetBlockError(e.to_string()));
                    }
                    Ok(Some(block)) => Some(
                        block
                            .dump()
                            .map_err(|e| NodeError::GetBlockError(e.to_string()))?,
                    ),
                    Ok(None) => None,
                };

                socket
                    .send(packet_models::Response::GetBlockByHash(
                        packet_models::GetBlockByHashResponse {
                            id: p.id,
                            dump: block_dump,
                        },
                    ))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
        },
        packet_models::Packet::Response(r) => {}
        packet_models::Packet::Error(e) => {
            error!("Node: {:?} returned error: {:?}", socket.addr, e);
            return Err(NodeError::RemoteNodeError(e.clone()));
        }
    }

    Ok(())
}

pub async fn connect_new_peers(
    shutdown: Sender<u8>,
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    propagate: Sender<packet_models::Packet>,
    new_peers_tx: Sender<SocketAddr>,
    blockchain: Arc<BlockChainTree>,
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
            new_peers_tx,
            blockchain
        ) => {}
    }
}

async fn connect_new_peers_wrapped(
    peers_mut: Arc<Mutex<HashSet<SocketAddr>>>,
    propagate: Sender<packet_models::Packet>,
    shutdown: Sender<u8>,
    new_peers_rx: &mut Receiver<SocketAddr>,
    new_peers_tx: Sender<SocketAddr>,
    blockchain: Arc<BlockChainTree>,
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
            blockchain.clone(),
        ));
    }
}
