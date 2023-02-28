use thiserror::Error;

pub type ResultSmall<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub mod models_errors {
    use super::*;

    #[derive(Debug, Error)]
    pub enum AddressError {
        #[error("The amount of bytes should be divisable by 6")]
        WronSizeIPv4Error,

        #[error("The amount of bytes should be divisable by 18")]
        WrongSizeIPv6Error,

        #[error("Bad address")]
        BadAddressError,
    }
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

    // #[derive(Debug, Clone, Error)]
    // #[error("Node error: {:?}", self.e)]
    // pub struct NodeError {
    //     pub e: String,
    // }
    // impl NodeError {
    //     pub fn new(e: String) -> NodeError {
    //         NodeError { e }
    //     }
    // }

    #[derive(Debug, Error)]
    pub enum NodeError {
        #[error("Error connecting to peer {0:?}: {1:?}")]
        ConnectToPeerError(SocketAddr, EncSocketError),

        #[error("Error sending packet {0:?}: {1:?}")]
        SendPacketError(SocketAddr, EncSocketError),

        #[error("Error recieving packet from {0:?}: {1:?}")]
        ReceievePacketError(SocketAddr, EncSocketError),

        #[error("Error reading propagation channel {0:?}: {1:?}")]
        PropagationReadError(SocketAddr, RecvError),

        #[error("Error sending data to propagation channel {0:?}: {1:?}")]
        PropagationSendError(SocketAddr, String),

        #[error("Error converting binary to address: {0:?}")]
        BinToAddressError(models_errors::AddressError),

        #[error("Error accepting connection: {0:?}")]
        AcceptConnectionError(io::Error),

        #[error("Error binding socket: {0:?}")]
        BindSocketError(io::Error),

        #[error("Error returned from remote node: {0:?}")]
        RemoteNodeError(ErrorR),

        #[error("Address in blockchain should have size of 33 bytes, the supplied address had size: {0}")]
        BadBlockchainAddressSizeError(usize),

        #[error("Error getting funds from blockchain: {0:?}")]
        GetFundsError(String),
    }

    #[derive(Debug, Clone, Error)]
    #[error("Peer closed connection")]
    pub struct ConnectionClosed {}
}

pub mod enc_socket_errors {
    use rmp_serde::decode::Error as DeserializeError;
    use std::io;
    use std::net::SocketAddr;

    use super::*;

    #[derive(Debug, Error)]
    pub enum EncSocketError {
        #[error("Error writing into socket: {0:?}")]
        WriteSocketError(io::Error),

        #[error("Error reading from socket: {0:?}")]
        ReadSocketError(io::Error),

        #[error("Error connecting to address {address:?} reason: {reason:?}")]
        ConnectError {
            address: SocketAddr,
            reason: io::Error,
        },

        #[error("Timeout Error")]
        TimeoutError,

        #[error("Too big packet: {0:?}")]
        TooBigPacketError(usize),

        #[error("Error decompressing packet: {0:?}")]
        DecompressError(io::Error),

        #[error("Error compressing packet: {0:?}")]
        CompressError(io::Error),

        #[error("Packet deserialization error: {0:?}")]
        DeserializeError(DeserializeError),
    }
}
