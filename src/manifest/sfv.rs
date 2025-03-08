use async_trait::async_trait;
use regex::Regex;
use toml;

use super::{Manifest, ManifestParser, ManifestSource};
use crate::{checksum::ChecksumAlgorithm, error::ManifestError};

pub const DEFAULT_MANIFEST_FILENAME: &str = "sfv.toml";

pub struct SFVParser {
    filename_patterns: Vec<Regex>,
}

impl Default for SFVParser {
    fn default() -> Self {
        SFVParser {
            filename_patterns: vec![Regex::new(r"^sfv\.toml$").unwrap()],
        }
    }
}

#[async_trait]
impl ManifestParser for SFVParser {
    fn filename_patterns(&self) -> &[Regex] {
        &self.filename_patterns
    }

    fn default_filename(&self) -> &str {
        DEFAULT_MANIFEST_FILENAME
    }

    fn algorithm(&self) -> Option<ChecksumAlgorithm> {
        None
    }

    async fn parse_manifest_source(
        &self,
        source: &ManifestSource,
    ) -> Result<Manifest, ManifestError> {
        self.from_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        toml::from_str(data).map_err(|e| ManifestError::DeserializeError(e))
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        toml::to_string(manifest).map_err(|e| ManifestError::SerializeError(e))
    }
}
