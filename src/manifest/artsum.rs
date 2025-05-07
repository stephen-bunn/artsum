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

#[cfg(test)]
mod tests {
    use std::{io::Write, path::Path};
    use tempfile::NamedTempFile;

    use crate::{
        checksum::ChecksumMode,
        manifest::{utils::fake_manifest, ManifestFormat},
    };

    use super::*;

    #[test]
    fn default_filename() {
        assert_eq!(
            ARTSUMParser::default().default_filename(),
            DEFAULT_MANIFEST_FILENAME
        )
    }

    #[test]
    fn algorithm() {
        assert_eq!(ARTSUMParser::default().algorithm(), None);
    }

    #[test]
    fn can_handle_filepath_default() {
        assert!(ARTSUMParser::default().can_handle_filepath(&Path::new(DEFAULT_MANIFEST_FILENAME)));
    }

    #[tokio::test]
    async fn parse_read_format_binary() {
        let expected = fake_manifest(ChecksumAlgorithm::XXH3, ChecksumMode::Binary);
        let mut test_file = NamedTempFile::new().expect("Failed to create temp file");
        test_file
            .write(toml::to_string(&expected).unwrap().as_bytes())
            .expect("Failed to write to temp file");
        test_file.flush().expect("Failed to flush temp file");

        let actual = ARTSUMParser::default()
            .parse(&ManifestSource {
                filepath: test_file.path().to_path_buf(),
                format: ManifestFormat::ARTSUM,
            })
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts)
    }

    #[tokio::test]
    async fn parse_str_reads_format_binary() {
        let expected = fake_manifest(ChecksumAlgorithm::XXH3, ChecksumMode::Binary);
        let actual = ARTSUMParser::default()
            .parse_str(toml::to_string(&expected).unwrap().as_str())
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts)
    }

    #[tokio::test]
    async fn parse_str_reads_format_text() {
        let expected = fake_manifest(ChecksumAlgorithm::XXH3, ChecksumMode::Text);
        let actual = ARTSUMParser::default()
            .parse_str(toml::to_string(&expected).unwrap().as_str())
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts)
    }

    #[tokio::test]
    async fn to_string_produces_standard_format_binary() {
        let manifest = fake_manifest(ChecksumAlgorithm::XXH3, ChecksumMode::Binary);
        let expected = toml::to_string(&manifest).unwrap().to_string();

        let actual = ARTSUMParser::default().to_string(&manifest).await.unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn to_string_produces_standard_format_text() {
        let manifest = fake_manifest(ChecksumAlgorithm::XXH3, ChecksumMode::Text);
        let expected = toml::to_string(&manifest).unwrap().to_string();

        let actual = ARTSUMParser::default().to_string(&manifest).await.unwrap();
        assert_eq!(actual, expected);
    }
}
