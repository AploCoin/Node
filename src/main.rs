mod errors;
mod models;
mod node;
#[macro_use]
mod tools;

use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> errors::ResultSmall<()> {
    let (tx, mut rx) = broadcast::channel::<u8>(1);

    let mut nd = node::Node::new("0.0.0.0:5050", tx.clone()).await?;

    match nd.load_peers() {
        Ok(_) => {
            println!("Successfuly loaded peers from the file")
        }
        Err(e) => {
            println!("Failed to load peers from the file, due to: {}", e);
        }
    }

    nd.start().await?;

    // waiting for tasks to finish
    drop(tx);
    let _ = rx.recv().await;

    match nd.dump_peers() {
        Ok(_) => {
            println!("Successfuly dumped peers to the file")
        }
        Err(e) => {
            println!("Failed to dump peers to the file, due to: {}", e);
        }
    }

    Ok(())
}
