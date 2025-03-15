use std::io::Error;

use super::{process_file, ChecksumOptions};

/// Calculates the XXH3 checksum of a file.
pub async fn calculate_xxh3(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = xxhash_rust::xxh3::Xxh3::default();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.digest().to_be_bytes().to_vec())
}

/// Calculates the XXH32 checksum of a file.
pub async fn calculate_xxh32(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = xxhash_rust::xxh32::Xxh32::default();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.digest().to_be_bytes().to_vec())
}

/// Calculates the XXH64 checksum of a file.
pub async fn calculate_xxh64(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = xxhash_rust::xxh64::Xxh64::default();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.digest().to_be_bytes().to_vec())
}
