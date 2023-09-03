use thiserror::Error;

pub type ResultSmall<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub mod models_errors {
    use super::*;

    #[derive(Debug, Error)]
    pub enum AddressError {
        #[error("The amount of bytes should be divisable by 6")]
        WronSizeIPv4,

        #[error("The amount of bytes should be divisable by 18")]
        WrongSizeIPv6,

        #[error("Bad address")]
        BadAddress,
    }

    // #[derive(Debug, Error)]
    // pub enum DeserializeTransactionsError {
    //     #[error("Supplied data with a wrong size")]
    //     NotEnoughDataError,

    //     #[error("Error")]

    // }
}

pub mod node_errors {
    use super::enc_socket_errors::EncSocketError;
    use super::*;
    use crate::models::packet_models::ErrorR;
    use getrandom::Error as grandErr;
    use std::io;
    use std::net::SocketAddr;
    use tokio::sync::broadcast::error::RecvError;

    #[derive(Debug, Clone, Error)]
    #[error("Get random failed: {:?}", self.e)]
    pub struct GetRandomError {
        pub e: grandErr,
    }
    impl GetRandomError {
        #[allow(dead_code)]
        pub fn new(e: grandErr) -> GetRandomError {
            GetRandomError { e }
        }
    }

    #[derive(Debug, Error)]
    pub enum NodeError {
        #[error("Error connecting to peer {0:?}: {1:?}")]
        ConnectToPeer(SocketAddr, EncSocketError),

        #[error("Error sending packet {0:?}: {1:?}")]
        SendPacket(SocketAddr, EncSocketError),

        #[error("Error recieving packet from {0:?}: {1:?}")]
        ReceievePacket(SocketAddr, EncSocketError),

	#[allow(dead_code)]
        #[error("Error reading propagation channel {0:?}: {1:?}")]
        PropagationRead(SocketAddr, RecvError),

        #[error("Error sending data to propagation channel {0:?}: {1:?}")]
        PropagationSend(SocketAddr, String),

        #[error("Error converting binary to address: {0:?}")]
        BinToAddress(models_errors::AddressError),

        #[error("Error accepting connection: {0:?}")]
        AcceptConnection(io::Error),

        #[error("Error binding socket: {0:?}")]
        BindSocket(io::Error),

        #[error("Error returned from remote node: {0:?}")]
        RemoteNode(ErrorR),

        #[error("Address in blockchain should have size of 33 bytes, the supplied address had size: {0}")]
        BadBlockchainAddressSize(usize),

        #[error("Error getting funds from blockchain: {0:?}")]
        GetFunds(String),

        #[error("Error getting block from blockchain: {0:?}")]
        GetBlock(String),

        #[error("Error parsing transaction, bad size")]
        BadTransactionSize,

        #[error("Error parsing transaction: {0:?}")]
        ParseTransaction(String),

        #[error("Error parsing block: {0:?}")]
        ParseBlock(String),

        #[error("Error creating transaction: {0:?}")]
        CreateTransaction(String),

        #[error("Error searching for transaction: {0:?}")]
        FindTransaction(String),

        #[error("Too much blocks requested, max: {0}")]
        TooMuchBlocks(usize),

        #[error("Chain hasn't yet reached that height")]
        _NotReachedHeight(usize),

        #[error("Same Peer tried to add same block")]
        SamePeerSameBlock,

        #[error("The submitted timestamp is bigger, than the actual time {0:?} > {1:?}")]
        TimestampInFuture(u64, u64),

        #[error("Submitted timestamp is expired")]
        TimestampExpired,

        #[error("Tried to send funds from the root address")]
        SendFundsFromRoot,

        #[error("Provided address is {0} but should be 33 bytes")]
        WrongAddressSize(usize),

        #[error("Failed to emit new main chain block: {0}")]
        EmitMainChainBlock(String),

        #[error("Failed to add new main chain block: {0}")]
        AddMainChainBlock(String),
    }
}

pub mod enc_socket_errors {
    use rmp_serde::decode::Error as DeserializeError;
    use std::io;
    use std::net::SocketAddr;

    use super::*;

    #[derive(Debug, Error)]
    pub enum EncSocketError {
        #[error("Error writing into socket: {0:?}")]
        WriteSocket(io::Error),

        #[error("Error reading from socket: {0:?}")]
        ReadSocket(io::Error),

        #[error("Error connecting to address {address:?} reason: {reason:?}")]
        Connect {
            address: SocketAddr,
            reason: io::Error,
        },

        #[error("Timeout Error")]
        Timeout,

        #[error("Too big packet: {0:?}")]
        TooBigPacket(usize),

        #[error("Error decompressing packet: {0:?}")]
        Decompress(io::Error),

        #[error("Error compressing packet: {0:?}")]
        Compress(io::Error),

        #[error("Packet deserialization error: {0:?}")]
        Deserialize(DeserializeError),
    }
}
