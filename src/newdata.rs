use crate::errors::node_errors::NodeError;
use blockchaintree::block::MainChainBlockArc;

use blockchaintree::transaction::Transaction;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use tracing::{debug, error};

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

impl NewData {
    /// Adds new block to the data
    ///
    /// if the block already exists increases amount of approves and returns false
    pub async fn new_block(
        &mut self,
        block: MainChainBlockArc,
        transactions: &[Transaction],
        peer: &SocketAddr,
        time_recieved: usize,
    ) -> Result<bool, NodeError> {
        let block_hash = block
            .hash()
            .map_err(|e| {
                error!("Failed to hash newly created block: {}", e.to_string());
                e
            })
            .unwrap(); // smth went horribly wrong, it's safer to crash

        let mut existed = true;
        let mut same_peer = false;
        self.blocks_approves
            .entry(block_hash)
            .and_modify(|approves| {
                if approves.peers.insert(*peer) {
                    approves.total_approves += 1;
                    approves.last_recieved = time_recieved;
                } else {
                    same_peer = true
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

        if same_peer {
            return Err(NodeError::SamePeerSameBlock);
        }
        if existed {
            return Ok(false);
        }

        self.new_blocks
            .entry(block.get_info().height)
            .and_modify(|blocks| blocks.push(block.clone()))
            .or_insert(vec![block]);

        self.transactions
            .insert(block_hash, transactions.to_owned());

        Ok(true)
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
        let blocks = match self.new_blocks.remove(&height) {
            Some(blocks) => blocks,
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
