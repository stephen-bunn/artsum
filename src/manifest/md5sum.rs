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
    const SAMPLE_COUNT: usize = 20;

    use super::*;
    use crate::checksum::{Checksum, ChecksumMode};
    use fake::{faker::filesystem::en::*, Fake, Faker};
    use std::{
        collections::HashMap,
        ffi::OsStr,
        path::{Path, PathBuf},
    };

    fn get_parser() -> MD5SUMParser {
        MD5SUMParser::default()
    }

    #[test]
    fn test_can_handle_filepath_default() {
        assert!(get_parser().can_handle_filepath(&Path::new(DEFAULT_MANIFEST_FILENAME)));
    }

    #[test]
    fn test_can_handle_filepath_md5_extension() {
        for mut path in fake::vec![PathBuf as FilePath(); SAMPLE_COUNT] {
            if path.file_name().is_none() {
                path.set_file_name(&OsStr::new(FileName().fake::<String>().as_str()));
            }

            path.set_extension("md5");
            assert!(get_parser().can_handle_filepath(&path));
        }
    }

    #[test]
    fn test_can_handle_filepath_unsupported_extension() {
        for mut path in fake::vec![PathBuf as FilePath(); SAMPLE_COUNT] {
            if path.file_name().is_none() {
                path.set_file_name(&OsStr::new(FileName().fake::<String>().as_str()));
            }

            if let Some(ext) = path.extension() {
                if ext != "md5" {
                    path.set_extension(&OsStr::new(FileExtension().fake::<String>().as_str()));
                }
            }
            assert!(!get_parser().can_handle_filepath(&path));
        }
    }

    #[test]
    fn test_default_filename() {
        assert_eq!(get_parser().default_filename(), DEFAULT_MANIFEST_FILENAME);
    }

    #[test]
    fn test_algorithm() {
        assert_eq!(get_parser().algorithm(), Some(ChecksumAlgorithm::MD5));
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
