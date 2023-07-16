#![allow(arithmetic_overflow)]
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use crate::config;
use crate::config::*;
use crate::encsocket::EncSocket;
use crate::errors::node_errors::NodeError;
use crate::errors::*;
use crate::models;
use crate::models::*;
use crate::tools;
use blockchaintree::block::{self, MainChainBlock, MainChainBlockArc};
use blockchaintree::blockchaintree::BlockChainTree;
use blockchaintree::transaction::{Transaction, Transactionable};
use lazy_static::lazy_static;
use num_bigint::BigUint;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{
    broadcast::{Receiver, Sender},
    RwLock,
};
use tokio::time::{sleep, Duration};
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
#[derive(Clone, Default)]
pub struct NewData {
    pub new_blocks: HashMap<u64, Vec<MainChainBlockArc>>,
    pub blocks_approves: HashMap<[u8; 32], Approves>,
    pub transactions: HashMap<[u8; 32], Vec<Transaction>>,
}

// impl Default for NewData {
//     fn default() -> Self {
//         Self {
//             new_blocks: Default::default(),
//             blocks_approves: Default::default(),
//             transactions: Default::default(),
//         }
//     }
// }

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
        transactions: &[Transaction],
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

        self.transactions
            .insert(block_hash, transactions.to_owned());

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

    /// Remove all block intries, including transactions and approves
    pub fn remove_blocks(&mut self, height: u64) {
        let blocks = match self.new_blocks.get(&height) {
            Some(block) => block,
            None => {
                return;
            }
        };
        for block in blocks {
            let hash = block.hash().unwrap();
            self.blocks_approves.remove(&hash);
            for tr in block.get_transactions().iter() {
                self.transactions.remove(tr);
            }
        }
    }

    pub fn clear(&mut self) {
        self.blocks_approves.clear();
        self.new_blocks.clear();
    }
}

#[derive(Clone, Debug)]
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

pub async fn update_blockchain_wrapped(context: NodeContext) {
    loop {
        sleep(Duration::from_secs(MIN_BLOCK_APPROVE_TIME as u64)).await;
        info!("Updating blockchain");
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
                last_recieved: 0,
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

            if tools::current_time() as usize - max_approves.last_recieved >= MIN_BLOCK_APPROVE_TIME
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
                    .send(PropagatedPacket {
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
    let mut shutdown_reciever = context.shutdown.subscribe();

    tokio::select! {
        _ = update_blockchain_wrapped(context) => {},
        _ = shutdown_reciever.recv() => {}
    }
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
        .map_err(NodeError::BindSocket)?;

    loop {
        let (sock, addr) = tokio::select! {
            res = listener.accept() => {
                res.map_err( NodeError::AcceptConnection)?
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
    let mut rx_propagate = context.propagate_packet.subscribe();

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

    // main loop
    loop {
        let packet = tokio::select! {
            propagate_message = rx_propagate.recv() => {
                let mut propagate_data = propagate_message.map_err(|e| NodeError::PropagationRead(*addr, e))?;
                if propagate_data.source_addr == *addr{
                    continue;
                }

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                propagate_data.packet.set_id(packet_id);
                waiting_response.insert(packet_id);

                socket.send(propagate_data.packet).await.map_err(|e| NodeError::SendPacket(*addr, e))?;
                continue;
            },
            packet = socket.recv::<packet_models::Packet>() => {
                packet.map_err(|e| NodeError::ReceievePacket(*addr, e))?
            },
            _ = sleep(Duration::from_millis(5000)) => {
                let packet_id: u64 = rand::random();
                socket.send(
                    packet_models::Packet::Request{id:packet_id,data:packet_models::Request::Ping(packet_models::PingRequest{})}).await.map_err(|e| NodeError::SendPacket(*addr, e))?;
                waiting_response.insert(packet_id);
                continue;
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
            if let Err(e) = ret { error!("Unexpected error on peer {}: {:?}", addr, e) }
        }
    };

    // remove peer from active peers
    let mut peers = peers.write().await;
    peers.remove(&addr);
}

pub async fn handle_peer(addr: &SocketAddr, context: NodeContext) -> Result<(), NodeError> {
    // set up
    let mut rx_propagate = context.propagate_packet.subscribe();

    let mut socket = EncSocket::create_new_connection(*addr, *PEER_TIMEOUT)
        .await
        .map_err(|e| NodeError::ConnectToPeer(*addr, e))?;

    let mut waiting_response: HashSet<u64> = HashSet::with_capacity(20);

    // announce
    let id: u64 = rand::random();

    let body = models::addr2bin(&SERVER_ADDRESS);
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

    // main loop
    loop {
        let packet = tokio::select! {
            propagate_message = rx_propagate.recv() => {
                let mut propagate_data = propagate_message.map_err(|e| NodeError::PropagationRead(*addr, e))?;
                if propagate_data.source_addr == *addr{
                    continue;
                }

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                propagate_data.packet.set_id(packet_id);

                waiting_response.insert(packet_id);

                socket.send(propagate_data.packet).await.map_err(|e| NodeError::SendPacket(*addr, e))?;
                continue;
            },
            packet = socket.recv::<packet_models::Packet>() => {
                packet.map_err(|e| NodeError::ReceievePacket(*addr, e))?
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

async fn new_block(
    recieved_block: Arc<dyn MainChainBlock + Send + Sync>,
    transaction_dumps: &[u8],
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_timestamp: u64,
) -> Result<(), NodeError> {
    let transactions = tools::deserialize_transactions(transaction_dumps)?;

    let mut new_data = context.new_data.write().await;

    if !context
        .blockchain
        .new_main_chain_block(&recieved_block)
        .await
        .map_err(|e| NodeError::AddMainChainBlock(e.to_string()))?
    {
        // diverging data
        // get already existing block from the chain
        let existing_block = context
            .blockchain
            .get_main_chain()
            .find_by_height(recieved_block.get_info().height)
            .await
            .map_err(|e| NodeError::GetBlock(e.to_string()))?
            .ok_or(NodeError::GetBlock(
                "Couldn't find block with same height".into(),
            ))?;

        let main_chain = context.blockchain.get_main_chain();

        let mut transactions: Vec<Transaction> =
            Vec::with_capacity(existing_block.get_transactions().len());
        for tr_hash in existing_block.get_transactions().iter() {
            let transaction = main_chain
                .find_transaction(tr_hash)
                .await
                .map_err(|e| NodeError::FindTransaction(e.to_string()))?
                .ok_or(NodeError::FindTransaction(format!(
                    "Transaction with hash {tr_hash:?} not found"
                )))?;
            transactions.push(transaction);
        }

        // add block from the chain to the new data
        new_data
            .new_block(
                existing_block,
                &transactions,
                &SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0),
                recieved_timestamp as usize,
            )
            .await;
    }

    if !new_data
        .new_block(
            recieved_block.clone(),
            &transactions,
            &socket.addr,
            recieved_timestamp as usize,
        )
        .await
    {
        // block was already in new data
        let mut blocks_same_height =
            new_data.get_blocks_same_height(recieved_block.get_info().height);

        // sort ascending with approves
        blocks_same_height.sort_by(|a, b| {
            let a_approves = new_data.get_block_approves(&a.hash().unwrap()).unwrap(); // critical section, stop app if error
            let b_approves = new_data.get_block_approves(&b.hash().unwrap()).unwrap(); // critical section, stop app if error

            match a_approves.total_approves.cmp(&b_approves.total_approves) {
                Ordering::Greater => Ordering::Greater,
                Ordering::Less => Ordering::Less,
                Ordering::Equal => a_approves.last_recieved.cmp(&b_approves.last_recieved), // compare timestamps
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
                    context
                        .blockchain
                        .overwrite_main_chain_block(&recieved_block, &transactions)
                        .await
                        .map_err(|e| NodeError::AddMainChainBlock(e.to_string()))?;
                }
            }
            _ => {
                let next_block = unsafe { blocks_same_height.get_unchecked(1) };
                let max_block_approves = new_data
                    .get_block_approves(&max_block.hash().unwrap())
                    .unwrap(); // critical error
                let next_block_approves = new_data
                    .get_block_approves(&next_block.hash().unwrap())
                    .unwrap(); // critical error

                if max_block_approves.total_approves >= next_block_approves.total_approves * 2
                    || tools::current_time() as usize - max_block_approves.last_recieved
                        > MIN_BLOCK_APPROVE_TIME
                {
                    context
                        .blockchain
                        .overwrite_main_chain_block(max_block, &transactions)
                        .await
                        .map_err(|e| NodeError::AddMainChainBlock(e.to_string()))?;

                    new_data.clear();
                }
            }
        }
    }

    Ok(())
}

async fn process_packet(
    socket: &mut EncSocket,
    packet: &packet_models::Packet,
    waiting_response: &mut HashSet<u64>,
    recieved_timestamp: u64,
    context: &NodeContext,
) -> Result<(), NodeError> {
    match packet {
        packet_models::Packet::Request {
            id: recieved_id,
            data: r,
        } => match r {
            packet_models::Request::Ping(_) => socket
                .send(packet_models::Packet::Response {
                    id: *recieved_id,
                    data: packet_models::Response::Ping(packet_models::PingResponse {}),
                })
                .await
                .map_err(|e| NodeError::SendPacket(socket.addr, e))?,
            packet_models::Request::Announce(p) => {
                let addr = bin2addr(&p.addr).map_err(NodeError::BinToAddress)?;

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
                        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

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

                    let request = p.clone();
                    let packet = packet_models::Packet::Request {
                        id: packet_id,
                        data: packet_models::Request::Announce(request),
                    };

                    context
                        .propagate_packet
                        .send(PropagatedPacket {
                            packet,
                            source_addr: socket.addr,
                        })
                        .map_err(|e| NodeError::PropagationSend(addr, e.to_string()))?;
                    context
                        .new_peers_tx
                        .send(addr)
                        .map_err(|e| NodeError::PropagationSend(addr, e.to_string()))?;
                }
            }
            packet_models::Request::GetAmount(p) => {
                if p.address.len() != 33 {
                    socket
                        .send(packet_models::Packet::Error(packet_models::ErrorR {
                            code: packet_models::ErrorCode::BadBlockchainAddress,
                        }))
                        .await
                        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
                    return Err(NodeError::BadBlockchainAddressSize(p.address.len()));
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
                            .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

                        return Err(NodeError::GetFunds(e.to_string()));
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
                        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

                    return Err(NodeError::GetFunds(e.to_string()));
                };

                socket
                    .send(packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::GetAmount(
                            packet_models::GetAmountResponse {
                                amount: funds_dumped,
                            },
                        ),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::GetNodes(_) => {
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

                let packet = packet_models::Packet::Response {
                    id: *recieved_id,
                    data: packet_models::Response::GetNodes(packet_models::GetNodesReponse {
                        ipv4,
                        ipv6,
                    }),
                };

                socket
                    .send(packet)
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?
            }
            packet_models::Request::GetTransaction(p) => {
                let packet = packet_models::Packet::Response {
                    id: *recieved_id,
                    data: packet_models::Response::GetTransaction(
                        packet_models::GetTransactionResponse {
                            transaction: context
                                .blockchain
                                .get_main_chain()
                                .find_transaction(&p.hash)
                                .await
                                .map_err(|e| NodeError::FindTransaction(e.to_string()))?
                                .map(|tr| {
                                    tr.dump()
                                        .map_err(|e| NodeError::FindTransaction(e.to_string()))
                                        .unwrap() // TODO: remove unwrap
                                }),
                        },
                    ),
                };

                socket
                    .send(packet)
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
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
                        return Err(NodeError::GetBlock(e.to_string()));
                    }
                    Ok(Some(block)) => Some(block),
                    Ok(None) => None,
                };

                socket
                    .send(packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::GetBlock(packet_models::GetBlockResponse {
                            dump: block_dump,
                        }),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::GetBlockByHeight(p) => {
                let block_dump = match context
                    .blockchain
                    .get_main_chain()
                    .find_raw_by_height(p.height)
                    .await
                {
                    Err(e) => {
                        return Err(NodeError::GetBlock(e.to_string()));
                    }
                    Ok(Some(block)) => Some(block),
                    Ok(None) => None,
                };
                socket
                    .send(packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::GetBlock(packet_models::GetBlockResponse {
                            dump: block_dump,
                        }),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::GetBlocksByHeights(p) => {
                if p.amount as usize > config::MAX_BLOCKS_IN_RESPONSE {
                    return Err(NodeError::TooMuchBlocks(config::MAX_BLOCKS_IN_RESPONSE));
                } else if p.amount == 0 {
                    socket
                        .send(packet_models::Packet::Response {
                            id: *recieved_id,
                            data: packet_models::Response::GetBlocks(
                                packet_models::GetBlocksResponse {
                                    blocks: Vec::new(),
                                    transactions: Vec::new(),
                                },
                            ),
                        })
                        .await
                        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
                    return Ok(());
                }

                let chain = context.blockchain.get_main_chain();
                let height = chain.get_height().await;
                if height <= p.start {
                    return Err(NodeError::NotReachedHeight(p.start as usize));
                }

                let amount = if p.start + p.amount > height {
                    height - p.start
                } else {
                    p.amount
                };

                let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(amount as usize);
                let mut transactions: Vec<Vec<u8>> = Vec::with_capacity(amount as usize);

                for height in p.start..p.start + amount {
                    if let Some(block) = chain
                        .find_by_height(height)
                        .await
                        .map_err(|e| NodeError::GetBlock(e.to_string()))?
                    {
                        let mut trs_to_add: Vec<u8> = Vec::new();
                        for tr_hash in block.get_transactions().iter() {
                            let tr = chain.find_transaction_raw(tr_hash).await.unwrap().unwrap(); // critical error

                            trs_to_add.extend_from_slice(&tr.len().to_be_bytes());
                        }
                        transactions.push(trs_to_add);
                        blocks.push(block.dump().unwrap());
                    } else {
                        break;
                    }
                }

                socket
                    .send(packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::GetBlocks(
                            packet_models::GetBlocksResponse {
                                blocks,
                                transactions,
                            },
                        ),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::GetLastBlock(_) => {
                let block_dump = match context
                    .blockchain
                    .get_main_chain()
                    .get_last_raw_block()
                    .await
                {
                    Err(e) => {
                        return Err(NodeError::GetBlock(e.to_string()));
                    }
                    Ok(Some(block)) => Some(block),
                    Ok(None) => None,
                };

                socket
                    .send(packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::GetBlock(packet_models::GetBlockResponse {
                            dump: block_dump,
                        }),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::NewTransaction(p) => {
                if p.transaction.len() < 4 {
                    return Err(NodeError::BadTransactionSize);
                }
                let transaction_size: u32 = u32::from_be_bytes(unsafe {
                    p.transaction[0..4].try_into().unwrap_unchecked()
                });
                if p.transaction.len() - 4 != transaction_size as usize {
                    return Err(NodeError::BadTransactionSize);
                }

                let transaction = blockchaintree::transaction::Transaction::parse(
                    &p.transaction[4..],
                    transaction_size as u64,
                )
                .map_err(|e| NodeError::ParseTransaction(e.to_string()))?;

                // check if the transaction is with root as source
                if transaction
                    .get_sender()
                    .eq(&blockchaintree::blockchaintree::ROOT_PUBLIC_ADDRESS)
                {
                    return Err(NodeError::SendFundsFromRoot);
                }

                // verify transaction
                let last_block = context
                    .blockchain
                    .get_main_chain()
                    .get_last_block()
                    .await
                    .map_err(|e| NodeError::CreateTransaction(e.to_string()))?;

                if let Some(last_block) = last_block {
                    let transaction_time = transaction.get_timestamp();
                    if transaction_time <= last_block.get_info().timestamp
                        || transaction_time > recieved_timestamp
                    {
                        return Err(NodeError::CreateTransaction(
                            "Wrong transaction time".into(),
                        ));
                    }
                }

                if !transaction
                    .verify()
                    .map_err(|e| NodeError::CreateTransaction(e.to_string()))?
                {
                    return Err(NodeError::CreateTransaction("Bad signature".into()));
                }

                context
                    .blockchain
                    .new_transaction(transaction)
                    .await
                    .map_err(|e| NodeError::CreateTransaction(e.to_string()))?;

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                let request = p.clone();
                let packet = packet_models::Packet::Request {
                    id: packet_id,
                    data: packet_models::Request::NewTransaction(request),
                };

                context
                    .propagate_packet
                    .send(PropagatedPacket {
                        packet,
                        source_addr: socket.addr,
                    })
                    .map_err(|e| NodeError::PropagationSend(socket.addr, e.to_string()))?;

                socket
                    .send(packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::Ok(packet_models::OkResponse {}),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::SubmitPow(p) => {
                if p.address.len() != 33 {
                    return Err(NodeError::WrongAddressSize(p.address.len()));
                }
                if p.timestamp > recieved_timestamp {
                    return Err(NodeError::TimestampInFuture(
                        p.timestamp,
                        recieved_timestamp,
                    ));
                } else if recieved_timestamp - p.timestamp > MAX_POW_SUBMIT_DELAY {
                    return Err(NodeError::TimestampExpired);
                }
                let pow = BigUint::from_bytes_be(&p.pow);

                // cannot fail
                let address: [u8; 33] = unsafe { p.address.clone().try_into().unwrap_unchecked() };

                let new_block = context
                    .blockchain
                    .emit_main_chain_block(pow, address, p.timestamp)
                    .await
                    .map_err(|e| NodeError::EmitMainChainBlock(e.to_string()))?;

                // context
                //     .new_data
                //     .write()
                //     .await
                //     .new_block(new_block.clone(), &socket.addr, recieved_timestamp as usize)
                //     .await;

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
                            NodeError::FindTransaction("No such transaction in db".into())
                        })
                        .unwrap();

                    dumped_transactions.reserve(4 + dump.len());

                    dumped_transactions.extend((dump.len() as u32).to_be_bytes());
                    dumped_transactions.extend(dump.iter());
                }

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                let block_packet = packet_models::Packet::Request {
                    id: packet_id,
                    data: packet_models::Request::NewBlock(packet_models::NewBlockRequest {
                        dump: new_block
                            .dump()
                            .map_err(|e| {
                                error!("Error dumping new block, fatal error");
                                e
                            })
                            .unwrap(),
                        transactions: dumped_transactions,
                    }),
                };
                context
                    .propagate_packet
                    .send(PropagatedPacket {
                        packet: block_packet,
                        source_addr: socket.addr,
                    })
                    .map_err(|e| NodeError::PropagationSend(socket.addr, e.to_string()))?;

                let response_packet = packet_models::Packet::Response {
                    id: *recieved_id,
                    data: packet_models::Response::SubmitPow(packet_models::SubmitPowResponse {
                        accepted: true,
                    }),
                };

                socket
                    .send(response_packet)
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
            packet_models::Request::NewBlock(p) => {
                let recieved_block = block::deserialize_main_chain_block(&p.dump)
                    .map_err(|e| NodeError::ParseBlock(e.to_string()))?;
                new_block(
                    recieved_block,
                    &p.transactions,
                    context,
                    socket,
                    recieved_timestamp,
                )
                .await?;

                let mut packet_id: u64 = rand::random();
                while waiting_response.get(&packet_id).is_some() {
                    packet_id = rand::random();
                }

                let block_packet = packet_models::Packet::Request {
                    id: packet_id,
                    data: packet_models::Request::NewBlock(packet_models::NewBlockRequest {
                        dump: p.dump.to_owned(),
                        transactions: p.transactions.to_owned(),
                    }),
                };

                context
                    .propagate_packet
                    .send(PropagatedPacket {
                        packet: block_packet,
                        source_addr: socket.addr,
                    })
                    .map_err(|e| NodeError::PropagationSend(socket.addr, e.to_string()))?;

                socket
                    .send(&packet_models::Packet::Response {
                        id: *recieved_id,
                        data: packet_models::Response::Ok(packet_models::OkResponse {}),
                    })
                    .await
                    .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
            }
        },
        packet_models::Packet::Response {
            id: recieved_id,
            data: r,
        } => {
            if !waiting_response.remove(recieved_id) {
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
                    if let Some(dump) = &p.ipv4 {
                        let parsed = parse_ipv4(dump).map_err(NodeError::BinToAddress)?;
                        for addr in parsed {
                            if (addr.ip().is_loopback() && addr.port() == SERVER_ADDRESS.port())
                                || addr.ip().is_unspecified()
                                || addr.eq(&SERVER_ADDRESS)
                            {
                                continue;
                            }
                            if !context.peers.write().await.insert(addr) {
                                continue;
                            };
                            context
                                .new_peers_tx
                                .send(addr)
                                .map_err(|e| NodeError::PropagationSend(addr, e.to_string()))?;
                        }
                    }

                    if let Some(dump) = &p.ipv6 {
                        let parsed = parse_ipv4(dump).map_err(NodeError::BinToAddress)?;
                        for addr in parsed {
                            if (addr.ip().is_loopback() && addr.port() == SERVER_ADDRESS.port())
                                || addr.ip().is_unspecified()
                            {
                                continue;
                            }
                            if !context.peers.write().await.insert(addr) {
                                continue;
                            };
                            context
                                .new_peers_tx
                                .send(addr)
                                .map_err(|e| NodeError::PropagationSend(addr, e.to_string()))?;
                        }
                    }
                }
                packet_models::Response::GetAmount(_) => todo!(),
                packet_models::Response::GetTransaction(_) => todo!(),
                packet_models::Response::Ping(_) => {}
                packet_models::Response::GetBlock(_) => todo!(),
                packet_models::Response::GetBlocks(p) => {
                    let mut recieved_blocks: Vec<Arc<dyn MainChainBlock + Send + Sync>> =
                        Vec::with_capacity(p.blocks.len());

                    for block in p.blocks.iter().map(|dump| {
                        block::deserialize_main_chain_block(dump)
                            .map_err(|e| NodeError::ParseBlock(e.to_string()))
                    }) {
                        recieved_blocks.push(block?);
                    }

                    for (block, transactions_dumped) in
                        recieved_blocks.into_iter().zip(p.transactions.iter())
                    {
                        new_block(
                            block,
                            transactions_dumped,
                            context,
                            socket,
                            recieved_timestamp,
                        )
                        .await?;
                    }
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
