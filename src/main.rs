mod errors;
mod models;
mod node;
#[macro_use]
mod tools;
mod config;
mod encsocket;
mod handlers;
mod newdata;

use blockchaintree::blockchaintree::BlockChainTree;
use std::collections::HashSet;
use std::io;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter};

use crate::models::NodeContext;
use crate::models::ReceivedPacket;

#[tokio::main]
async fn main() -> errors::ResultSmall<()> {
    // load .env file
    dotenv::dotenv().map_err(|e| {
        error!(error = e.to_string(), "Error loading .env");
        e
    })?;

    // load log config
    let env_filter = EnvFilter::from_default_env().add_directive("node=debug".parse().unwrap());
    let collector = tracing_subscriber::registry().with(env_filter).with(
        fmt::Layer::new()
            .with_writer(io::stdout)
            .with_thread_names(true),
    );
    let file_appender = tracing_appender::rolling::minutely("logs", "node_log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let collector = collector.with(
        fmt::Layer::new()
            .with_writer(non_blocking)
            .with_thread_names(true),
    );
    tracing::subscriber::set_global_default(collector).unwrap();

    info!("Staring Node");

    // configure channels
    let (tx, mut rx) = broadcast::channel::<u8>(1);
    let (txp, _) = broadcast::channel::<ReceivedPacket>(100);
    let (new_peers_tx, _) = broadcast::channel::<SocketAddr>(100);

    let peers: Arc<RwLock<HashSet<SocketAddr>>> =
        Arc::new(RwLock::new(HashSet::with_capacity(100)));

    // loading blockchain
    info!("Loading blockchain");
    let blockchain = Arc::new(match BlockChainTree::with_config() {
        Err(e) => {
            error!("Failed to load blockchain with config {:?}", e.to_string());
            info!("Trying to load blockchain without config");
            BlockChainTree::without_config()
                .map_err(|e| {
                    error!(
                        "Error loading blockchain tree without config: {:?}",
                        e.to_string()
                    )
                })
                .unwrap()
        }
        Ok(tree) => tree,
    });
    debug!("Blockchain loaded");

    debug!("Creating node context");
    let context = NodeContext {
        peers: peers.clone(),
        shutdown: tx,
        propagate_packet: txp.clone(),
        new_peers_tx: new_peers_tx.clone(),
        blockchain: blockchain.clone(),
        new_data: Default::default(),
    };

    // starting main tasks
    debug!("Starting node task");
    let fut = node::start(context.clone());
    tokio::spawn(fut);

    debug!("Starting connecting new peers task");
    let fut = node::connect_new_peers(context.clone());
    tokio::spawn(fut);

    debug!("Starting blockchain updater task");
    let fut = node::update_blockchain(context.clone());
    tokio::spawn(fut);

    debug!("Sleeping to give time for the tasks to subscribe");
    sleep(Duration::from_millis(500)).await;

    debug!("Loading peers");
    match tools::load_peers(new_peers_tx).await {
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

    info!("Node started");

    // giving the node the time to subscribe
    debug!("Sleeping to give time for the node to subscribe");
    sleep(Duration::from_millis(500)).await;

    // waiting for tasks to finish
    debug!("Waiting for the ctr+c signal");
    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {break;},
            _ = sleep(Duration::from_millis(15000)) => {
                debug!("Propagating ping packet");
                _ = txp.send(ReceivedPacket { packet: models::packet_models::Packet::Request { id: 1, data: models::packet_models::Request::Ping(models::packet_models::PingRequest{}) }, source_addr: SocketAddr::new(
                        std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
                        5050), timestamp: 228 });
            }
        }
    }
    info!("received ctrl+c command, procceding to stop the node");
    context.shutdown.send(0).unwrap();
    drop(context);
    loop {
        if let Err(broadcast::error::RecvError::Closed) = rx.recv().await {
            break;
        }
    }

    info!("Dumping peers to file");
    match tools::dump_peers(peers.read().await.iter()).await {
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

    info!("Flushing blockchain");
    blockchain.flush_blockchain().await.unwrap();

    Ok(())
}
