use async_trait::async_trait;
use regex::Regex;

use super::{
    standard_from_str, standard_to_string, Manifest, ManifestError, ManifestParser, ManifestSource,
};
use crate::checksum::ChecksumAlgorithm;

pub const DEFAULT_MANIFEST_FILENAME: &str = "artsum.sha1";

pub struct SHA1SUMParser {
    filename_patterns: Vec<Regex>,
}

impl Default for SHA1SUMParser {
    fn default() -> Self {
        SHA1SUMParser {
            filename_patterns: vec![
                Regex::new(r"^artsum\.sha1$").unwrap(),
                Regex::new(r"^.*\.sha1$").unwrap(),
                Regex::new(r"^.*\.sha1sum$").unwrap(),
            ],
        }
    }
}

#[async_trait]
impl ManifestParser for SHA1SUMParser {
    fn filename_patterns(&self) -> &[Regex] {
        &self.filename_patterns
    }

    fn default_filename(&self) -> &str {
        DEFAULT_MANIFEST_FILENAME
    }

    fn algorithm(&self) -> Option<ChecksumAlgorithm> {
        Some(ChecksumAlgorithm::SHA1)
    }

    async fn parse(&self, source: &ManifestSource) -> Result<Manifest, ManifestError> {
        self.parse_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn parse_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        standard_from_str(data, self.algorithm().unwrap()).await
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        standard_to_string(manifest).await
    }
}

#[cfg(test)]
mod tests {
    use fake::{faker::filesystem::en::*, Fake};
    use proptest::prelude::*;
    use std::{
        ffi::OsStr,
        io::Write,
        path::{Path, PathBuf},
    };
    use tempfile::NamedTempFile;

    use super::*;
    use crate::checksum::ChecksumMode;
    use crate::manifest::{utils::fake_manifest, ManifestFormat};

    #[test]
    fn default_filename() {
        assert_eq!(
            SHA1SUMParser::default().default_filename(),
            DEFAULT_MANIFEST_FILENAME
        );
    }

    #[test]
    fn algorithm() {
        assert_eq!(
            SHA1SUMParser::default().algorithm(),
            Some(ChecksumAlgorithm::SHA1)
        );
    }

    #[test]
    fn can_handle_filepath_default() {
        assert!(SHA1SUMParser::default().can_handle_filepath(&Path::new(DEFAULT_MANIFEST_FILENAME)));
    }

    proptest! {
        #[test]
        fn can_handle_filepath_extension(ext in "sha1(sum)?") {
            let mut filepath = PathBuf::from(FileName().fake::<String>());
            filepath.set_extension(&OsStr::new(ext.as_str()));
            prop_assert!(SHA1SUMParser::default().can_handle_filepath(filepath.as_path()));
        }

        #[test]
        fn can_handle_filepath_unsupported_extension(ext in "[a-zA-Z0-9]{1,4}") {
            prop_assume!(ext != ".sha1" && ext != ".sha1sum");

            let mut filepath = PathBuf::from(FileName().fake::<String>());
            filepath.set_extension(&OsStr::new(ext.as_str()));
            prop_assert!(!SHA1SUMParser::default().can_handle_filepath(filepath.as_path()));
        }
    }

    #[tokio::test]
    async fn parse_reads_standard_format_binary() {
        let expected = fake_manifest(ChecksumAlgorithm::SHA1, ChecksumMode::Binary);
        let mut test_file = NamedTempFile::new().expect("Failed to create temp file");
        test_file
            .write(standard_to_string(&expected).await.unwrap().as_bytes())
            .expect("Failed to write to temp file");
        test_file.flush().expect("Failed to flush temp file");

        let actual = SHA1SUMParser::default()
            .parse(&ManifestSource {
                filepath: test_file.path().to_path_buf(),
                format: ManifestFormat::SHA1SUM,
            })
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts);
    }

    #[tokio::test]
    async fn parse_str_reads_standard_format_binary() {
        let expected = fake_manifest(ChecksumAlgorithm::SHA1, ChecksumMode::Binary);
        let actual = SHA1SUMParser::default()
            .parse_str(standard_to_string(&expected).await.unwrap().as_str())
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts);
    }

    #[tokio::test]
    async fn parse_str_reads_standard_format_text() {
        let expected = fake_manifest(ChecksumAlgorithm::SHA1, ChecksumMode::Text);
        let actual = SHA1SUMParser::default()
            .parse_str(standard_to_string(&expected).await.unwrap().as_str())
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts);
    }

    #[tokio::test]
    async fn parse_str_produces_standard_format_binary() {
        let manifest = fake_manifest(ChecksumAlgorithm::SHA1, ChecksumMode::Binary);
        let expected = standard_to_string(&manifest).await.unwrap();

        let actual = SHA1SUMParser::default().to_string(&manifest).await.unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn to_string_produces_standard_format_text() {
        let manifest = fake_manifest(ChecksumAlgorithm::SHA1, ChecksumMode::Text);
        let expected = standard_to_string(&manifest).await.unwrap();

        let actual = SHA1SUMParser::default().to_string(&manifest).await.unwrap();
        assert_eq!(actual, expected);
    }
}
