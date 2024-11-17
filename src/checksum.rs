use std::io::{Error, ErrorKind};
use std::path::Path;

use crc32fast::Hasher as Crc32;
use md5::Context as Md5;
use sha1::Sha1;
use sha2::{Digest as _, Sha256, Sha512};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use xxhash_rust::{xxh3::Xxh3, xxh32::Xxh32, xxh64::Xxh64};

use crate::error::ChecksumError;

/// The delimiter used to separate the checksum algorithm and the digest.
const CHECKSUM_DELIMITER: char = ';';
/// The default chunk size used to read files.
const DEFAULT_CHUNK_SIZE: u64 = 8192;

/// Defines the checksum algorithms supported by this library.
#[derive(Clone, Debug, strum_macros::EnumString, strum_macros::EnumIter, strum_macros::Display)]
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
    /// Returns the default checksum algorithm, which is XXH3.
    fn default() -> Self {
        ChecksumAlgorithm::XXH3
    }
}

impl ChecksumAlgorithm {
    /// Calculates the checksum of a file using the current algorithm.
    pub async fn checksum_file(
        &self,
        filepath: &Path,
        chunk_size: Option<u64>,
        progress_callback: Option<fn(u64, u64)>,
    ) -> Result<Vec<u8>, ChecksumError> {
        checksum_file(filepath, self, chunk_size, progress_callback)
            .await
            .map_err(ChecksumError::IoError)
    }
}

/// Defines a checksum, which is a pair of an algorithm and a digest.
#[derive(Debug)]
pub struct Checksum {
    pub algorithm: ChecksumAlgorithm,
    pub digest: String,
}

impl Checksum {
    /// Parses a checksum from a string, which should be in the format '<algorithm>;<digest>'.
    pub fn from_str(s: &str) -> Result<Self, ChecksumError> {
        let parts: Vec<&str> = s.split(CHECKSUM_DELIMITER).collect();
        if parts.len() != 2 {
            return Err(ChecksumError::InvalidChecksumFormat);
        }

        let algorithm = parts[0]
            .parse::<ChecksumAlgorithm>()
            .map_err(|_| ChecksumError::UnsupportedAlgorithm(parts[0].to_string()))?;
        let digest = parts[1].to_string();
        Ok(Checksum { algorithm, digest })
    }

    /// Converts the checksum to a string, which is in the format '<algorithm>;<digest>'.
    pub fn to_string(&self) -> String {
        format!("{}{}{}", self.algorithm, CHECKSUM_DELIMITER, self.digest)
    }

    /// Calculates the checksum of a file using the specified algorithm.
    pub async fn from_file(
        filepath: &Path,
        algorithm: &ChecksumAlgorithm,
        chunk_size: Option<u64>,
        progress_callback: Option<fn(u64, u64)>,
    ) -> Result<Self, ChecksumError> {
        let digest = algorithm
            .checksum_file(filepath, chunk_size, progress_callback)
            .await?;
        Ok(Checksum {
            algorithm: algorithm.clone(),
            digest: hex::encode(digest),
        })
    }

    /// Verifies the checksum of a file using the currenc checksum.
    pub async fn verify_file(
        &self,
        filepath: &Path,
        chunk_size: Option<u64>,
        progress_callback: Option<fn(u64, u64)>,
    ) -> Result<bool, ChecksumError> {
        let expected =
            hex::decode(&self.digest).map_err(|_| ChecksumError::InvalidChecksumFormat)?;
        let actual = self
            .algorithm
            .checksum_file(filepath, chunk_size, progress_callback)
            .await?;

        Ok(expected == actual)
    }
}

/// Calculates the checksum of a file using the specified algorithm.
async fn read_file_in_chunks<F>(
    filepath: &Path,
    chunk_size: Option<u64>,
    mut process_chunk: F,
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

    let mut file = File::open(filepath).await?;
    let total_size = file.metadata().await?.len();
    let mut total_read = 0;
    let report_progress = progress_callback.unwrap_or(|_, _| {});

    let chunk_size = chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let mut buffer = vec![0; chunk_size as usize];
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

/// Calculates the MD5 checksum of a file.
async fn calculate_md5_checksum(
    filepath: &Path,
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Md5::new();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha1::new();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha256::new();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Sha512::new();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Crc32::new();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Xxh3::default();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Xxh32::default();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut hasher = Xxh64::default();
    read_file_in_chunks(
        filepath,
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
    chunk_size: Option<u64>,
    progress_callback: Option<fn(u64, u64)>,
) -> Result<Vec<u8>, Error> {
    match algorithm {
        ChecksumAlgorithm::MD5 => {
            calculate_md5_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::SHA1 => {
            calculate_sha1_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::SHA256 => {
            calculate_sha256_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::SHA512 => {
            calculate_sha512_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::CRC32 => {
            calculate_crc32_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::XXH3 => {
            calculate_xxh3_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::XXH32 => {
            calculate_xxh32_checksum(filepath, chunk_size, progress_callback).await
        }
        ChecksumAlgorithm::XXH64 => {
            calculate_xxh64_checksum(filepath, chunk_size, progress_callback).await
        }
    }
}
