use std::io;

use async_trait::async_trait;
use regex::Regex;
use toml;

use super::{Manifest, ManifestError, ManifestParser, ManifestSource};
use crate::checksum::ChecksumAlgorithm;

pub const DEFAULT_MANIFEST_FILENAME: &str = "artsum.toml";

pub struct ARTSUMParser {
    filename_patterns: Vec<Regex>,
}

impl Default for ARTSUMParser {
    fn default() -> Self {
        ARTSUMParser {
            filename_patterns: vec![Regex::new(r"^artsum\.toml$").unwrap()],
        }
    }
}

#[async_trait]
impl ManifestParser for ARTSUMParser {
    fn filename_patterns(&self) -> &[Regex] {
        &self.filename_patterns
    }

    fn default_filename(&self) -> &str {
        DEFAULT_MANIFEST_FILENAME
    }

    fn algorithm(&self) -> Option<ChecksumAlgorithm> {
        None
    }

    async fn parse(&self, source: &ManifestSource) -> Result<Manifest, ManifestError> {
        self.parse_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn parse_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        toml::from_str(data).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err).into())
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        toml::to_string(manifest)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err).into())
    }
}
