use std::io::Error;

use super::{process_file, ChecksumOptions};

/// Calculates the CRC32 checksum of a file.
pub async fn calculate_crc32(options: ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = crc32fast::Hasher::new();
    process_file(options.to_processing_options(|chunk| hasher.update(chunk))).await?;

    Ok(hasher.finalize().to_be_bytes().to_vec())
}
