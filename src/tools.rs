use crate::errors::*;
use crate::models::{dump_addresses, parse_ipv4, parse_ipv6, peers_dump};
use blockchaintree::transaction::{Transaction, Transactionable};
use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

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

pub async fn load_peers(peers_mut: Arc<RwLock<HashSet<SocketAddr>>>) -> ResultSmall<()> {
    let file = File::open(PEERS_BACKUP_FILE)?;

    let mut decoder = zstd::Decoder::new(file)?;

    let mut decoded_data: Vec<u8> = Vec::new();

    decoder.read_to_end(&mut decoded_data)?;

    let peers = peers_dump::Peers::deserialize(&mut Deserializer::new(Cursor::new(decoded_data)))?;

    let mut peers_storage = peers_mut.write().await;
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

pub async fn dump_peers(peers_mut: Arc<RwLock<HashSet<SocketAddr>>>) -> ResultSmall<()> {
    let target = File::create(PEERS_BACKUP_FILE)?;

    let mut encoder = zstd::Encoder::new(target, 21)?;

    let peers_storage = peers_mut.read().await;

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
    }

    Ok(transactions)
}

// pub fn serialize_transactions(transactions:&[Transaction]) -> Result<Vec<u8>, node_errors::NodeError>{
//     let mut size = 0usize;
//     for transaction in transactions{
//         size += transaction.get_dump_size()+4;
//     }
//     let mut to_return = Vec::with_capacity(size);

//     for transaction in transactions{
//         let dump = transaction.dump().map_err(|e| )
//         to_return.pus
//     }

//     Ok(to_return)
// }

#[cfg(test)]
mod dump_parse_tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[tokio::test]
    async fn dump_peers_test() {
        let mut peers: HashSet<SocketAddr> = HashSet::new();

        peers.insert(SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::new(192, 168, 1, 213)),
            5050,
        ));

        dump_peers(Arc::new(RwLock::new(peers))).await.unwrap()
    }
}
