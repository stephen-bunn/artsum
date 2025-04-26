use async_trait::async_trait;

use regex::Regex;

use super::{
    standard_from_str, standard_to_string, Manifest, ManifestError, ManifestParser, ManifestSource,
};
use crate::checksum::ChecksumAlgorithm;

pub const DEFAULT_MANIFEST_FILENAME: &str = "artsum.md5";

pub struct MD5SUMParser {
    filename_patterns: Vec<Regex>,
}

impl Default for MD5SUMParser {
    fn default() -> Self {
        MD5SUMParser {
            filename_patterns: vec![
                Regex::new(r"^artsum\.md5$").unwrap(),
                Regex::new(r"^.*\.md5$").unwrap(),
                Regex::new(r"^.*\.md5sum$").unwrap(),
            ],
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

    async fn parse(&self, source: &ManifestSource) -> Result<Manifest, ManifestError> {
        self.from_str(tokio::fs::read_to_string(&source.filepath).await?.as_str())
            .await
    }

    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        standard_from_str(data, self.algorithm().unwrap()).await
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        standard_to_string(manifest).await
    }
}

#[cfg(test)]
mod tests {
    use fake::{faker::filesystem::en::*, Fake, Faker};
    use proptest::prelude::*;
    use std::{
        collections::HashMap,
        ffi::OsStr,
        io::Write,
        path::{Path, PathBuf},
    };
    use tempfile::NamedTempFile;

    use super::*;
    use crate::checksum::{Checksum, ChecksumMode};
    use crate::manifest::ManifestFormat;

    fn get_parser() -> MD5SUMParser {
        MD5SUMParser::default()
    }

    #[test]
    fn test_default_filename() {
        assert_eq!(get_parser().default_filename(), DEFAULT_MANIFEST_FILENAME);
    }

    #[test]
    fn test_algorithm() {
        assert_eq!(get_parser().algorithm(), Some(ChecksumAlgorithm::MD5));
    }

    #[test]
    fn test_can_handle_filepath_default() {
        assert!(get_parser().can_handle_filepath(&Path::new(DEFAULT_MANIFEST_FILENAME)));
    }

    proptest! {
        #[test]
        fn test_can_handle_filepath_extension(ext in "md5(sum)?") {
            let mut filepath = PathBuf::from(FileName().fake::<String>());
            filepath.set_extension(&OsStr::new(ext.as_str()));
            prop_assert!(MD5SUMParser::default().can_handle_filepath(filepath.as_path()));
        }

        #[test]
        fn test_can_handle_filepath_unsupported_extension(ext in "[a-zA-Z0-9]{1,4}") {
            prop_assume!(ext != ".md5" && ext != ".md5sum");

            let mut filepath = PathBuf::from(FileName().fake::<String>());
            filepath.set_extension(&OsStr::new(ext.as_str()));
            prop_assert!(!MD5SUMParser::default().can_handle_filepath(filepath.as_path()));
        }
    }

    #[tokio::test]
    async fn test_parse_reads_standard_format_binary() {
        let parser = get_parser();
        let filename: String = FilePath().fake();
        let checksum = Checksum {
            algorithm: parser.algorithm().unwrap(),
            digest: Faker.fake(),
            mode: ChecksumMode::Binary,
        };

        let expected = Manifest {
            version: None,
            artifacts: HashMap::from([(filename.clone(), checksum.clone())]),
        };

        let mut test_file = NamedTempFile::new().expect("Failed to create temp file");
        test_file
            .write(format!("{} {}", checksum.digest, filename).as_bytes())
            .expect("Failed to write to temp file");
        test_file.flush().expect("Failed to flush temp file");

        let actual = parser
            .parse(&ManifestSource {
                filepath: test_file.path().to_path_buf(),
                format: ManifestFormat::MD5SUM,
            })
            .await
            .unwrap();

        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts);
    }

    #[tokio::test]
    async fn test_from_str_reads_standard_format_binary() {
        let parser = get_parser();
        let filename: String = FilePath().fake();
        let checksum = Checksum {
            algorithm: parser.algorithm().unwrap(),
            digest: Faker.fake(),
            mode: ChecksumMode::Binary,
        };

        let expected = Manifest {
            version: None,
            artifacts: HashMap::from([(filename.clone(), checksum.clone())]),
        };
        let actual = parser
            .from_str(format!("{} {}", checksum.digest, filename).as_str())
            .await
            .unwrap();
        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts);
    }

    #[tokio::test]
    async fn test_from_str_reads_standard_format_text() {
        let parser = get_parser();
        let filename: String = FilePath().fake();
        let checksum = Checksum {
            algorithm: parser.algorithm().unwrap(),
            digest: Faker.fake(),
            mode: ChecksumMode::Text,
        };
        let expected = Manifest {
            version: None,
            artifacts: HashMap::from([(filename.clone(), checksum.clone())]),
        };
        let actual = parser
            .from_str(format!("{}  {}", checksum.digest, filename).as_str())
            .await
            .unwrap();
        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.artifacts, expected.artifacts);
    }

    #[tokio::test]
    async fn test_to_string_produces_standard_format_binary() {
        let parser = get_parser();
        let filename: String = FilePath().fake();
        let checksum: Checksum = Checksum {
            algorithm: parser.algorithm().unwrap(),
            digest: Faker.fake(),
            mode: ChecksumMode::Binary,
        };
        let manifest = Manifest {
            version: None,
            artifacts: HashMap::from([(filename.clone(), checksum.clone())]),
        };

        assert_eq!(
            parser.to_string(&manifest).await.unwrap(),
            format!("{} {}", checksum.digest, filename)
        );
    }

    #[tokio::test]
    async fn test_to_string_produces_standard_format_text() {
        let parser = get_parser();
        let filename: String = FilePath().fake();
        let checksum: Checksum = Checksum {
            algorithm: parser.algorithm().unwrap(),
            digest: Faker.fake(),
            mode: ChecksumMode::Text,
        };
        let manifest = Manifest {
            version: None,
            artifacts: HashMap::from([(filename.clone(), checksum.clone())]),
        };

        assert_eq!(
            parser.to_string(&manifest).await.unwrap(),
            format!("{}  {}", checksum.digest, filename)
        );
    }
}
