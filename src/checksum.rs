use std::{
    fmt::Display,
    io::{Error, ErrorKind},
    path::Path,
    str::FromStr,
};

use crc32fast::Hasher as Crc32;
use md5::Context as Md5;
use sha1::Sha1;
use sha2::{Digest as _, Sha256, Sha512};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use xxhash_rust::{xxh3::Xxh3, xxh32::Xxh32, xxh64::Xxh64};

use crate::error::ChecksumError;

/// The delimiter used to separate the checksum algorithm and the digest.
const CHECKSUM_DELIMITER: &str = ";";

/// The default chunk size used to read files.
pub const DEFAULT_CHUNK_SIZE: usize = 8192;

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
    pub async fn checksum_file(
        &self,
        filepath: &Path,
        mode: ChecksumMode,
        chunk_size: Option<usize>,
        progress_callback: Option<fn(u64, u64)>,
    ) -> Result<Vec<u8>, ChecksumError> {
        checksum_file(filepath, self, mode, chunk_size, progress_callback)
            .await
            .map_err(ChecksumError::IoError)
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
        let mut parts = vec![self.algorithm.to_string(), self.digest.to_string()];
        if self.mode == ChecksumMode::Text {
            parts.insert(0, self.mode.to_string());
        }

        write!(f, "{}", parts.join(CHECKSUM_DELIMITER))
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
        let parts: Vec<&str> = s.split(CHECKSUM_DELIMITER).collect();
        let parts_length = parts.len();

        if parts_length != 2 && parts_length != 3 {
            return Err(ChecksumError::InvalidChecksumFormat);
        }

        let (mode, algorithm_index, digest_index) = if parts_length == 3 {
            let mode = ChecksumMode::from_str(parts[0])
                .map_err(|_| ChecksumError::UnsupportedMode(parts[0].to_string()))?;
            (mode, 1, 2)
        } else {
            (ChecksumMode::default(), 0, 1)
        };

        let algorithm = parts[algorithm_index]
            .parse::<ChecksumAlgorithm>()
            .map_err(|_| ChecksumError::UnsupportedAlgorithm(parts[algorithm_index].to_string()))?;
        let digest = parts[digest_index].to_string();

        Ok(Checksum {
            mode,
            algorithm,
            digest,
        })
    }

    /// Calculates the checksum of a file using the specified algorithm.
    pub async fn from_file(
        filepath: &Path,
        algorithm: &ChecksumAlgorithm,
        mode: Option<ChecksumMode>,
        chunk_size: Option<usize>,
        progress_callback: Option<fn(u64, u64)>,
    ) -> Result<Self, ChecksumError> {
        let mode = mode.unwrap_or_default();
        let digest = algorithm
            .checksum_file(filepath, mode, chunk_size, progress_callback)
            .await?;

        Ok(Checksum {
            mode,
            algorithm: algorithm.clone(),
            digest: hex::encode(digest),
        })
    }

    /// Verifies the checksum of a file using the current checksum.
    #[allow(dead_code)]
    pub async fn verify_file(
        &self,
        filepath: &Path,
        mode: ChecksumMode,
        chunk_size: Option<usize>,
        progress_callback: Option<fn(u64, u64)>,
    ) -> Result<bool, ChecksumError> {
        let expected =
            hex::decode(&self.digest).map_err(|_| ChecksumError::InvalidChecksumFormat)?;
        let actual = self
            .algorithm
            .checksum_file(filepath, mode, chunk_size, progress_callback)
            .await?;

        Ok(expected == actual)
    }
}

async fn read_file_text_in_chunks<F>(
    filepath: &Path,
    chunk_size: Option<usize>,
    mut process_chunk: F,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<(), std::io::Error>
where
    F: FnMut(&[u8]),
{
    let file = tokio::fs::File::open(filepath).await?;
    let total_size = file.metadata().await?.len();
    let mut total_read = 0;
    let report_progress = progress_callback.unwrap_or(|_, _| {});

    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();

    let chunk_size = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let mut current_chunk = Vec::with_capacity(chunk_size);
    while let Some(line) = lines.next_line().await? {
        let normalized_line = line.replace('\r', "");
        let line_bytes = normalized_line.as_bytes();

        if current_chunk.len() + line_bytes.len() > chunk_size && !current_chunk.is_empty() {
            process_chunk(&current_chunk);
            total_read += current_chunk.len() as u64;
            report_progress(total_read, total_size);
            current_chunk.clear();
        }

        if line_bytes.len() > chunk_size {
            for chunk in line_bytes.chunks(chunk_size) {
                process_chunk(chunk);
                total_read += chunk.len() as u64;
                report_progress(total_read, total_size);
            }
        } else {
            current_chunk.extend_from_slice(line_bytes);
        }
    }

    if !current_chunk.is_empty() {
        process_chunk(&current_chunk);
        total_read += current_chunk.len() as u64;
        report_progress(total_read, total_size);
    }

    Ok(())
}

async fn read_file_binary_in_chunks<F>(
    filepath: &Path,
    chunk_size: Option<usize>,
    mut process_chunk: F,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<(), std::io::Error>
where
    F: FnMut(&[u8]),
{
    let mut file = tokio::fs::File::open(filepath).await?;
    let total_size = file.metadata().await?.len();
    let mut total_read = 0;
    let report_progress = progress_callback.unwrap_or(|_, _| {});

    let chunk_size = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let mut buffer = vec![0; chunk_size];
    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            report_progress(total_read, total_size);
            break;
        }

        process_chunk(&buffer[..bytes_read]);
        total_read += bytes_read as u64;
        report_progress(total_read, total_size);
    }

    Ok(())
}

/// Calculates the checksum of a file using the specified algorithm.
async fn read_file_in_chunks<F>(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    process_chunk: F,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<(), std::io::Error>
where
    F: FnMut(&[u8]),
{
    if !filepath.is_file() {
        return Err(Error::new(
            ErrorKind::NotFound,
            format!("File not found: {}", filepath.display()),
        ));
    }

    match mode {
        ChecksumMode::Binary => {
            return read_file_binary_in_chunks(
                filepath,
                chunk_size,
                process_chunk,
                progress_callback,
            )
            .await
        }
        ChecksumMode::Text => {
            return read_file_text_in_chunks(filepath, chunk_size, process_chunk, progress_callback)
                .await
        }
    }
}

/// Calculates the MD5 checksum of a file.
async fn calculate_md5_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Md5::new();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.consume(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.compute().0.to_vec())
}

/// Calculates the SHA1 checksum of a file.
async fn calculate_sha1_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha1::new();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.finalize().to_vec())
}

/// Calculates the SHA256 checksum of a file.
async fn calculate_sha256_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha256::new();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.finalize().to_vec())
}

/// Calculates the SHA512 checksum of a file.
async fn calculate_sha512_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha512::new();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.finalize().to_vec())
}

/// Calculates the CRC32 checksum of a file.
async fn calculate_crc32_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Crc32::new();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.finalize().to_be_bytes().to_vec())
}

/// Calculates the XXH3 checksum of a file.
async fn calculate_xxh3_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Xxh3::default();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.digest().to_be_bytes().to_vec())
}

/// Calculates the XXH32 checksum of a file.
async fn calculate_xxh32_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Xxh32::default();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.digest().to_be_bytes().to_vec())
}

/// Calculates the XXH64 checksum of a file.
async fn calculate_xxh64_checksum(
    filepath: &Path,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Xxh64::default();
    read_file_in_chunks(
        filepath,
        mode,
        chunk_size,
        |chunk| hasher.update(chunk),
        progress_callback,
    )
    .await?;

    Ok(hasher.digest().to_be_bytes().to_vec())
}

/// Calculates the checksum of a file using the specified algorithm.
pub async fn checksum_file(
    filepath: &Path,
    algorithm: &ChecksumAlgorithm,
    mode: ChecksumMode,
    chunk_size: Option<usize>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    match algorithm {
        ChecksumAlgorithm::MD5 => {
            calculate_md5_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::SHA1 => {
            calculate_sha1_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::SHA256 => {
            calculate_sha256_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::SHA512 => {
            calculate_sha512_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::CRC32 => {
            calculate_crc32_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::XXH3 => {
            calculate_xxh3_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::XXH32 => {
            calculate_xxh32_checksum(filepath, mode, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::XXH64 => {
            calculate_xxh64_checksum(filepath, mode, chunk_size, progress_callback).await
        }
    }
}
