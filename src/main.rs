mod errors;
mod models;
mod node;
#[macro_use]
mod tools;

#[tokio::main]
async fn main() -> errors::Result<()>{
    let mut nd = node::Node::new("0.0.0.0:5050").await?;

    match nd.load_peers(){
        Ok(_) => {
            println!("Successfuly loaded peers from the file")
        },
        Err(e) => {
            println!("Failed to load peers from the file, due to: {}",e);
        }
    }

    nd.start().await?;

    Ok(())
}
