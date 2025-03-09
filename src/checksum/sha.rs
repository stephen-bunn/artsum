use std::io::Error;

use sha2::Digest as _;

use super::{process_file, ChecksumOptions};

/// Calculates the SHA1 checksum of a file.
pub async fn calculate_sha1(options: ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = sha1::Sha1::new();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.finalize().to_vec())
}

/// Calculates the SHA256 checksum of a file.
pub async fn calculate_sha256(options: ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = sha2::Sha256::new();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.finalize().to_vec())
}

/// Calculates the SHA512 checksum of a file.
pub async fn calculate_sha512(options: ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = sha2::Sha512::new();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.finalize().to_vec())
}
