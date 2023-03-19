#![allow(arithmetic_overflow)]
use std::collections::HashSet;
use std::net::SocketAddr;

use crate::config;
use crate::errors::node_errors::NodeError;
use tokio::sync::{
    broadcast::{Receiver, Sender},
    RwLock,
};

use crate::config::*;
use crate::encsocket::EncSocket;
use crate::errors::*;
use crate::models;
use crate::models::*;
use crate::tools;
use blockchaintree::blockchaintree::BlockChainTree;
use blockchaintree::transaction::Transactionable;
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::Duration;
#[allow(unused_imports)]
use tracing::{debug, error, info};

lazy_static! {
    static ref PEER_TIMEOUT: Duration = Duration::from_secs(15);
}

#[derive(Clone)]
pub struct NodeContext {
    pub peers: Arc<RwLock<HashSet<SocketAddr>>>,
    pub shutdown: Sender<u8>,
    pub propagate_packet: Sender<(u64, packet_models::Packet)>,
    pub new_peers_tx: Sender<SocketAddr>,
    pub blockchain: Arc<RwLock<BlockChainTree>>,
}

/// Start node, entry point
pub async fn start(context: NodeContext) -> Result<(), node_errors::NodeError> {
    let mut rx = context.shutdown.subscribe();

    tokio::select! {
        _ = connect_to_peers(
            context.clone()
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
                debug!("Recieved shutdown command");
                context.shutdown.send(0).unwrap();
                break;
            }
        };

        info!("New connection from: {}", addr);
        tokio::spawn(handle_incoming(sock, addr, context.clone()));
    }

    Ok(())
}

/// Handle incoming connection wrapper
async fn handle_incoming(
    socket: TcpStream,
    addr: SocketAddr,
    context: NodeContext,
) -> Result<(), node_errors::NodeError> {
    let mut rx = context.shutdown.subscribe();
    tokio::select! {
        res = handle_incoming_wrapped(
            socket,
            addr,
            context.propagate_packet,
            context.peers,
            context.new_peers_tx,
            context.blockchain) => {
                match res {
                    Err(e) => error!("Unexpected error on peer {}: {:?}", addr, e),
                    Ok(_) => {}
            }},
        _ = rx.recv() => {

        }
    }

    Ok(())
}

/// Wrapped main body of incoming connection handler
async fn handle_incoming_wrapped(
    socket: TcpStream,
    addr: SocketAddr,
    mut propagate: Sender<(u64, packet_models::Packet)>,
    peers: Arc<RwLock<HashSet<SocketAddr>>>,
    mut new_peers_tx: Sender<SocketAddr>,
    blockchain: Arc<RwLock<BlockChainTree>>,
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
        if let Err(e) = process_packet(
            &mut socket,
            &packet,
            &mut waiting_response,
            peers.clone(),
            &mut propagate,
            &mut new_peers_tx,
            &blockchain,
            tools::current_time(),
        )
        .await
        {
            debug!(
                "Error processing packet for the peer {}: {:?} ERROR: {:?}",
                addr, packet, e
            );
            break;
        }
    }

    Ok(())
}

async fn connect_to_peers(context: NodeContext) {
    let peers = context.peers.read().await;

    info!("Connecting to {} peers", peers.len());

    for peer in peers.iter() {
        tokio::spawn(connect_to_peer(*peer, context.clone()));
    }
}

pub async fn connect_to_peer(addr: SocketAddr, context: NodeContext) {
    debug!("Connecting to peer: {}", &addr);
    let mut rx = context.shutdown.subscribe();
    tokio::select! {
        _ = rx.recv() => {},
        ret = handle_peer(
            &addr,
            context.peers.clone(),
            context.propagate_packet,
            context.new_peers_tx,
            context.blockchain
        ) => {
            match ret {
                Err(e) => error!("Unexpected error on peer {}: {:?}", addr, e),
                Ok(_) => {}
            }
        }
    };

    // remove peer from active peers
    let mut peers = context.peers.write().await;
    peers.remove(&addr);
}

pub async fn handle_peer(
    addr: &SocketAddr,
    peers_mut: Arc<RwLock<HashSet<SocketAddr>>>,
    mut propagate: Sender<(u64, packet_models::Packet)>,
    mut new_peers_tx: Sender<SocketAddr>,
    blockchain: Arc<RwLock<BlockChainTree>>,
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
                let packet = propagate_message.map_err(|e| NodeError::PropagationReadError(*addr, e))?;
                socket.send(packet).await.map_err(|e| NodeError::SendPacketError(*addr, e))?;
                continue;
            },
            packet = socket.recv::<packet_models::Packet>() => {
                packet.map_err(|e| NodeError::ReceievePacketError(*addr, e))?
            }
        };

        // handle packet
        if let Err(e) = process_packet(
            &mut socket,
            &packet,
            &mut waiting_response,
            peers_mut.clone(),
            &mut propagate,
            &mut new_peers_tx,
            &blockchain,
            tools::current_time(),
        )
        .await
        {
            error!(
                "Error processing packet for the peer {}: {:?} | Error: {:?}",
                addr, packet, e
            );
            break;
        }
    }

    Ok(())
}

async fn process_packet(
    socket: &mut EncSocket,
    packet: &packet_models::Packet,
    waiting_response: &HashSet<u64>,
    peers_mut: Arc<RwLock<HashSet<SocketAddr>>>,
    propagate: &mut Sender<(u64, packet_models::Packet)>,
    new_peers_tx: &mut Sender<SocketAddr>,
    blockchain: &Arc<RwLock<BlockChainTree>>,
    recieved_timestamp: u64,
) -> Result<(), NodeError> {
    match packet {
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

                let mut peers = peers_mut.write().await;
                let peer_doesnt_exist = peers.insert(addr);
                drop(peers);

                if peer_doesnt_exist {
                    let mut packet_id: u64 = rand::random();
                    while waiting_response.get(&packet_id).is_some() {
                        packet_id = rand::random();
                    }

                    let mut request = p.clone();
                    request.id = packet_id;
                    let packet =
                        packet_models::Packet::Request(packet_models::Request::Announce(request));

                    propagate
                        .send((packet_id, packet))
                        .map_err(|e| NodeError::PropagationSendError(addr, e.to_string()))?;
                    new_peers_tx
                        .send(addr)
                        .map_err(|e| NodeError::PropagationSendError(addr, e.to_string()))?;
                }
            }
            packet_models::Request::GetAmount(p) => {
                let blockchain = blockchain.read().await;

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
                    .send(packet_models::Packet::Response(
                        packet_models::Response::GetAmount(packet_models::GetAmountResponse {
                            id: p.id,
                            amount: funds_dumped,
                        }),
                    ))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::GetNodes(p) => {
                let mut peers_cloned: Box<[SocketAddr]>;
                {
                    // clone peers into vec
                    let peers = peers_mut.read().await;
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
            packet_models::Request::GetTransaction(p) => {
                let blockchain = blockchain.read().await;
                let packet = packet_models::Response::GetTransaction(
                    packet_models::GetTransactionResponse {
                        id: p.id,
                        transaction: blockchain
                            .get_main_chain()
                            .find_transaction(&p.hash)
                            .await
                            .map_err(|e| NodeError::FindTransactionError(e.to_string()))?
                            .map(|tr| {
                                tr.dump()
                                    .map_err(|e| NodeError::FindTransactionError(e.to_string()))
                                    .unwrap() // TODO: remove unwrap
                            }),
                    },
                );

                socket
                    .send(packet)
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::GetBlockByHash(p) => {
                let blockchain = blockchain.read().await;
                let block_dump = match blockchain.get_main_chain().find_raw_by_hash(&p.hash).await {
                    Err(e) => {
                        // socket
                        //     .send(packet_models::ErrorR {
                        //         code: packet_models::ErrorCode::UnexpectedInternalError,
                        //     })
                        //     .await
                        //     .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
                        return Err(NodeError::GetBlockError(e.to_string()));
                    }
                    Ok(Some(block)) => Some(block),
                    Ok(None) => None,
                };

                socket
                    .send(packet_models::Response::GetBlock(
                        packet_models::GetBlockResponse {
                            id: p.id,
                            dump: block_dump,
                        },
                    ))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::GetBlockByHeight(p) => {
                let blockchain = blockchain.read().await;
                let block_dump = match blockchain
                    .get_main_chain()
                    .find_raw_by_height(p.height)
                    .await
                {
                    Err(e) => {
                        return Err(NodeError::GetBlockError(e.to_string()));
                    }
                    Ok(Some(block)) => Some(block),
                    Ok(None) => None,
                };
                socket
                    .send(packet_models::Response::GetBlock(
                        packet_models::GetBlockResponse {
                            id: p.id,
                            dump: block_dump,
                        },
                    ))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::GetBlocksByHeights(p) => {
                if p.amount as usize > config::MAX_BLOCKS_IN_RESPONSE {
                    return Err(NodeError::TooMuchBlocksError(
                        config::MAX_BLOCKS_IN_RESPONSE,
                    ));
                } else if p.amount == 0 {
                    socket
                        .send(packet_models::Packet::Response(
                            packet_models::Response::GetBlocks(packet_models::GetBlocksResponse {
                                id: p.id,
                                blocks: Vec::new(),
                            }),
                        ))
                        .await
                        .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
                    return Ok(());
                }

                let blockchain = blockchain.read().await;

                let chain = blockchain.get_main_chain();
                let height = chain.get_height().await;
                if height <= p.start {
                    return Err(NodeError::NotReachedHeightError(p.start as usize));
                }

                let amount = if p.start + p.amount > height {
                    height - p.start
                } else {
                    p.amount
                };

                let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(amount as usize);

                for height in p.start..p.start + amount {
                    if let Some(block) = chain
                        .find_raw_by_height(height)
                        .await
                        .map_err(|e| NodeError::GetBlockError(e.to_string()))?
                    {
                        blocks.push(block);
                    } else {
                        break;
                    }
                }

                socket
                    .send(packet_models::Packet::Response(
                        packet_models::Response::GetBlocks(packet_models::GetBlocksResponse {
                            id: p.id,
                            blocks,
                        }),
                    ))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::NewTransaction(p) => {
                let blockchain = blockchain.read().await;
                if p.transaction.len() < 4 {
                    return Err(NodeError::BadTransactionSizeError);
                }
                let transaction_size: u32 = u32::from_be_bytes(unsafe {
                    p.transaction[0..4].try_into().unwrap_unchecked()
                });
                if p.transaction.len() - 4 != transaction_size as usize {
                    return Err(NodeError::BadTransactionSizeError);
                }

                let transaction = blockchaintree::transaction::Transaction::parse(
                    &p.transaction[4..],
                    transaction_size as u64,
                )
                .map_err(|e| NodeError::ParseTransactionError(e.to_string()))?;

                // verify transaction
                let last_block = blockchain
                    .get_main_chain()
                    .get_last_block()
                    .await
                    .map_err(|e| NodeError::CreateTransactionError(e.to_string()))?;

                if let Some(last_block) = last_block {
                    let transaction_time = transaction.get_timestamp();
                    if transaction_time <= last_block.get_info().timestamp
                        || transaction_time > recieved_timestamp
                    {
                        return Err(NodeError::CreateTransactionError(
                            "Wrong transaction time".into(),
                        ));
                    }
                }

                if !transaction
                    .verify()
                    .map_err(|e| NodeError::CreateTransactionError(e.to_string()))?
                {
                    return Err(NodeError::CreateTransactionError("Bad signature".into()));
                }

                blockchain
                    .new_transaction(transaction)
                    .await
                    .map_err(|e| NodeError::CreateTransactionError(e.to_string()))?;

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                let mut request = p.clone();
                request.id = packet_id;
                let packet =
                    packet_models::Packet::Request(packet_models::Request::NewTransaction(request));

                propagate
                    .send((packet_id, packet))
                    .map_err(|e| NodeError::PropagationSendError(socket.addr, e.to_string()))?;

                socket
                    .send(packet_models::Response::Ok(packet_models::OkResponse {
                        id: p.id,
                    }))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::SubmitPow(p) => {
                todo!()
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

pub async fn connect_new_peers(context: NodeContext) {
    let mut shutdown_watcher = context.shutdown.subscribe();
    let mut new_peers_rx = context.new_peers_tx.subscribe();

    tokio::select! {
        _ = shutdown_watcher.recv() => {},
        _ = connect_new_peers_wrapped(
            &mut new_peers_rx,
            context
        ) => {}
    }
}

async fn connect_new_peers_wrapped(new_peers_rx: &mut Receiver<SocketAddr>, context: NodeContext) {
    loop {
        let peer_addr = match new_peers_rx.recv().await {
            Ok(p) => p,
            Err(_) => {
                continue;
            }
        };

        tokio::spawn(connect_to_peer(peer_addr, context.clone()));
    }
}
