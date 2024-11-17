use strum_macros::{Display, EnumString};

use crate::error::ChecksumError;

const CHECKSUM_DELIMITER: char = ';';

#[derive(Debug, EnumString, Display)]
pub enum ChecksumAlgorithm {
    #[strum(serialize = "md5")]
    MD5,
    #[strum(serialize = "sha1")]
    SHA1,
    #[strum(serialize = "sha256")]
    SHA256,
    #[strum(serialize = "sha512")]
    SHA512,
    #[strum(serialize = "crc32")]
    CRC32,
    #[strum(serialize = "xxhash32")]
    XXHASH32,
    #[strum(serialize = "xxhash64")]
    XXHASH64,
}

#[derive(Debug)]
pub struct Checksum {
    pub algorithm: ChecksumAlgorithm,
    pub digest: String,
}

impl Checksum {
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

    pub fn to_string(&self) -> String {
        format!("{}{}{}", self.algorithm, CHECKSUM_DELIMITER, self.digest)
    }
}
