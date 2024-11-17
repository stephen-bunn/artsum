use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChecksumError {
    #[error("Invalid checksum string format, expected '<algorithm>;<digest>'")]
    InvalidChecksumFormat,

    #[error("Unsupported checksum algorithm {0}")]
    UnsupportedAlgorithm(String),
}
