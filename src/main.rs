mod errors;
mod models;
mod node;
#[macro_use]
mod tools;

use tokio::sync::broadcast;
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use std::net::SocketAddr;
use tokio::signal;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> errors::ResultSmall<()> {
    let (tx, mut rx) = broadcast::channel::<u8>(1);
    let (txp, _) = broadcast::channel::<Vec<u8>>(50);

    let peers:Arc<Mutex<HashSet<SocketAddr>>> = Arc::new(Mutex::new(HashSet::with_capacity(100)));

    match node::load_peers(peers.clone()) {
        Ok(_) => {
            println!("Successfuly loaded peers from the file")
        }
        Err(e) => {
            println!("Failed to load peers from the file, due to: {}", e);
        }
    }

    println!("Starting the node...");

    let fut = node::start("127.0.0.1:5050", peers.clone(), tx.clone(), txp.clone());
    tokio::spawn(fut);

    // giving the node the time to subscribe
    sleep(Duration::from_millis(500)).await;

    // waiting for tasks to finish
    signal::ctrl_c().await.unwrap();
    tx.send(0).unwrap();
    drop(tx);
    loop{
        match rx.recv().await{
            Err(broadcast::error::RecvError::Closed) => break,
            _ => {}
        }
    };

    match node::dump_peers(peers.clone()) {
        Ok(_) => {
            println!("Successfuly dumped peers to the file")
        }
        Err(e) => {
            println!("Failed to dump peers to the file, due to: {}", e);
        }
    }

    Ok(())
}
