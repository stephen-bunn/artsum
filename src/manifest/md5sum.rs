use std::collections::HashMap;

use async_trait::async_trait;
use regex::Regex;

use super::{Manifest, ManifestParser, ManifestSource};
use crate::{
    checksum::{Checksum, ChecksumAlgorithm, ChecksumMode},
    error::ManifestError,
};

pub const DEFAULT_MANIFEST_FILENAME: &str = "sfv.md5";

pub struct MD5SUMParser {
    filename_patterns: Vec<Regex>,
}

impl Default for MD5SUMParser {
    fn default() -> Self {
        MD5SUMParser {
            filename_patterns: vec![Regex::new(r"^sfv\.md5$").unwrap()],
        }
    }
}

#[async_trait]
impl ManifestParser for MD5SUMParser {
    fn filename_patterns(&self) -> &[Regex] {
        &self.filename_patterns
    }

    fn default_filename(&self) -> &str {
        DEFAULT_MANIFEST_FILENAME
    }

    fn algorithm(&self) -> Option<ChecksumAlgorithm> {
        Some(ChecksumAlgorithm::MD5)
    }

    async fn parse_manifest_source(
        &self,
        source: &ManifestSource,
    ) -> Result<Manifest, ManifestError> {
        self.from_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        // Parse out the artifacts from a standard md5sum file structure
        let mut artifacts = HashMap::new();

        for line in data.lines() {
            // Ignore blank lines or lines starting with a comment character (`#`)
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts = line.trim().split_whitespace().collect::<Vec<&str>>();
            if parts.len() != 2 {
                continue;
            }

            let mode = if line.contains("  ") {
                ChecksumMode::Text
            } else {
                ChecksumMode::Binary
            };

            // Destructure the parts into digest and path
            let (digest, path) = (parts[0], parts[1]);
            let checksum = Checksum {
                mode,
                algorithm: ChecksumAlgorithm::MD5,
                digest: digest.to_string(),
            };
            artifacts.insert(path.trim_start_matches('*').to_string(), checksum);
        }

        Ok(Manifest {
            version: None,
            artifacts,
        })
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        let mut lines = Vec::new();

        for (path, checksum) in manifest.artifacts.iter() {
            if checksum.mode == ChecksumMode::Text {
                lines.push(format!("{}  {}", checksum.digest, path));
            } else {
                lines.push(format!("{} {}", checksum.digest, path));
            }
        }

        Ok(lines.join("\n"))
    }
}
