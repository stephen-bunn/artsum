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

/// Known errors for manifest operations.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Checksum Error: {0}")]
    ChecksumError(#[from] ChecksumError),

    #[error("Deserialization Error: {0}")]
    DeserializeError(#[from] toml::de::Error),

    #[error("Serialization Error: {0}")]
    SerializeError(#[from] toml::ser::Error),

    #[error("Unsupported manifest format")]
    UnsupportedFormat,
}
