use thiserror::Error;

/// Known errors for checksum operations.
#[derive(Debug, Error)]
pub enum ChecksumError {
    /// Represents errors that occur during IO operations
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    /// Occurs when a checksum string does not follow the expected format of '<algorithm>;<digest>'
    #[error("Invalid checksum string format, expected '<algorithm>;<digest>'")]
    InvalidChecksumFormat,

    /// Occurs when attempting to use a checksum algorithm that is not supported
    #[error("Unsupported checksum algorithm {0}")]
    UnsupportedAlgorithm(String),

    #[error("Unsupported checksum mode {0}")]
    #[allow(dead_code)]
    UnsupportedMode(String),
}

/// Known errors for manifest operations.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// Represents errors that occur during IO operations
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    /// Wraps a [`ChecksumError`] that occurred during manifest operations
    #[error("Checksum Error: {0}")]
    ChecksumError(#[from] ChecksumError),

    /// Occurs when the manifest cannot be properly deserialized from TOML format
    #[error("Deserialization Error: {0}")]
    DeserializeError(#[from] toml::de::Error),

    /// Occurs when the manifest cannot be properly serialized to TOML format
    #[error("Serialization Error: {0}")]
    SerializeError(#[from] toml::ser::Error),

    /// Occurs when an unsupported manifest format is encountered
    #[error("Unsupported manifest format")]
    #[allow(dead_code)]
    UnsupportedFormat,
}
