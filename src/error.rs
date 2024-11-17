use thiserror::Error;

/// Known errors for checksum operations.
#[derive(Debug, Error)]
pub enum ChecksumError {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid checksum string format, expected '<algorithm>;<digest>'")]
    InvalidChecksumFormat,

    #[error("Unsupported checksum algorithm {0}")]
    UnsupportedAlgorithm(String),
}
