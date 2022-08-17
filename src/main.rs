mod errors;
mod models;
mod node;
#[macro_use]
mod tools;
mod config;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::signal;
use tokio::sync::broadcast;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> errors::ResultSmall<()> {
    // load .env file
    dotenv::dotenv().expect("Failed to read .env file");

    // configure channels
    let (tx, mut rx) = broadcast::channel::<u8>(1);
    let (txp, _) = broadcast::channel::<models::packet_models::Packet>(100);
    let (new_peers_tx, _) = broadcast::channel::<SocketAddr>(100);

    let peers: Arc<Mutex<HashSet<SocketAddr>>> = Arc::new(Mutex::new(HashSet::with_capacity(100)));

    match node::load_peers(peers.clone()) {
        Ok(_) => {
            println!("Successfuly loaded peers from the file")
        }
        Err(e) => {
            println!("Failed to load peers from the file, due to: {}", e);
        }
    }

    println!("Starting the node...");

    // starting main tasks
    let fut = node::start(peers.clone(), tx.clone(), txp.clone(), new_peers_tx.clone());
    tokio::spawn(fut);

    let fut = node::connect_new_peers(tx.clone(), peers.clone(), txp.clone(), new_peers_tx);
    tokio::spawn(fut);

    // giving the node the time to subscribe
    sleep(Duration::from_millis(500)).await;

    // waiting for tasks to finish
    signal::ctrl_c().await.unwrap();
    tx.send(0).unwrap();
    drop(tx);
    loop {
        if let Err(broadcast::error::RecvError::Closed) = rx.recv().await {
            break;
        }
    }

    match node::dump_peers(peers) {
        Ok(_) => {
            println!("Successfuly dumped peers to the file")
        }
        Err(e) => {
            println!("Failed to dump peers to the file, due to: {}", e);
        }
    }

    Ok(())
}
