use lazy_static::lazy_static;
use std::env::var;
use std::net::SocketAddr;

lazy_static! {
    pub static ref SERVER_ADDRESS: SocketAddr = var("SERVER_ADDRESS").unwrap().parse().unwrap();
    pub static ref ANNOUNCE_ADDRESS: SocketAddr = var("ANNOUNCE_ADDRESS").unwrap().parse().unwrap();
}

pub static MAX_BLOCKS_IN_RESPONSE: usize = 10;
pub static MAX_POW_SUBMIT_DELAY: u64 = 10; // max delay of pow submission in seconds
pub static MIN_BLOCK_APPROVE_TIME: usize = 180;
pub static MAX_BLOCKS_SYNC_AMOUNT: usize = 10;
