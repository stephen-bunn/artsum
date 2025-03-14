mod crc32;
mod md5;
mod sha;
mod xxhash;

use std::{
    fmt::Display,
    io::{Error, ErrorKind},
    path::PathBuf,
    str::FromStr,
};

use log::debug;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

/// The delimiter used to separate the checksum algorithm and the digest.
const CHECKSUM_DELIMITER: &str = ";";

/// The default chunk size used to read files.
pub const DEFAULT_CHUNK_SIZE: usize = 8192;

/// Known errors for checksum operations.
#[derive(Debug, thiserror::Error)]
pub enum ChecksumError {
    /// Represents errors that occur during IO operations
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    /// Occurs when a checksum string does not follow the expected format
    #[error("Invalid checksum string format, expected '<algorithm>;<digest>' or '<mode>;<algorithm>;<digest>'")]
    InvalidChecksumFormat,

    /// Occurs when attempting to use a checksum algorithm that is not supported
    #[error("Unsupported checksum algorithm {0}")]
    UnsupportedAlgorithm(String),

    #[error("Unsupported checksum mode {0}")]
    #[allow(dead_code)]
    UnsupportedMode(String),
}

#[derive(Debug)]
pub struct ChecksumOptions {
    /// The path to the file to process.
    pub filepath: PathBuf,

    /// The checksum algorithm to use.
    pub algorithm: ChecksumAlgorithm,

    /// The checksum mode to use.
    pub mode: ChecksumMode,

    /// Size of chunks to read at once, defaults to DEFAULT_CHUNK_SIZE.
    pub chunk_size: Option<usize>,

    /// Optional progress callback to report progress.
    /// Takes the number of bytes read and the total file size.
    pub progress_callback: Option<fn(u64, u64)>,
}

impl ChecksumOptions {
    pub fn to_processing_options<F>(&self, process_chunk: F) -> ChecksumProcessingOptions<F>
    where
        F: FnMut(&[u8]),
    {
        ChecksumProcessingOptions {
            filepath: self.filepath.clone(),
            mode: self.mode,
            chunk_size: self.chunk_size,
            process_chunk,
            progress_callback: self.progress_callback,
        }
    }
}

pub struct ChecksumProcessingOptions<F>
where
    F: FnMut(&[u8]),
{
    /// The path to the file to process.
    pub filepath: PathBuf,

    /// The checksum mode to use.
    pub mode: ChecksumMode,

    /// Size of chunks to read at once, defaults to DEFAULT_CHUNK_SIZE.
    pub chunk_size: Option<usize>,

    /// Function to process each chunk of data.
    pub process_chunk: F,

    /// Optional progress callback to report progress.
    /// Takes the number of bytes read and the total file size.
    pub progress_callback: Option<fn(u64, u64)>,
}

/// Defines the checksum algorithms supported by this library.
#[derive(
    Clone,
    Debug,
    Hash,
    PartialEq,
    Eq,
    strum_macros::EnumString,
    strum_macros::EnumIter,
    strum_macros::Display,
    clap::ValueEnum,
)]
#[strum(serialize_all = "lowercase")]
pub enum ChecksumAlgorithm {
    MD5,
    SHA1,
    SHA256,
    SHA512,
    CRC32,
    XXH3,
    XXH32,
    XXH64,
}

impl Default for ChecksumAlgorithm {
    /// Returns the default checksum algorithm.
    fn default() -> Self {
        ChecksumAlgorithm::XXH3
    }
}

impl ChecksumAlgorithm {
    /// Calculates the checksum of a file using the current algorithm.
    pub async fn checksum_file(&self, options: ChecksumOptions) -> Result<Vec<u8>, ChecksumError> {
        checksum_file(options).await.map_err(ChecksumError::IoError)
    }
}

/// Defines the available modes for checksum calculation.
#[derive(
    Debug,
    Clone,
    Copy,
    Hash,
    PartialEq,
    Eq,
    strum_macros::EnumString,
    strum_macros::EnumIter,
    strum_macros::Display,
    clap::ValueEnum,
)]
pub enum ChecksumMode {
    #[strum(serialize = "")]
    Binary,
    #[strum(serialize = "text")]
    Text,
}

impl Default for ChecksumMode {
    /// Returns the default checksum mode.
    fn default() -> Self {
        ChecksumMode::Binary
    }
}

/// Defines a checksum, which is a pair of an algorithm and a digest.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct Checksum {
    pub mode: ChecksumMode,
    pub algorithm: ChecksumAlgorithm,
    pub digest: String,
}

impl Display for Checksum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.mode == ChecksumMode::Text {
            write!(
                f,
                "{}{}{}{}{}",
                self.mode, CHECKSUM_DELIMITER, self.algorithm, CHECKSUM_DELIMITER, self.digest
            )
        } else {
            write!(f, "{}{}{}", self.algorithm, CHECKSUM_DELIMITER, self.digest)
        }
    }
}

impl serde::Serialize for Checksum {
    /// Serializes the checksum to a string, which is in the format '<algorithm>;<digest>'.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Checksum {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Checksum::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Checksum {
    /// Parses a checksum from a string, which should be in the format '<algorithm>;<digest>' or '<mode>;<algorithm>;<digest>'.
    pub fn from_str(s: &str) -> Result<Self, ChecksumError> {
        let mut parts = s.splitn(3, CHECKSUM_DELIMITER);

        // Get the first part which could be mode or algorithm
        let first = parts.next().ok_or(ChecksumError::InvalidChecksumFormat)?;

        // Try to parse the first part as a mode
        let (mode, algorithm_str) = match ChecksumMode::from_str(first) {
            Ok(mode) => {
                // If first part is a valid mode, get algorithm from second part
                let alg = parts.next().ok_or(ChecksumError::InvalidChecksumFormat)?;
                (mode, alg)
            }
            Err(_) => {
                // If first part is not a mode, it must be the algorithm
                (ChecksumMode::default(), first)
            }
        };

        // The last part must be the digest
        let digest = parts.next().ok_or(ChecksumError::InvalidChecksumFormat)?;

        // Check for extra parts (invalid format)
        if parts.next().is_some() {
            return Err(ChecksumError::InvalidChecksumFormat);
        }

        let algorithm = algorithm_str
            .parse::<ChecksumAlgorithm>()
            .map_err(|_| ChecksumError::UnsupportedAlgorithm(algorithm_str.to_string()))?;

        Ok(Checksum {
            mode,
            algorithm,
            digest: digest.to_string(),
        })
    }

    /// Calculates the checksum of a file using the specified algorithm.
    pub async fn from_file(options: ChecksumOptions) -> Result<Self, ChecksumError> {
        let mode = options.mode.clone();
        let algorithm = options.algorithm.clone();
        let digest = algorithm.checksum_file(options).await?;

        Ok(Checksum {
            mode,
            algorithm,
            digest: hex::encode(digest),
        })
    }

    /// Verifies the checksum of a file using the current checksum.
    #[allow(dead_code)]
    pub async fn verify_file(&self, options: ChecksumOptions) -> Result<bool, ChecksumError> {
        let expected =
            hex::decode(&self.digest).map_err(|_| ChecksumError::InvalidChecksumFormat)?;
        let actual = self.algorithm.checksum_file(options).await?;

        Ok(expected == actual)
    }
}

/// Calculates the checksum of a file using the specified algorithm.
pub async fn checksum_file(options: ChecksumOptions) -> Result<Vec<u8>, Error> {
    debug!("{:?}", options);
    match options.algorithm {
        ChecksumAlgorithm::MD5 => md5::calculate_md5(options).await,
        ChecksumAlgorithm::SHA1 => sha::calculate_sha1(options).await,
        ChecksumAlgorithm::SHA256 => sha::calculate_sha256(options).await,
        ChecksumAlgorithm::SHA512 => sha::calculate_sha512(options).await,
        ChecksumAlgorithm::CRC32 => crc32::calculate_crc32(options).await,
        ChecksumAlgorithm::XXH3 => xxhash::calculate_xxh3(options).await,
        ChecksumAlgorithm::XXH32 => xxhash::calculate_xxh32(options).await,
        ChecksumAlgorithm::XXH64 => xxhash::calculate_xxh64(options).await,
    }
}

async fn process_file_text<F>(options: ChecksumProcessingOptions<F>) -> Result<(), std::io::Error>
where
    F: FnMut(&[u8]),
{
    let file = tokio::fs::File::open(&options.filepath).await?;
    let total_size = file.metadata().await?.len();
    let chunk_size = options.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let report_progress = options.progress_callback.unwrap_or(|_, _| {});
    let mut process_chunk = options.process_chunk;

    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut buffer = Vec::with_capacity(chunk_size);
    let mut total_read = 0;

    // Process line by line for text mode
    while let Some(line) = lines.next_line().await? {
        // Add line with normalized line ending (\n)
        let line_with_newline = line + "\n";
        let line_bytes = line_with_newline.as_bytes();

        // If adding this line would exceed chunk size, process current buffer first
        if !buffer.is_empty() && buffer.len() + line_bytes.len() > chunk_size {
            process_chunk(&buffer);
            total_read += buffer.len() as u64;
            report_progress(total_read, total_size);
            buffer.clear();
        }

        // Handle lines that exceed chunk size
        if line_bytes.len() > chunk_size {
            // If buffer has content, process it first
            if !buffer.is_empty() {
                process_chunk(&buffer);
                total_read += buffer.len() as u64;
                report_progress(total_read, total_size);
                buffer.clear();
            }

            // Process the long line in chunks
            for chunk in line_bytes.chunks(chunk_size) {
                process_chunk(chunk);
                total_read += chunk.len() as u64;
                report_progress(total_read, total_size);
            }
        } else {
            // Add line to buffer
            buffer.extend_from_slice(line_bytes);
        }
    }

    // Process any remaining data
    if !buffer.is_empty() {
        process_chunk(&buffer);
        total_read += buffer.len() as u64;
        report_progress(total_read, total_size);
    }

    Ok(())
}

async fn process_file_binary<F>(options: ChecksumProcessingOptions<F>) -> Result<(), std::io::Error>
where
    F: FnMut(&[u8]),
{
    let mut file = tokio::fs::File::open(&options.filepath).await?;
    let total_size = file.metadata().await?.len();
    let chunk_size = options.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let report_progress = options.progress_callback.unwrap_or(|_, _| {});
    let mut process_chunk = options.process_chunk;

    let mut buffer = vec![0; chunk_size];
    let mut total_read = 0;

    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }

        process_chunk(&buffer[..bytes_read]);
        total_read += bytes_read as u64;
        report_progress(total_read, total_size);
    }

    // Final progress report
    report_progress(total_read, total_size);

    Ok(())
}

/// Calculates the checksum of a file using the specified algorithm.
async fn process_file<F>(options: ChecksumProcessingOptions<F>) -> Result<(), std::io::Error>
where
    F: FnMut(&[u8]),
{
    let filepath = &options.filepath;
    if !filepath.is_file() {
        return Err(Error::new(
            ErrorKind::NotFound,
            format!("File not found: {}", filepath.display()),
        ));
    }

    match options.mode {
        ChecksumMode::Binary => process_file_binary(options).await,
        ChecksumMode::Text => process_file_text(options).await,
    }
}
