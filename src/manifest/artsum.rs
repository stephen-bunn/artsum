use std::collections::HashMap;

use async_trait::async_trait;
use regex::Regex;

use super::{Manifest, ManifestError, ManifestParser, ManifestSource};
use crate::checksum::{Checksum, ChecksumAlgorithm, ChecksumMode};

pub const DEFAULT_MANIFEST_FILENAME: &str = ".artsum";

pub struct ArtsumParser {
    filename_patterns: Vec<Regex>,
}

impl Default for ArtsumParser {
    fn default() -> Self {
        ArtsumParser {
            filename_patterns: vec![Regex::new(r"^\.artsum$").unwrap()],
        }
    }
}

#[async_trait]
impl ManifestParser for ArtsumParser {
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
        let mut artifacts = HashMap::new();

        for line in data.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let mode = if line.contains("  ") {
                ChecksumMode::Text
            } else {
                ChecksumMode::Binary
            };

            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 2 {
                continue;
            }

            let mut checksum = Checksum::from_str(parts[0])?;
            checksum.mode = mode;
            artifacts.insert(parts[1].to_string(), checksum);
        }

        Ok(Manifest { artifacts })
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        let mut lines = Vec::with_capacity(manifest.artifacts.len());
        for (path, checksum) in manifest.artifacts.iter() {
            if checksum.mode == ChecksumMode::Text {
                lines.push(format!("{}  {}", checksum, path));
            } else {
                lines.push(format!("{} {}", checksum, path));
            }
        }

        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, path::Path};
    use tempfile::NamedTempFile;

    use crate::{
        checksum::{ChecksumAlgorithm, ChecksumMode},
        manifest::ManifestFormat,
    };

    use super::*;

    #[test]
    fn default_filename() {
        assert_eq!(
            ArtsumParser::default().default_filename(),
            DEFAULT_MANIFEST_FILENAME
        )
    }

    #[test]
    fn algorithm() {
        assert_eq!(ArtsumParser::default().algorithm(), None);
    }

    #[test]
    fn can_handle_filepath_default() {
        assert!(ArtsumParser::default().can_handle_filepath(Path::new(DEFAULT_MANIFEST_FILENAME)));
    }

    #[tokio::test]
    async fn parse_read_format_binary() {
        let content = "xxh3;abc123 file1.txt\ncrc32;def456 file2.txt\n";
        let mut test_file = NamedTempFile::new().expect("Failed to create temp file");
        test_file
            .write_all(content.as_bytes())
            .expect("Failed to write to temp file");
        test_file.flush().expect("Failed to flush temp file");

        let actual = ArtsumParser::default()
            .parse(&ManifestSource {
                filepath: test_file.path().to_path_buf(),
                format: ManifestFormat::ARTSUM,
            })
            .await
            .unwrap();

        assert_eq!(actual.artifacts.len(), 2);
        let c1 = actual.artifacts.get("file1.txt").unwrap();
        assert_eq!(c1.algorithm, ChecksumAlgorithm::XXH3);
        assert_eq!(c1.digest, "abc123");
        assert_eq!(c1.mode, ChecksumMode::Binary);

        let c2 = actual.artifacts.get("file2.txt").unwrap();
        assert_eq!(c2.algorithm, ChecksumAlgorithm::CRC32);
        assert_eq!(c2.digest, "def456");
        assert_eq!(c2.mode, ChecksumMode::Binary);
    }

    #[tokio::test]
    async fn parse_str_reads_format_binary() {
        let input = "xxh3;abc123 src/main.rs\nsha256;deadbeef lib.rs\n";
        let actual = ArtsumParser::default().parse_str(input).await.unwrap();

        assert_eq!(actual.artifacts.len(), 2);
        let c = actual.artifacts.get("src/main.rs").unwrap();
        assert_eq!(c.algorithm, ChecksumAlgorithm::XXH3);
        assert_eq!(c.digest, "abc123");
        assert_eq!(c.mode, ChecksumMode::Binary);

        let c2 = actual.artifacts.get("lib.rs").unwrap();
        assert_eq!(c2.algorithm, ChecksumAlgorithm::SHA256);
        assert_eq!(c2.digest, "deadbeef");
        assert_eq!(c2.mode, ChecksumMode::Binary);
    }

    #[tokio::test]
    async fn parse_str_reads_format_text() {
        let input = "text;sha256;abc123  README.md\n";
        let actual = ArtsumParser::default().parse_str(input).await.unwrap();

        assert_eq!(actual.artifacts.len(), 1);
        let c = actual.artifacts.get("README.md").unwrap();
        assert_eq!(c.algorithm, ChecksumAlgorithm::SHA256);
        assert_eq!(c.digest, "abc123");
        assert_eq!(c.mode, ChecksumMode::Text);
    }

    #[tokio::test]
    async fn to_string_produces_format_binary() {
        let mut artifacts = HashMap::new();
        artifacts.insert(
            "src/main.rs".to_string(),
            Checksum {
                mode: ChecksumMode::Binary,
                algorithm: ChecksumAlgorithm::XXH3,
                digest: "abc123".to_string(),
            },
        );

        let manifest = Manifest { artifacts };

        let actual = ArtsumParser::default().to_string(&manifest).await.unwrap();
        assert_eq!(actual, "xxh3;abc123 src/main.rs");
    }

    #[tokio::test]
    async fn to_string_produces_format_text() {
        let mut artifacts = HashMap::new();
        artifacts.insert(
            "README.md".to_string(),
            Checksum {
                mode: ChecksumMode::Text,
                algorithm: ChecksumAlgorithm::SHA256,
                digest: "abc123".to_string(),
            },
        );

        let manifest = Manifest { artifacts };

        let actual = ArtsumParser::default().to_string(&manifest).await.unwrap();
        assert_eq!(actual, "text;sha256;abc123  README.md");
    }

    #[tokio::test]
    async fn parse_str_ignores_comment_lines() {
        let input = "# this is a comment\nxxh3;abc123 file.txt\n# another comment\n";
        let actual = ArtsumParser::default().parse_str(input).await.unwrap();

        assert_eq!(actual.artifacts.len(), 1);
        assert!(actual.artifacts.contains_key("file.txt"));
    }

    #[tokio::test]
    async fn parse_str_ignores_blank_lines() {
        let input = "\nxxh3;abc123 file.txt\n\n\ncrc32;def456 other.txt\n";
        let actual = ArtsumParser::default().parse_str(input).await.unwrap();

        assert_eq!(actual.artifacts.len(), 2);
        assert!(actual.artifacts.contains_key("file.txt"));
        assert!(actual.artifacts.contains_key("other.txt"));
    }

    #[tokio::test]
    async fn parse_str_round_trips() {
        let mut artifacts = HashMap::new();
        artifacts.insert(
            "src/main.rs".to_string(),
            Checksum {
                mode: ChecksumMode::Binary,
                algorithm: ChecksumAlgorithm::XXH3,
                digest: "abc123".to_string(),
            },
        );
        artifacts.insert(
            "README.md".to_string(),
            Checksum {
                mode: ChecksumMode::Text,
                algorithm: ChecksumAlgorithm::SHA256,
                digest: "deadbeef".to_string(),
            },
        );

        let manifest = Manifest {
            artifacts: artifacts.clone(),
        };

        let parser = ArtsumParser::default();
        let serialized = parser.to_string(&manifest).await.unwrap();
        let parsed = parser.parse_str(&serialized).await.unwrap();

        assert_eq!(parsed.artifacts, artifacts);
    }

    #[tokio::test]
    async fn parse_str_handles_multi_algorithm() {
        let input = "xxh3;aaa111 small.txt\ncrc32;bbb222 tiny.bin\nsha256;ccc333 big.dat\nblake2b512;ddd444 huge.iso\n";
        let actual = ArtsumParser::default().parse_str(input).await.unwrap();

        assert_eq!(actual.artifacts.len(), 4);
        assert_eq!(
            actual.artifacts.get("small.txt").unwrap().algorithm,
            ChecksumAlgorithm::XXH3
        );
        assert_eq!(
            actual.artifacts.get("tiny.bin").unwrap().algorithm,
            ChecksumAlgorithm::CRC32
        );
        assert_eq!(
            actual.artifacts.get("big.dat").unwrap().algorithm,
            ChecksumAlgorithm::SHA256
        );
        assert_eq!(
            actual.artifacts.get("huge.iso").unwrap().algorithm,
            ChecksumAlgorithm::BLAKE2B512
        );
    }
}
