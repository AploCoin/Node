use lazy_static::lazy_static;
use std::env::var;
use std::net::SocketAddr;

lazy_static! {
    pub static ref SERVER_ADDRESS: SocketAddr = var("SERVER_ADDRESS").unwrap().parse().unwrap();
}
