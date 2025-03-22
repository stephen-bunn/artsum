use async_trait::async_trait;
use regex::Regex;

use super::{gnu_from_str, gnu_to_string, Manifest, ManifestError, ManifestParser, ManifestSource};
use crate::checksum::ChecksumAlgorithm;

pub const DEFAULT_MANIFEST_FILENAME: &str = "sfv.sha256";

pub struct SHA256SUMParser {
    filename_patterns: Vec<Regex>,
}

impl Default for SHA256SUMParser {
    fn default() -> Self {
        SHA256SUMParser {
            filename_patterns: vec![
                Regex::new(r"^sfv\.sha256$").unwrap(),
                Regex::new(r"^.*\.sha256$").unwrap(),
            ],
        }
    }
}

#[async_trait]
impl ManifestParser for SHA256SUMParser {
    fn filename_patterns(&self) -> &[Regex] {
        &self.filename_patterns
    }

    fn default_filename(&self) -> &str {
        DEFAULT_MANIFEST_FILENAME
    }

    fn algorithm(&self) -> Option<ChecksumAlgorithm> {
        Some(ChecksumAlgorithm::SHA256)
    }

    async fn parse(&self, source: &ManifestSource) -> Result<Manifest, ManifestError> {
        self.from_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        gnu_from_str(data, self.algorithm().unwrap()).await
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        gnu_to_string(manifest).await
    }
}
