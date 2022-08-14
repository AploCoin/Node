use thiserror::Error;

pub type ResultSmall<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub mod models_errors {
    use super::*;

    #[derive(Debug, Clone, Error)]
    #[error("The amount of bytes should be divisable by 6")]
    pub struct WrongSizeIPv4;

    #[derive(Debug, Clone, Error)]
    #[error("The amount of bytes should be divisable by 18")]
    pub struct WrongSizeIPv6;
}

pub mod node_errors {
    use super::*;
    use getrandom::Error as grandErr;

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

    #[derive(Debug, Clone, Error)]
    #[error("Node error: {:?}", self.e)]
    pub struct NodeError {
        pub e: String,
    }
    impl NodeError {
        pub fn new(e: String) -> NodeError {
            NodeError { e }
        }
    }

    #[derive(Debug, Clone, Error)]
    #[error("Peer closed connection")]
    pub struct ConnectionClosed {}
}
