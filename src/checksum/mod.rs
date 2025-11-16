mod blake;
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
    pub fn to_processing_options<F>(&self, process_chunk: F) -> ChecksumProcessingOptions<'_, F>
    where
        F: FnMut(&[u8]),
    {
        ChecksumProcessingOptions {
            filepath: &self.filepath,
            mode: self.mode,
            chunk_size: self.chunk_size,
            process_chunk,
            progress_callback: self.progress_callback,
        }
    }
}

pub struct ChecksumProcessingOptions<'a, F>
where
    F: FnMut(&[u8]),
{
    /// The path to the file to process.
    pub filepath: &'a PathBuf,

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
#[strum(serialize_all = "lowercase")]
pub enum ChecksumAlgorithm {
    /// MD5 (Message-Digest Algorithm 5) is a widely used cryptographic hash function producing a 128-bit hash value.
    MD5,
    /// SHA-1 (Secure Hash Algorithm 1) is a cryptographic hash function that produces a 160-bit hash value.
    SHA1,
    /// SHA-256 (Secure Hash Algorithm 256) is part of the SHA-2 family, producing a 256-bit hash value.
    SHA256,
    /// SHA-512 (Secure Hash Algorithm 512) is part of the SHA-2 family, producing a 512-bit hash value.
    SHA512,
    /// CRC32 (Cyclic Redundancy Check 32-bit) is a checksum algorithm commonly used for error-checking in data storage and transmission.
    CRC32,
    /// XXH3 is a high-speed, non-cryptographic hash algorithm optimized for modern CPUs.
    XXH3,
    /// XXH32 is a fast, non-cryptographic hash algorithm producing a 32-bit hash value.
    XXH32,
    /// XXH64 is a fast, non-cryptographic hash algorithm producing a 64-bit hash value.
    XXH64,
    /// BLAKE2B256 is a cryptographic hash function producing a 256-bit hash value, optimized for speed and security.
    BLAKE2B256,
    /// BLAKE2B512 is a cryptographic hash function producing a 512-bit hash value, optimized for speed and security.
    BLAKE2B512,
    /// Automatically selects the best (fastest) algorithm based on file size.
    /// Only works with the artsum format.
    Best,
}

impl Default for ChecksumAlgorithm {
    /// Returns the default checksum algorithm.
    fn default() -> Self {
        ChecksumAlgorithm::Best
    }
}

impl ChecksumAlgorithm {
    /// Selects the best (fastest) algorithm based on file size.
    ///
    /// Based on xxHash performance benchmarks:
    /// - For files < 1KB: CRC32 is competitive
    /// - For files >= 1KB: XXH3 is generally fastest
    pub async fn select_best(filepath: &PathBuf) -> Result<Self, ChecksumError> {
        let metadata = tokio::fs::metadata(filepath).await?;
        let file_size = metadata.len();

        // For very small files (< 1KB), CRC32 is competitive
        // For everything else, XXH3 is the fastest
        if file_size < 1024 {
            Ok(ChecksumAlgorithm::CRC32)
        } else {
            Ok(ChecksumAlgorithm::XXH3)
        }
    }

    /// Calculates the checksum of a file using the current algorithm.
    pub async fn checksum_file(&self, options: &ChecksumOptions) -> Result<Vec<u8>, ChecksumError> {
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
    /// Binary mode processes the file as raw binary data.
    #[strum(serialize = "")]
    Binary,

    /// Text mode processes the file line by line, normalizing line endings to '\n'.
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
        // Resolve the algorithm if it's Best
        let actual_algorithm = if options.algorithm == ChecksumAlgorithm::Best {
            ChecksumAlgorithm::select_best(&options.filepath).await?
        } else {
            options.algorithm
        };

        let digest = options.algorithm.checksum_file(&options).await?;

        Ok(Checksum {
            mode: options.mode,
            algorithm: actual_algorithm,
            digest: hex::encode(digest),
        })
    }

    /// Verifies the checksum of a file using the current checksum.
    #[allow(dead_code)]
    pub async fn verify_file(&self, options: &ChecksumOptions) -> Result<bool, ChecksumError> {
        let expected =
            hex::decode(&self.digest).map_err(|_| ChecksumError::InvalidChecksumFormat)?;
        let actual = self.algorithm.checksum_file(options).await?;

        Ok(expected == actual)
    }
}

/// Calculates the checksum of a file using the specified algorithm.
pub async fn checksum_file(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
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
        ChecksumAlgorithm::BLAKE2B256 => blake::calculate_blake2b256(options).await,
        ChecksumAlgorithm::BLAKE2B512 => blake::calculate_blake2b512(options).await,
        ChecksumAlgorithm::Best => {
            // Select the best algorithm based on file size
            let best_algorithm = ChecksumAlgorithm::select_best(&options.filepath)
                .await
                .map_err(|_| {
                    Error::new(ErrorKind::NotFound, "Failed to determine best algorithm")
                })?;

            // Create new options with the selected algorithm
            let best_options = ChecksumOptions {
                filepath: options.filepath.clone(),
                algorithm: best_algorithm,
                mode: options.mode,
                chunk_size: options.chunk_size,
                progress_callback: options.progress_callback,
            };

            // Box the recursive call to avoid infinite size
            Box::pin(checksum_file(&best_options)).await
        }
    }
}

async fn process_file_text<'a, F>(
    options: ChecksumProcessingOptions<'a, F>,
) -> Result<(), std::io::Error>
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

async fn process_file_binary<'a, F>(
    options: ChecksumProcessingOptions<'a, F>,
) -> Result<(), std::io::Error>
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
async fn process_file<'a, F>(
    options: ChecksumProcessingOptions<'a, F>,
) -> Result<(), std::io::Error>
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_select_best_small_file() {
        // Create a small file (< 1KB)
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "small content").unwrap();
        let path = temp_file.path().to_path_buf();

        let algorithm = ChecksumAlgorithm::select_best(&path).await.unwrap();
        assert_eq!(algorithm, ChecksumAlgorithm::CRC32);
    }

    #[tokio::test]
    async fn test_select_best_large_file() {
        // Create a large file (>= 1KB)
        let mut temp_file = NamedTempFile::new().unwrap();
        let large_content = vec![0u8; 2048]; // 2KB
        temp_file.write_all(&large_content).unwrap();
        let path = temp_file.path().to_path_buf();

        let algorithm = ChecksumAlgorithm::select_best(&path).await.unwrap();
        assert_eq!(algorithm, ChecksumAlgorithm::XXH3);
    }

    #[tokio::test]
    async fn test_checksum_best_algorithm_small_file() {
        // Create a small file
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "test content").unwrap();
        let path = temp_file.path().to_path_buf();

        let checksum = Checksum::from_file(ChecksumOptions {
            filepath: path.clone(),
            algorithm: ChecksumAlgorithm::Best,
            mode: ChecksumMode::Binary,
            chunk_size: None,
            progress_callback: None,
        })
        .await
        .unwrap();

        // Should resolve to CRC32 for small files
        assert_eq!(checksum.algorithm, ChecksumAlgorithm::CRC32);
        assert!(!checksum.digest.is_empty());
    }

    #[tokio::test]
    async fn test_checksum_best_algorithm_large_file() {
        // Create a large file
        let mut temp_file = NamedTempFile::new().unwrap();
        let large_content = vec![0u8; 2048]; // 2KB
        temp_file.write_all(&large_content).unwrap();
        let path = temp_file.path().to_path_buf();

        let checksum = Checksum::from_file(ChecksumOptions {
            filepath: path.clone(),
            algorithm: ChecksumAlgorithm::Best,
            mode: ChecksumMode::Binary,
            chunk_size: None,
            progress_callback: None,
        })
        .await
        .unwrap();

        // Should resolve to XXH3 for large files
        assert_eq!(checksum.algorithm, ChecksumAlgorithm::XXH3);
        assert!(!checksum.digest.is_empty());
    }

    #[tokio::test]
    async fn test_checksum_best_algorithm_text_mode() {
        // Test that text mode is preserved with best algorithm
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "test content\n").unwrap();
        let path = temp_file.path().to_path_buf();

        let checksum = Checksum::from_file(ChecksumOptions {
            filepath: path.clone(),
            algorithm: ChecksumAlgorithm::Best,
            mode: ChecksumMode::Text,
            chunk_size: None,
            progress_callback: None,
        })
        .await
        .unwrap();

        assert_eq!(checksum.mode, ChecksumMode::Text);
        assert_eq!(checksum.algorithm, ChecksumAlgorithm::CRC32);
    }

    #[test]
    fn test_checksum_algorithm_default_is_best() {
        // Verify that the default algorithm is Best
        assert_eq!(ChecksumAlgorithm::default(), ChecksumAlgorithm::Best);
    }
}
