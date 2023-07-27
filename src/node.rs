#![allow(arithmetic_overflow)]
use std::collections::HashSet;
use std::net::{SocketAddr, SocketAddrV4};
use std::str::FromStr;

use crate::config::*;
use crate::encsocket::{EncSocket, ReadHalf, WriteHalf};
use crate::errors::node_errors::NodeError;
use crate::errors::*;
use crate::handlers::*;
use crate::models;
use crate::models::*;
use crate::newdata::*;
use crate::tools::{self, current_time};
use async_channel::{Receiver as ACReceiver, Sender as ACSender};
use blockchaintree::block::MainChainBlock;
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::Receiver;
use tokio::time::{sleep, Duration};
#[allow(unused_imports)]
use tracing::{debug, error, info};

lazy_static! {
    static ref PEER_TIMEOUT: Duration = Duration::from_secs(30);
}

pub async fn update_blockchain_wrapped(context: NodeContext) {
    loop {
        sleep(Duration::from_secs(MIN_BLOCK_APPROVE_TIME as u64)).await;
        info!("Updating blockchain");

        debug!("Propagating a packet to get new blocks");
        let height = context.blockchain.get_main_chain().get_height().await;
        if let Some(e) = context
            .propagate_packet
            .send(ReceivedPacket {
                packet: packet_models::Packet::Request {
                    id: 0,
                    data: packet_models::Request::GetBlocksByHeights(
                        packet_models::GetBlocksByHeightsRequest {
                            start: height,
                            amount: 1 as u64, //MAX_BLOCKS_SYNC_AMOUNT as u64,
                        },
                    ),
                },
                source_addr: SocketAddr::V4(unsafe {
                    SocketAddrV4::from_str("0.0.0.0:0").unwrap_unchecked()
                }),
                timestamp: current_time(),
            })
            .err()
        {
            error!("Error propagating packet {:?}", e);
        }

        {
            let mut new_data = context.new_data.write().await;
            let mut max_height: u64 = 0;

            for height in new_data.new_blocks.keys() {
                if *height > max_height {
                    max_height = *height;
                }
            }

            if max_height == 0 {
                // new data is empty
                info!("No data to update in blockchain");
                continue;
            }

            let blocks = new_data.get_blocks_same_height(max_height);

            debug!("Found {} blocks in new data", blocks.len());

            let mut max_approves = Approves {
                total_approves: 0,
                peers: Default::default(),
                last_received: 0,
            };

            let mut max_block: Option<Arc<dyn MainChainBlock + Send + Sync>> = None;

            for block in blocks {
                let approves = new_data.get_block_approves(&block.hash().unwrap()).unwrap();
                if approves.total_approves > max_approves.total_approves {
                    max_approves = approves.clone();
                    max_block = Some(block);
                }
            }

            let max_block = max_block.unwrap();

            if tools::current_time() as usize - max_approves.last_received >= MIN_BLOCK_APPROVE_TIME
            {
                info!("Found old enough block to update blockchain");
                context
                    .blockchain
                    .overwrite_main_chain_block(
                        &max_block,
                        new_data
                            .transactions
                            .get(&max_block.hash().unwrap())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                let height = max_block.get_info().height;

                if let Some(e) = context
                    .propagate_packet
                    .send(ReceivedPacket {
                        packet: packet_models::Packet::Request {
                            id: 0,
                            data: packet_models::Request::GetBlocksByHeights(
                                packet_models::GetBlocksByHeightsRequest {
                                    start: height + 1,
                                    amount: MAX_BLOCKS_SYNC_AMOUNT as u64,
                                },
                            ),
                        },
                        source_addr: SocketAddr::V4(unsafe {
                            SocketAddrV4::from_str("0.0.0.0:0").unwrap_unchecked()
                        }),
                        timestamp: current_time(),
                    })
                    .err()
                {
                    error!("Error propagating packet {:?}", e);
                }

                debug!("Removing blocks from a new info");
                new_data.remove_blocks(max_block.get_info().height);
            }
        }
    }
}

pub async fn update_blockchain(context: NodeContext) {
    let mut shutdown_receiver = context.shutdown.subscribe();

    tokio::select! {
        _ = update_blockchain_wrapped(context) => {},
        _ = shutdown_receiver.recv() => {}
    }
}

/// Start node, entry point
pub async fn start(context: NodeContext) -> Result<(), node_errors::NodeError> {
    let mut rx = context.shutdown.subscribe();

    // tokio::select! {
    //     _ = connect_to_peers(
    //         context.clone()
    //     ) => {},
    //     _ = rx.recv() => {
    //         return Ok(());
    //     }
    // };

    let listener = TcpListener::bind(*SERVER_ADDRESS)
        .await
        .map_err(NodeError::BindSocket)?;

    loop {
        let (sock, addr) = tokio::select! {
            res = listener.accept() => {
                res.map_err( NodeError::AcceptConnection)?
            },
            _ = rx.recv() => {
                debug!("received shutdown command");
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
            &addr,
            context) => {
                if let Err(e) = res { error!("Unexpected error on peer {}: {:?}", addr, e) }},
        _ = rx.recv() => {

        }
    }

    Ok(())
}

/// Wrapped main body of incoming connection handler
async fn handle_incoming_wrapped(
    socket: TcpStream,
    addr: &SocketAddr,
    context: NodeContext,
) -> Result<(), NodeError> {
    let mut waiting_response: HashSet<u64> = HashSet::with_capacity(20);

    let mut socket = EncSocket::new_connection(socket, *addr, *PEER_TIMEOUT)
        .await
        .map_err(|e| NodeError::ConnectToPeer(*addr, e))?;

    let packet = packet_models::Packet::Request {
        id: 0,
        data: packet_models::Request::GetNodes(packet_models::GetNodesRequest {}),
    };
    socket
        .send(packet)
        .await
        .map_err(|e| NodeError::SendPacket(*addr, e))?;
    waiting_response.insert(0);

    let (socket, write_socket) = socket.split();
    let (send_new_packet, receive_new_packet) = async_channel::unbounded::<ReceivedPacket>();

    //context.peers.write().await.insert(addr.clone());

    tokio::select! {
        result = main_peer_loop(context.clone(), socket, send_new_packet ) => result,
        result = peer_worker(context.clone(), write_socket, waiting_response, receive_new_packet) => result
    }?;

    Ok(())
}

pub async fn connect_to_peer(addr: SocketAddr, context: NodeContext) {
    debug!("Connecting to peer: {}", &addr);
    let peers = context.peers.clone();

    let mut rx = context.shutdown.subscribe();
    tokio::select! {
        _ = rx.recv() => {
            return;
        },
        ret = handle_peer(
            &addr,
            &context
        ) => {
            if let Err(e) = ret { error!("Unexpected error on peer {}: {:?}", addr, e) }
        }
    };

    // remove peer from active peers
    let mut peers = peers.write().await;
    peers.remove(&addr);
}

pub async fn handle_peer(addr: &SocketAddr, context: &NodeContext) -> Result<(), NodeError> {
    let mut socket = EncSocket::create_new_connection(*addr, *PEER_TIMEOUT)
        .await
        .map_err(|e| NodeError::ConnectToPeer(*addr, e))?;

    let mut waiting_response: HashSet<u64> = HashSet::with_capacity(20);

    // announce
    let id: u64 = rand::random();

    let body = models::addr2bin(&ANNOUNCE_ADDRESS);
    let packet = packet_models::Packet::Request {
        id,
        data: packet_models::Request::Announce(packet_models::AnnounceRequest { addr: body }),
    };

    socket
        .send(packet)
        .await
        .map_err(|e| NodeError::SendPacket(*addr, e))?;
    waiting_response.insert(id);

    let packet = packet_models::Packet::Request {
        id: 0,
        data: packet_models::Request::GetNodes(packet_models::GetNodesRequest {}),
    };
    socket
        .send(packet)
        .await
        .map_err(|e| NodeError::SendPacket(*addr, e))?;
    waiting_response.insert(0);

    let (socket, write_socket) = socket.split();
    let (send_new_packet, receive_new_packet) = async_channel::unbounded::<ReceivedPacket>();

    context.peers.write().await.insert(addr.clone());

    tokio::select! {
        result = main_peer_loop(context.clone(), socket, send_new_packet ) => result,
        result = peer_worker(context.clone(), write_socket, waiting_response, receive_new_packet) => result
    }?;

    Ok(())
}

async fn main_peer_loop(
    _context: NodeContext,
    mut socket: ReadHalf,
    send_new_packet: ACSender<ReceivedPacket>,
) -> Result<(), NodeError> {
    loop {
        let packet = socket
            .recv::<packet_models::Packet>()
            .await
            .map_err(|e| NodeError::ReceievePacket(socket.addr, e))?;

        send_new_packet
            .send(ReceivedPacket {
                packet,
                source_addr: socket.addr,
                timestamp: tools::current_time(),
            })
            .await
            .unwrap();
    }
}

pub async fn peer_worker(
    context: NodeContext,
    mut socket: WriteHalf,
    mut waiting_response: HashSet<u64>,
    received_packet_channel: ACReceiver<ReceivedPacket>,
) -> Result<(), NodeError> {
    let mut propagated_packet_rx = context.propagate_packet.subscribe();
    loop {
        let packet = tokio::select! {
            packet = received_packet_channel.recv() => packet.unwrap(),

            propagated_packet = propagated_packet_rx.recv() => {
                let mut packet = propagated_packet.unwrap();
                if packet.source_addr == socket.addr{
                    continue;
                }

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                packet.packet.set_id(packet_id);
                waiting_response.insert(packet_id);
                socket.send(packet.packet).await.map_err(|e| NodeError::SendPacket(socket.addr, e))?;
                continue;
            }
        };

        process_packet(
            &mut socket,
            &packet.packet,
            &mut waiting_response,
            packet.timestamp,
            &context,
        )
        .await?;
    }
}

async fn process_packet(
    socket: &mut WriteHalf,
    packet: &packet_models::Packet,
    waiting_response: &mut HashSet<u64>,
    received_timestamp: u64,
    context: &NodeContext,
) -> Result<(), NodeError> {
    match packet {
        packet_models::Packet::Request {
            id: received_id,
            data: r,
        } => match r {
            packet_models::Request::Ping(_) => socket
                .send(packet_models::Packet::Response {
                    id: *received_id,
                    data: packet_models::Response::Ping(packet_models::PingResponse {}),
                })
                .await
                .map_err(|e| NodeError::SendPacket(socket.addr, e))?,
            packet_models::Request::Announce(p) => {
                announce_request_handler(context, waiting_response, socket, p).await?;
            }
            packet_models::Request::GetAmount(p) => {
                get_amount_request_handler(context, socket, p, *received_id).await?;
            }
            packet_models::Request::GetNodes(_) => {
                get_nodes_request_handler(context, socket, *received_id).await?;
            }
            packet_models::Request::GetTransaction(p) => {
                get_transaction_request_handler(context, socket, *received_id, p).await?;
            }
            packet_models::Request::GetBlockByHash(p) => {
                get_block_by_hash_request_handler(context, socket, *received_id, p).await?;
            }
            packet_models::Request::GetBlockByHeight(p) => {
                get_block_by_height_request_handler(context, socket, *received_id, p).await?;
            }
            packet_models::Request::GetBlocksByHeights(p) => {
                get_blocks_by_height(context, socket, *received_id, p).await?;
            }
            packet_models::Request::GetLastBlock(_) => {
                get_last_block_request_handler(context, socket, *received_id).await?;
            }
            packet_models::Request::NewTransaction(p) => {
                new_transaction_request_handler(
                    context,
                    socket,
                    *received_id,
                    received_timestamp,
                    waiting_response,
                    p,
                )
                .await?;
            }
            packet_models::Request::SubmitPow(p) => {
                submit_pow_request_handler(
                    context,
                    socket,
                    *received_id,
                    received_timestamp,
                    waiting_response,
                    p,
                )
                .await?;
            }
            packet_models::Request::NewBlock(p) => {
                new_block_request_handler(
                    context,
                    socket,
                    *received_id,
                    received_timestamp,
                    waiting_response,
                    p,
                )
                .await?;
            }
        },
        packet_models::Packet::Response {
            id: received_id,
            data: r,
        } => {
            if !waiting_response.remove(received_id) {
                socket
                    .send(&packet_models::Packet::Error(packet_models::ErrorR {
                        code: packet_models::ErrorCode::UnexpectedResponseId,
                    }))
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            match r {
                packet_models::Response::Ok(_) => {}
                packet_models::Response::GetNodes(p) => {
                    get_nodes_response_handler(context, p).await?;
                }
                packet_models::Response::GetAmount(_) => todo!(),
                packet_models::Response::GetTransaction(_) => todo!(),
                packet_models::Response::Ping(_) => {}
                packet_models::Response::GetBlock(_) => todo!(),
                packet_models::Response::GetBlocks(p) => {
                    get_blocks_response_handler(context, socket, received_timestamp, p).await?;
                }
                packet_models::Response::SubmitPow(_) => todo!(),
            }
        }
        packet_models::Packet::Error(e) => {
            //error!("Node: {:?} returned error: {:?}", socket.addr, e);
            return Err(NodeError::RemoteNode(e.clone()));
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
