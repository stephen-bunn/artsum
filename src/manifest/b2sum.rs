use async_trait::async_trait;
use regex::Regex;

use super::{
    default_from_str, default_to_string, Manifest, ManifestError, ManifestParser, ManifestSource,
};
use crate::checksum::ChecksumAlgorithm;

pub const DEFAULT_MANIFEST_FILENAME: &str = "sfv.b2sum";

pub struct B2SUMParser {
    filename_patterns: Vec<Regex>,
}

impl Default for B2SUMParser {
    fn default() -> Self {
        B2SUMParser {
            filename_patterns: vec![Regex::new(r"^sfv\.b2sum$").unwrap()],
        }
    }
}

#[async_trait]
impl ManifestParser for B2SUMParser {
    fn filename_patterns(&self) -> &[Regex] {
        &self.filename_patterns
    }

    fn default_filename(&self) -> &str {
        DEFAULT_MANIFEST_FILENAME
    }

    fn algorithm(&self) -> Option<ChecksumAlgorithm> {
        Some(ChecksumAlgorithm::BLAKE2B512)
    }

    async fn parse(&self, source: &ManifestSource) -> Result<Manifest, ManifestError> {
        self.from_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        default_from_str(data, self.algorithm().unwrap()).await
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        default_to_string(manifest).await
    }
}
