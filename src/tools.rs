use crate::config::MIN_BLOCK_APPROVE_TIME;
use crate::errors::node_errors::NodeError;
use crate::models::{dump_addresses, parse_ipv4, parse_ipv6, peers_dump, NodeContext};
use crate::{config, errors::*};
use blockchaintree::block::MainChainBlock;
use blockchaintree::transaction::{Transaction, Transactionable};
use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::Sender;

#[macro_export]
macro_rules! box_array {
    ($val:expr ; $len:expr) => {{
        // Use a generic function so that the pointer cast remains type-safe
        fn vec_to_boxed_array<T>(vec: Vec<T>) -> Box<[T; $len]> {
            let boxed_slice = vec.into_boxed_slice();

            let ptr = ::std::boxed::Box::into_raw(boxed_slice) as *mut [T; $len];

            unsafe { Box::from_raw(ptr) }
        }

        vec_to_boxed_array(vec![$val; $len])
    }};
}

#[allow(dead_code)]
pub fn current_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

const PEERS_BACKUP_FILE: &str = "peers.dump";

pub async fn load_peers(new_peers: Sender<SocketAddr>) -> ResultSmall<()> {
    let file = File::open(PEERS_BACKUP_FILE)?;

    let mut decoder = zstd::Decoder::new(file)?;

    let mut decoded_data: Vec<u8> = Vec::new();

    decoder.read_to_end(&mut decoded_data)?;

    let peers = peers_dump::Peers::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

    //let mut peers_storage = peers_mut.write().await;
    if let Some(dump) = peers.ipv4 {
        let parsed = parse_ipv4(&dump)?;
        for addr in parsed {
            new_peers.send(addr).unwrap();
        }
    }

    if let Some(dump) = peers.ipv6 {
        let parsed = parse_ipv6(&dump)?;
        for addr in parsed {
            new_peers.send(addr).unwrap();
        }
    }

    Ok(())
}

pub async fn dump_peers<'a, I>(peers_to_dump: I) -> ResultSmall<()>
where
    I: Iterator<Item = &'a SocketAddr>,
{
    let target = File::create(PEERS_BACKUP_FILE)?;

    let mut encoder = zstd::Encoder::new(target, 21)?;

    let (ipv4, ipv6) = dump_addresses(peers_to_dump);

    let mut buf: Vec<u8> = Vec::new();

    let peers = peers_dump::Peers { ipv4, ipv6 };

    peers.serialize(&mut Serializer::new(&mut buf))?;

    encoder.write_all(&buf)?;

    encoder.finish()?;

    Ok(())
}

/// Deserialize a set of transactions
///
/// 4 bytes big-endian - size of the next transaction
///
/// actual transaction
pub fn deserialize_transactions(data: &[u8]) -> Result<Vec<Transaction>, node_errors::NodeError> {
    let mut index: usize = 0;
    let mut transactions = Vec::new();
    while index < data.len() {
        if data.len() - index < 4 {
            return Err(node_errors::NodeError::BadTransactionSize);
        }

        let size =
            u32::from_be_bytes(unsafe { data[index..index + 4].try_into().unwrap_unchecked() })
                as u64;

        index += 4;

        if data.len() - index < size as usize {
            return Err(node_errors::NodeError::BadTransactionSize);
        }

        let _header = data[index];
        transactions.push(
            Transaction::parse(&data[index + 1..index + size as usize], size - 1)
                .map_err(|e| node_errors::NodeError::ParseTransaction(e.to_string()))?,
        );

        index += size as usize;
    }

    Ok(transactions)
}

/// Adds a new block to either blockchain or a NewData
///
/// returns true if that's in fact a new block
///
/// returns false, if the block already existed at some point, somewhere
pub async fn new_block(
    received_block: Arc<dyn MainChainBlock + Send + Sync>,
    transaction_dumps: &[u8],
    context: &NodeContext,
    address: &SocketAddr,
    received_timestamp: u64,
) -> Result<bool, NodeError> {
    let transactions = deserialize_transactions(transaction_dumps)?;

    let mut new_data = context.new_data.write().await;

    if !context
        .blockchain
        .new_main_chain_block(&received_block)
        .await
        .map_err(|e| NodeError::AddMainChainBlock(e.to_string()))?
    {
        // diverging data
        // get already existing block from the chain
        let existing_block = context
            .blockchain
            .get_main_chain()
            .find_by_height(received_block.get_info().height)
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
        if new_data
            .new_block(
                existing_block,
                &transactions,
                &SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0),
                received_timestamp as usize,
            )
            .await
            .is_err()
        {
            return Ok(false);
        };
    }

    if let Ok(new) = new_data
        .new_block(
            received_block.clone(),
            &transactions,
            address,
            received_timestamp as usize,
        )
        .await
    {
        if !new {
            // block was already in new data
            let mut blocks_same_height =
                new_data.get_blocks_same_height(received_block.get_info().height);

            // sort ascending with approves
            blocks_same_height.sort_by(|a, b| {
                let a_approves = new_data.get_block_approves(&a.hash().unwrap()).unwrap(); // critical section, stop app if error
                let b_approves = new_data.get_block_approves(&b.hash().unwrap()).unwrap(); // critical section, stop app if error

                match a_approves.total_approves.cmp(&b_approves.total_approves) {
                    Ordering::Greater => Ordering::Greater,
                    Ordering::Less => Ordering::Less,
                    Ordering::Equal => a_approves.last_received.cmp(&b_approves.last_received), // compare timestamps
                }
            });

            let max_block = blocks_same_height.last().unwrap(); // critical error
            match blocks_same_height.len() {
                1 => {
                    let approves = new_data
                        .get_block_approves(&max_block.hash().unwrap())
                        .unwrap(); // critical error
                    if current_time() as usize - approves.last_received
                        >= config::MIN_BLOCK_APPROVE_TIME
                    {
                        context
                            .blockchain
                            .overwrite_main_chain_block(&received_block, &transactions)
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
                        || current_time() as usize - max_block_approves.last_received
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
    } else {
        return Ok(false);
    }

    Ok(true)
}

#[cfg(test)]
mod dump_parse_tests {
    use std::{collections::HashSet, net::Ipv4Addr};

    use super::*;

    #[tokio::test]
    async fn dump_peers_test() {
        let mut peers: HashSet<SocketAddr> = HashSet::new();

        // peers.insert(SocketAddr::new(
        //     std::net::IpAddr::V4(Ipv4Addr::new(109, 248, 61, 27)),
        //     5050,
        // ));

        // peers.insert(SocketAddr::new(
        //     std::net::IpAddr::V4(Ipv4Addr::new(213, 160, 170, 230)),
        //     5050,
        // ));

        // peers.insert(SocketAddr::new(
        //     std::net::IpAddr::V4(Ipv4Addr::new(92, 63, 189, 56)),
        //     5050,
        // ));

        // peers.insert(SocketAddr::new(
        //     std::net::IpAddr::V4(Ipv4Addr::new(109, 248, 61, 27)),
        //     5050,
        // ));

        // peers.insert(SocketAddr::new(
        //     std::net::IpAddr::V4(Ipv4Addr::new(172, 16, 111, 181)),
        //     5050,
        // ));

        peers.insert(SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::new(192, 168, 1, 57)),
            5050,
        ));

        // peers.insert(SocketAddr::new(
        //     std::net::IpAddr::V4(Ipv4Addr::new(185, 154, 192, 59)),
        //     5050,
        // ));

        dump_peers(peers.iter()).await.unwrap()
    }
}
