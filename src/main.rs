mod errors;
mod models;
mod node;
#[macro_use]
mod tools;
mod config;
mod encsocket;

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter};

#[tokio::main]
async fn main() -> errors::ResultSmall<()> {
    // load log config
    let env_filter = EnvFilter::from_default_env().add_directive("node=debug".parse().unwrap());
    let collector = tracing_subscriber::registry().with(env_filter).with(
        fmt::Layer::new()
            .with_writer(io::stdout)
            .with_thread_names(true),
    );
    let file_appender = tracing_appender::rolling::daily("logs", "node_log");
    let (non_blocking, _) = tracing_appender::non_blocking(file_appender);
    let collector = collector.with(
        fmt::Layer::new()
            .with_writer(non_blocking)
            .with_thread_names(true),
    );
    tracing::subscriber::set_global_default(collector).unwrap();

    info!("Staring Node");

    // load .env file
    dotenv::dotenv().map_err(|e| {
        error!(error = e.to_string(), "Error loading .env");
        e
    })?;

    // configure channels
    let (tx, mut rx) = broadcast::channel::<u8>(1);
    let (txp, _) = broadcast::channel::<models::packet_models::Packet>(100);
    let (new_peers_tx, _) = broadcast::channel::<SocketAddr>(100);

    let peers: Arc<Mutex<HashSet<SocketAddr>>> = Arc::new(Mutex::new(HashSet::with_capacity(100)));

    match node::load_peers(peers.clone()).await {
        Ok(_) => {
            info!("Successfuly loaded peers from the file")
        }
        Err(e) => {
            error!(
                "Failed to load peers from the file, due to: {}",
                e.to_string()
            );
        }
    }

    // starting main tasks
    debug!("Starting node task");
    let fut = node::start(peers.clone(), tx.clone(), txp.clone(), new_peers_tx.clone());
    tokio::spawn(fut);

    debug!("Starting connecting new peers task");
    let fut = node::connect_new_peers(tx.clone(), peers.clone(), txp.clone(), new_peers_tx);
    tokio::spawn(fut);

    info!("Node started");

    // giving the node the time to subscribe
    debug!("Sleeping to give time for the node to subscribe");
    sleep(Duration::from_millis(500)).await;

    // waiting for tasks to finish
    debug!("Waiting for the ctr+c signal");
    signal::ctrl_c().await.unwrap();
    tx.send(0).unwrap();
    drop(tx);
    loop {
        if let Err(broadcast::error::RecvError::Closed) = rx.recv().await {
            break;
        }
    }

    match node::dump_peers(peers).await {
        Ok(_) => {
            info!("Successfuly dumped peers to the file")
        }
        Err(e) => {
            error!(
                "Failed to dump peers to the file, due to: {}",
                e.to_string()
            );
        }
    }

    Ok(())
}
