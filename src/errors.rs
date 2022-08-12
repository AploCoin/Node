use thiserror::Error;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub mod models_errors {
    use super::*;

    #[derive(Debug, Clone, Error)]
    #[error("The amount of bytes should be divisable by 6")]
    pub struct WrongSizeIPv4;

    #[derive(Debug, Clone, Error)]
    #[error("The amount of bytes should be divisable by 18")]
    pub struct WrongSizeIPv6;
}
