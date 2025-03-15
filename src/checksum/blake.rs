use blake2::Digest as _;
use std::io::Error;

use super::{process_file, ChecksumOptions};

/// Calculates the Blake2b256 checksum of a file.
pub async fn calculate_blake2b256(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = blake2::Blake2s256::new();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.finalize().to_vec())
}

pub async fn calculate_blake2b512(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = blake2::Blake2b512::new();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.finalize().to_vec())
}
