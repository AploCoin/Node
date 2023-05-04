#![allow(arithmetic_overflow)]
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::config::*;
use crate::encsocket::EncSocket;
use crate::errors::node_errors::NodeError;
use crate::errors::*;
use crate::models;
use crate::models::*;
use crate::tools;
use crate::{config, main};
use blockchaintree::block::{self, MainChainBlockArc};
use blockchaintree::blockchaintree::BlockChainTree;
use blockchaintree::transaction::Transactionable;
use lazy_static::lazy_static;
use num_bigint::BigUint;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{
    broadcast::{Receiver, Sender},
    RwLock,
};
use tokio::time::Duration;
#[allow(unused_imports)]
use tracing::{debug, error, info};

lazy_static! {
    static ref PEER_TIMEOUT: Duration = Duration::from_secs(15);
}

#[derive(Clone)]
pub struct Approves {
    pub total_approves: usize,
    pub peers: HashSet<SocketAddr>,
    pub last_recieved: usize,
}

/// Holds new data passed to the node until several verifications
#[derive(Clone)]
pub struct NewData {
    pub new_blocks: HashMap<u64, Vec<MainChainBlockArc>>,
    pub blocks_approves: HashMap<[u8; 32], Approves>,
}

impl Default for NewData {
    fn default() -> Self {
        Self {
            new_blocks: Default::default(),
            blocks_approves: Default::default(),
        }
    }
}

impl NewData {
    // pub fn new() -> NewData {
    //     Default::default()
    // }

    /// Adds new block to the data
    ///
    /// if the block already exists increases amount of approves and returns false
    pub async fn new_block(
        &mut self,
        block: MainChainBlockArc,
        peer: &SocketAddr,
        time_recieved: usize,
    ) -> bool {
        //let mut blocks_approves = self.blocks_approves.write().await;

        //let mut new_blocks = self.new_blocks.write().await;

        let block_hash = block
            .hash()
            .map_err(|e| {
                error!("Failed to hash newly created block: {}", e.to_string());
                e
            })
            .unwrap(); // smth went horribly wrong, it's safer to crash

        let mut existed = true;
        self.blocks_approves
            .entry(block_hash)
            .and_modify(|approves| {
                if approves.peers.insert(*peer) {
                    approves.total_approves += 1;
                    approves.last_recieved = time_recieved;
                }
            })
            .or_insert_with(|| {
                debug!("The block with hash: {:?} didn't exist", block_hash);
                existed = false;
                Approves {
                    total_approves: 1,
                    peers: HashSet::from([*peer]),
                    last_recieved: time_recieved,
                }
            });

        if existed {
            return false;
        }

        self.new_blocks
            .entry(block.get_info().height)
            .and_modify(|blocks| blocks.push(block.clone()))
            .or_insert(vec![block]);

        true
    }

    pub fn get_blocks_same_height(&self, height: u64) -> Vec<MainChainBlockArc> {
        if let Some(blocks) = self.new_blocks.get(&height) {
            blocks.to_vec()
        } else {
            Vec::with_capacity(0)
        }
    }

    pub fn get_block_approves(&self, hash: &[u8; 32]) -> Option<&Approves> {
        self.blocks_approves.get(hash)
    }
}

#[derive(Clone)]
pub struct PropagatedPacket {
    packet: packet_models::Packet,
    source_addr: SocketAddr,
}

#[derive(Clone)]
pub struct NodeContext {
    pub peers: Arc<RwLock<HashSet<SocketAddr>>>,
    pub shutdown: Sender<u8>,
    pub propagate_packet: Sender<PropagatedPacket>,
    pub new_peers_tx: Sender<SocketAddr>,
    pub blockchain: Arc<BlockChainTree>,
    pub new_data: Arc<RwLock<NewData>>,
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
            &addr,
            context) => {
            // context.propagate_packet,
            // context.peers,
            // context.new_peers_tx,
            // context.blockchain) => {
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
    addr: &SocketAddr,
    context: NodeContext,
) -> Result<(), NodeError> {
    let mut rx_propagate = context.propagate_packet.subscribe();

    let mut waiting_response: HashSet<u64> = HashSet::with_capacity(20);

    let mut socket = EncSocket::new_connection(socket, *addr, *PEER_TIMEOUT)
        .await
        .map_err(|e| NodeError::ConnectToPeerError(*addr, e))?;

    // main loop
    loop {
        let packet = tokio::select! {
            propagate_message = rx_propagate.recv() => {
                let propagate_data = propagate_message.map_err(|e| NodeError::PropagationReadError(*addr, e))?;
                if propagate_data.source_addr == *addr{
                    continue;
                }
                socket.send( propagate_data.packet).await.map_err(|e| NodeError::SendPacketError(*addr, e))?;
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
            tools::current_time(),
            &context,
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
    let peers = context.peers.clone();

    let mut rx = context.shutdown.subscribe();
    tokio::select! {
        _ = rx.recv() => {},
        ret = handle_peer(
            &addr,
            context
        ) => {
            match ret {
                Err(e) => error!("Unexpected error on peer {}: {:?}", addr, e),
                Ok(_) => {}
            }
        }
    };

    // remove peer from active peers
    let mut peers = peers.write().await;
    peers.remove(&addr);
}

pub async fn handle_peer(addr: &SocketAddr, context: NodeContext) -> Result<(), NodeError> {
    // set up
    let mut rx_propagate = context.propagate_packet.subscribe();

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
                let propagate_data = propagate_message.map_err(|e| NodeError::PropagationReadError(*addr, e))?;
                if propagate_data.source_addr == *addr{
                    continue;
                }
                socket.send(propagate_data.packet).await.map_err(|e| NodeError::SendPacketError(*addr, e))?;
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
            tools::current_time(),
            &context,
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
    recieved_timestamp: u64,
    context: &NodeContext,
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

                let mut peers = context.peers.write().await;
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

                    context
                        .propagate_packet
                        .send(PropagatedPacket {
                            packet,
                            source_addr: socket.addr,
                        })
                        .map_err(|e| NodeError::PropagationSendError(addr, e.to_string()))?;
                    context
                        .new_peers_tx
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

                let funds = match context.blockchain.get_funds(&address).await {
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
                    let peers = context.peers.read().await;
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
                let packet = packet_models::Response::GetTransaction(
                    packet_models::GetTransactionResponse {
                        id: p.id,
                        transaction: context
                            .blockchain
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
                let block_dump = match context
                    .blockchain
                    .get_main_chain()
                    .find_raw_by_hash(&p.hash)
                    .await
                {
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
                let block_dump = match context
                    .blockchain
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

                let chain = context.blockchain.get_main_chain();
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

                // check if the transaction is with root as source
                if transaction
                    .get_sender()
                    .eq(&blockchaintree::blockchaintree::ROOT_PUBLIC_ADDRESS)
                {
                    return Err(NodeError::SendFundsFromRootError);
                }

                // verify transaction
                let last_block = context
                    .blockchain
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

                context
                    .blockchain
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

                context
                    .propagate_packet
                    .send(PropagatedPacket {
                        packet,
                        source_addr: socket.addr,
                    })
                    .map_err(|e| NodeError::PropagationSendError(socket.addr, e.to_string()))?;

                socket
                    .send(packet_models::Response::Ok(packet_models::OkResponse {
                        id: p.id,
                    }))
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::SubmitPow(p) => {
                if p.address.len() != 33 {
                    return Err(NodeError::WrongAddressSizeError(p.address.len()));
                }
                if p.timestamp > recieved_timestamp {
                    return Err(NodeError::TimestampInFutureError(
                        p.timestamp,
                        recieved_timestamp,
                    ));
                } else if recieved_timestamp - p.timestamp > MAX_POW_SUBMIT_DELAY {
                    return Err(NodeError::TimestampExpiredError);
                }
                let pow = BigUint::from_bytes_be(&p.pow);

                // cannot fail
                let address: [u8; 33] = unsafe { p.address.clone().try_into().unwrap_unchecked() };

                let new_block = unsafe {
                    Arc::from_raw(Box::into_raw(
                        context
                            .blockchain
                            .emit_main_chain_block(pow, address, p.timestamp)
                            .await
                            .map_err(|e| NodeError::EmitMainChainBlockError(e.to_string()))?,
                    ))
                };

                context
                    .new_data
                    .write()
                    .await
                    .new_block(new_block.clone(), &socket.addr, recieved_timestamp as usize)
                    .await;

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                let block_transaction_hashes = new_block.get_transactions();
                let mut dumped_transactions: Vec<u8> = Vec::with_capacity(0);
                let main_chain = context.blockchain.get_main_chain();
                for transaction_hash in block_transaction_hashes.iter() {
                    let dump = main_chain
                        .find_transaction_raw(transaction_hash)
                        .await
                        .map_err(|e| {
                            error!(
                                "Error finding transaction {:?}, error: {:?}",
                                transaction_hash, e
                            );
                        })
                        .unwrap()
                        .ok_or_else(|| {
                            error!(
                                "Error finding transaction {:?}, fatal error",
                                transaction_hash
                            );
                            NodeError::FindTransactionError("No such transaction in db".into())
                        })
                        .unwrap();

                    dumped_transactions.reserve(8 + dump.len());

                    dumped_transactions.extend((dump.len() as u64).to_be_bytes());
                    dumped_transactions.extend(dump.iter());
                }

                let block_packet = packet_models::Packet::Request(
                    packet_models::Request::NewBlock(packet_models::NewBlockRequest {
                        id: packet_id,
                        dump: new_block
                            .dump()
                            .map_err(|e| {
                                error!("Error dumping new block, fatal error");
                                e
                            })
                            .unwrap(),
                        transactions: dumped_transactions,
                    }),
                );
                context
                    .propagate_packet
                    .send(PropagatedPacket {
                        packet: block_packet,
                        source_addr: socket.addr,
                    })
                    .map_err(|e| NodeError::PropagationSendError(socket.addr, e.to_string()))?;

                let response_packet = packet_models::Packet::Response(
                    packet_models::Response::SubmitPow(packet_models::SubmitPowResponse {
                        id: p.id,
                        accepted: true,
                    }),
                );

                socket
                    .send(response_packet)
                    .await
                    .map_err(|e| NodeError::SendPacketError(socket.addr, e))?;
            }
            packet_models::Request::NewBlock(p) => {
                let recieved_block = block::deserialize_main_chain_block(&p.dump)
                    .map_err(|e| NodeError::ParseBlockError(e.to_string()))?;

                let recieved_block_arc = unsafe { Arc::from_raw(Box::into_raw(recieved_block)) };

                let mut new_data = context.new_data.write().await;

                if !context
                    .blockchain
                    .new_main_chain_block(&recieved_block_arc)
                    .await
                    .map_err(|e| NodeError::AddMainChainBlockError(e.to_string()))?
                {
                    // diverging data
                    // get already existing block from the chain
                    let existing_block = unsafe {
                        Arc::from_raw(Box::into_raw(
                            context
                                .blockchain
                                .get_main_chain()
                                .find_by_height(recieved_block_arc.get_info().height)
                                .await
                                .map_err(|e| NodeError::GetBlockError(e.to_string()))?
                                .ok_or(NodeError::GetBlockError(
                                    "Couldn't find block with same height".into(),
                                ))?,
                        ))
                    };
                    // add block from the chain to the new data
                    new_data
                        .new_block(
                            existing_block,
                            &SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0),
                            recieved_timestamp as usize,
                        )
                        .await;
                }

                if !new_data
                    .new_block(
                        recieved_block_arc.clone(),
                        &socket.addr,
                        recieved_timestamp as usize,
                    )
                    .await
                {
                    // block was already in new data
                    let mut blocks_same_height =
                        new_data.get_blocks_same_height(recieved_block_arc.get_info().height);

                    // sort ascending with approves
                    blocks_same_height.sort_by(|a, b| {
                        let a_approves = new_data.get_block_approves(&a.hash().unwrap()).unwrap(); // critical section, stop app if error
                        let b_approves = new_data.get_block_approves(&b.hash().unwrap()).unwrap(); // critical section, stop app if error

                        match a_approves.total_approves.cmp(&b_approves.total_approves) {
                            Ordering::Greater => Ordering::Greater,
                            Ordering::Less => Ordering::Less,
                            Ordering::Equal => {
                                a_approves.last_recieved.cmp(&b_approves.last_recieved)
                            } // compare timestamps
                        }
                    });

                    let max_block = blocks_same_height.last().unwrap(); // critical error
                    match blocks_same_height.len() {
                        1 => {
                            let approves = new_data
                                .get_block_approves(&max_block.hash().unwrap())
                                .unwrap(); // critical error
                            if tools::current_time() as usize - approves.last_recieved
                                >= config::MIN_BLOCK_APPROVE_TIME
                            {
                                //context.blockchain.overwrite_main_chain_block(new_block, transactions)
                            }
                        }
                        _ => {}
                    }
                }
            }
        },
        packet_models::Packet::Response(r) => {}
        packet_models::Packet::Error(e) => {
            //error!("Node: {:?} returned error: {:?}", socket.addr, e);
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
