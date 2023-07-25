use std::{
    collections::HashSet,
    net::{SocketAddr, SocketAddrV4},
    str::FromStr,
    sync::Arc,
};

use blockchaintree::{
    block::{self, MainChainBlock},
    transaction::Transactionable,
};
use num_bigint::BigUint;
use tracing::{debug, error, warn};

use crate::{
    config::{self, MAX_BLOCKS_SYNC_AMOUNT, MAX_POW_SUBMIT_DELAY, SERVER_ADDRESS},
    encsocket::EncSocket,
    errors::node_errors::NodeError,
    models::{
        bin2addr, dump_addresses,
        packet_models::{
            self, AnnounceRequest, GetAmountRequest, GetBlockByHashRequest,
            GetBlockByHeightRequest, GetBlocksByHeightsRequest, GetBlocksResponse, GetNodesReponse,
            GetTransactionRequest, NewBlockRequest, NewTransactionRequest, SubmitPow,
        },
        parse_ipv4, NodeContext, PropagatedPacket,
    },
    tools::new_block,
};

pub async fn announce_request_handler(
    context: &NodeContext,
    waiting_response: &mut HashSet<u64>,
    socket: &mut EncSocket,
    packet: &AnnounceRequest,
) -> Result<(), NodeError> {
    let addr = bin2addr(&packet.addr).map_err(NodeError::BinToAddress)?;

    // verify address is not loopback
    if (addr.ip().is_loopback() && addr.port() == SERVER_ADDRESS.port())
        || addr.ip().is_unspecified()
        || addr.eq(&SERVER_ADDRESS)
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

        let request = packet.clone();
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

    Ok(())
}

pub async fn get_amount_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    packet: &GetAmountRequest,
    recieved_id: u64,
) -> Result<(), NodeError> {
    if packet.address.len() != 33 {
        socket
            .send(packet_models::Packet::Error(packet_models::ErrorR {
                code: packet_models::ErrorCode::BadBlockchainAddress,
            }))
            .await
            .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
        return Err(NodeError::BadBlockchainAddressSize(packet.address.len()));
    }

    let address: [u8; 33] = unsafe { packet.to_owned().address.try_into().unwrap_unchecked() };

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
            id: recieved_id,
            data: packet_models::Response::GetAmount(packet_models::GetAmountResponse {
                amount: funds_dumped,
            }),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

    Ok(())
}

pub async fn get_nodes_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
) -> Result<(), NodeError> {
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
        id: recieved_id,
        data: packet_models::Response::GetNodes(packet_models::GetNodesReponse { ipv4, ipv6 }),
    };

    socket
        .send(packet)
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

    Ok(())
}

pub async fn get_transaction_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    packet: &GetTransactionRequest,
) -> Result<(), NodeError> {
    let packet = packet_models::Packet::Response {
        id: recieved_id,
        data: packet_models::Response::GetTransaction(packet_models::GetTransactionResponse {
            transaction: context
                .blockchain
                .get_main_chain()
                .find_transaction(&packet.hash)
                .await
                .map_err(|e| NodeError::FindTransaction(e.to_string()))?
                .map(|tr| {
                    tr.dump()
                        .map_err(|e| NodeError::FindTransaction(e.to_string()))
                        .unwrap() // TODO: remove unwrap
                }),
        }),
    };

    socket
        .send(packet)
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

    Ok(())
}

pub async fn get_block_by_hash_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    packet: &GetBlockByHashRequest,
) -> Result<(), NodeError> {
    let block_dump = match context
        .blockchain
        .get_main_chain()
        .find_raw_by_hash(&packet.hash)
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
            id: recieved_id,
            data: packet_models::Response::GetBlock(packet_models::GetBlockResponse {
                dump: block_dump,
            }),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
    Ok(())
}

pub async fn get_block_by_height_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    packet: &GetBlockByHeightRequest,
) -> Result<(), NodeError> {
    let block_dump = match context
        .blockchain
        .get_main_chain()
        .find_raw_by_height(packet.height)
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
            id: recieved_id,
            data: packet_models::Response::GetBlock(packet_models::GetBlockResponse {
                dump: block_dump,
            }),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
    Ok(())
}

pub async fn get_blocks_by_height(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    packet: &GetBlocksByHeightsRequest,
) -> Result<(), NodeError> {
    if packet.amount as usize > config::MAX_BLOCKS_IN_RESPONSE {
        return Err(NodeError::TooMuchBlocks(config::MAX_BLOCKS_IN_RESPONSE));
    } else if packet.amount == 0 {
        socket
            .send(packet_models::Packet::Response {
                id: recieved_id,
                data: packet_models::Response::GetBlocks(packet_models::GetBlocksResponse {
                    blocks: Vec::new(),
                    transactions: Vec::new(),
                }),
            })
            .await
            .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
        return Ok(());
    }

    let chain = context.blockchain.get_main_chain();
    let height = chain.get_height().await;
    if height <= packet.start {
        socket
            .send(packet_models::Packet::Response {
                id: recieved_id,
                data: packet_models::Response::GetBlocks(packet_models::GetBlocksResponse {
                    blocks: Vec::with_capacity(0),
                    transactions: Vec::with_capacity(0),
                }),
            })
            .await
            .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
        return Ok(());
    }

    let amount = if packet.start + packet.amount > height {
        height - packet.start
    } else {
        packet.amount
    };

    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(amount as usize);
    let mut transactions: Vec<Vec<u8>> = Vec::with_capacity(amount as usize);

    for height in packet.start..packet.start + amount {
        if let Some(block) = chain
            .find_by_height(height)
            .await
            .map_err(|e| NodeError::GetBlock(e.to_string()))?
        {
            let mut trs_to_add: Vec<u8> = Vec::new();
            for tr_hash in block.get_transactions().iter() {
                let tr = chain.find_transaction_raw(tr_hash).await.unwrap().unwrap(); // critical error

                trs_to_add.extend_from_slice(&(tr.len() as u32).to_be_bytes());
                trs_to_add.extend(tr.into_iter());
            }
            transactions.push(trs_to_add);
            blocks.push(block.dump().unwrap());
        } else {
            break;
        }
    }

    socket
        .send(packet_models::Packet::Response {
            id: recieved_id,
            data: packet_models::Response::GetBlocks(packet_models::GetBlocksResponse {
                blocks,
                transactions,
            }),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
    Ok(())
}

pub async fn get_last_block_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
) -> Result<(), NodeError> {
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
            id: recieved_id,
            data: packet_models::Response::GetBlock(packet_models::GetBlockResponse {
                dump: block_dump,
            }),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
    Ok(())
}

pub async fn new_transaction_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    recieved_timestamp: u64,
    waiting_response: &mut HashSet<u64>,
    packet: &NewTransactionRequest,
) -> Result<(), NodeError> {
    if packet.transaction.len() < 4 {
        return Err(NodeError::BadTransactionSize);
    }
    let transaction_size: u32 =
        u32::from_be_bytes(unsafe { packet.transaction[0..4].try_into().unwrap_unchecked() });
    if packet.transaction.len() - 4 != transaction_size as usize {
        return Err(NodeError::BadTransactionSize);
    }

    let transaction = blockchaintree::transaction::Transaction::parse(
        &packet.transaction[4..],
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

    let request = packet.clone();
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
            id: recieved_id,
            data: packet_models::Response::Ok(packet_models::OkResponse {}),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

    Ok(())
}

pub async fn submit_pow_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    recieved_timestamp: u64,
    waiting_response: &mut HashSet<u64>,
    packet: &SubmitPow,
) -> Result<(), NodeError> {
    if packet.address.len() != 33 {
        return Err(NodeError::WrongAddressSize(packet.address.len()));
    }
    if packet.timestamp > recieved_timestamp {
        return Err(NodeError::TimestampInFuture(
            packet.timestamp,
            recieved_timestamp,
        ));
    } else if recieved_timestamp - packet.timestamp > MAX_POW_SUBMIT_DELAY {
        return Err(NodeError::TimestampExpired);
    }
    let pow = BigUint::from_bytes_be(&packet.pow);

    // cannot fail
    let address: [u8; 33] = unsafe { packet.address.clone().try_into().unwrap_unchecked() };

    let new_block = context
        .blockchain
        .emit_main_chain_block(pow, address, packet.timestamp)
        .await
        .map_err(|e| NodeError::EmitMainChainBlock(e.to_string()))?;

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
        id: recieved_id,
        data: packet_models::Response::SubmitPow(packet_models::SubmitPowResponse {
            accepted: true,
        }),
    };

    socket
        .send(response_packet)
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;

    Ok(())
}

pub async fn new_block_request_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_id: u64,
    recieved_timestamp: u64,
    waiting_response: &mut HashSet<u64>,
    packet: &NewBlockRequest,
) -> Result<(), NodeError> {
    let recieved_block = block::deserialize_main_chain_block(&packet.dump)
        .map_err(|e| NodeError::ParseBlock(e.to_string()))?;
    if new_block(
        recieved_block,
        &packet.transactions,
        context,
        socket,
        recieved_timestamp,
    )
    .await?
    {
        let mut packet_id: u64 = rand::random();
        while waiting_response.get(&packet_id).is_some() {
            packet_id = rand::random();
        }

        let block_packet = packet_models::Packet::Request {
            id: packet_id,
            data: packet_models::Request::NewBlock(packet_models::NewBlockRequest {
                dump: packet.dump.to_owned(),
                transactions: packet.transactions.to_owned(),
            }),
        };

        context
            .propagate_packet
            .send(PropagatedPacket {
                packet: block_packet,
                source_addr: socket.addr,
            })
            .map_err(|e| NodeError::PropagationSend(socket.addr, e.to_string()))?;
    }
    socket
        .send(&packet_models::Packet::Response {
            id: recieved_id,
            data: packet_models::Response::Ok(packet_models::OkResponse {}),
        })
        .await
        .map_err(|e| NodeError::SendPacket(socket.addr, e))?;
    Ok(())
}

pub async fn get_nodes_response_handler(
    context: &NodeContext,
    packet: &GetNodesReponse,
) -> Result<(), NodeError> {
    if let Some(dump) = &packet.ipv4 {
        let parsed = parse_ipv4(dump).map_err(NodeError::BinToAddress)?;
        for addr in parsed {
            if (addr.ip().is_loopback() && addr.port() == SERVER_ADDRESS.port())
                || addr.ip().is_unspecified()
                || addr.eq(&SERVER_ADDRESS)
            {
                warn!("A bad address was supplied: {:?}", addr);
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

    if let Some(dump) = &packet.ipv6 {
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
    Ok(())
}

pub async fn get_blocks_response_handler(
    context: &NodeContext,
    socket: &mut EncSocket,
    recieved_timestamp: u64,
    packet: &GetBlocksResponse,
) -> Result<(), NodeError> {
    let mut recieved_blocks: Vec<Arc<dyn MainChainBlock + Send + Sync>> =
        Vec::with_capacity(packet.blocks.len());

    for block in packet.blocks.iter().map(|dump| {
        block::deserialize_main_chain_block(dump).map_err(|e| NodeError::ParseBlock(e.to_string()))
    }) {
        recieved_blocks.push(block?);
    }

    if !recieved_blocks.is_empty() {
        debug!("Propagating a packet to get new blocks");
        if let Some(e) = context
            .propagate_packet
            .send(PropagatedPacket {
                packet: packet_models::Packet::Request {
                    id: 0,
                    data: packet_models::Request::GetBlocksByHeights(
                        packet_models::GetBlocksByHeightsRequest {
                            start: unsafe { recieved_blocks.last().unwrap_unchecked() }
                                .get_info()
                                .height
                                + 1,
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
    }

    for (block, transactions_dumped) in recieved_blocks.into_iter().zip(packet.transactions.iter())
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
    Ok(())
}
