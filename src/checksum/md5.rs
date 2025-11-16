use std::io::Error;

use super::{process_file, ChecksumOptions};

/// Calculates the MD5 checksum of a file.
pub async fn calculate_md5(options: &ChecksumOptions) -> Result<Vec<u8>, Error> {
    let mut hasher = md5::Context::new();
    process_file(options.to_processing_options(|chunk| {
        hasher.consume(chunk);
    }))
    .await?;

    Ok(hasher.finalize().0.to_vec())
}
