pub mod b2sum;
pub mod md5sum;
pub mod sfv;
pub mod sha1sum;
pub mod sha256sum;
pub mod sha512sum;

use std::{
    collections::HashMap,
    env::current_dir,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use log::{debug, info};
use strum::IntoEnumIterator;

use crate::checksum::{Checksum, ChecksumAlgorithm, ChecksumMode};

/// Known errors for manifest operations.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// Represents errors that occur during IO operations
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    /// Wraps a [`ChecksumError`] that occurred during manifest operations
    #[error("{0}")]
    ChecksumError(#[from] crate::checksum::ChecksumError),
}

/// The format of a manifest file.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    strum_macros::EnumString,
    strum_macros::EnumIter,
    strum_macros::Display,
    clap::ValueEnum,
)]
#[strum(serialize_all = "lowercase")]
pub enum ManifestFormat {
    SFV,
    MD5SUM,
    SHA1SUM,
    SHA256SUM,
    SHA512SUM,
    B2SUM,
}

impl Default for ManifestFormat {
    fn default() -> Self {
        ManifestFormat::SFV
    }
}

impl ManifestFormat {
    /// Get the parser for the manifest format.
    pub fn parser(&self) -> Box<dyn ManifestParser> {
        match self {
            ManifestFormat::SFV => Box::new(sfv::SFVParser::default()),
            ManifestFormat::MD5SUM => Box::new(md5sum::MD5SUMParser::default()),
            ManifestFormat::SHA1SUM => Box::new(sha1sum::SHA1SUMParser::default()),
            ManifestFormat::SHA256SUM => Box::new(sha256sum::SHA256SUMParser::default()),
            ManifestFormat::SHA512SUM => Box::new(sha512sum::SHA512SUMParser::default()),
            ManifestFormat::B2SUM => Box::new(b2sum::B2SUMParser::default()),
        }
    }
}

/// A manifest file that contains a list of artifacts and their checksums.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct Manifest {
    /// Optional version of the manifest file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    /// A map of file paths to their checksums.
    pub artifacts: HashMap<String, Checksum>,
}

/// A source for a manifest file.
#[derive(Debug)]
pub struct ManifestSource {
    /// The path to the manifest file.
    pub filepath: PathBuf,
    /// The format of the manifest file.
    pub format: ManifestFormat,
}

impl ManifestSource {
    /// Get the parser for the manifest source.
    pub fn parser(&self) -> Box<dyn ManifestParser> {
        self.format.parser()
    }

    /// Create a `ManifestSource` from a given file path.
    pub fn from_path(path: &Path) -> Option<Self> {
        let resolved_path = path.canonicalize().ok()?;
        debug!("Finding manifest source for path: {:?}", resolved_path);
        for format in ManifestFormat::iter() {
            let parser = format.parser();

            if resolved_path.is_file() && parser.can_handle_filepath(path) {
                info!("Using manifest file for path: {:?}", resolved_path);
                return Some(ManifestSource {
                    filepath: resolved_path,
                    format,
                });
            } else if resolved_path.is_dir() && parser.can_handle_dirpath(&resolved_path) {
                if let Some(manifest_file) = parser.find_manifest_filepath(&resolved_path) {
                    info!(
                        "Using manifest file found in directory: {:?}",
                        manifest_file
                    );
                    return Some(ManifestSource {
                        filepath: manifest_file.canonicalize().ok()?,
                        format,
                    });
                }
            }
        }

        None
    }
}

/// A trait for parsers of manifest files.
#[async_trait]
pub trait ManifestParser {
    /// Get the supported filename patterns for the parser.
    fn filename_patterns(&self) -> &[regex::Regex];

    /// Get the default output filename for the parser.
    fn default_filename(&self) -> &str;

    /// Get the checksum algorithm used by the parser.
    /// If the parser does not use a specific algorithm, return `None`.
    fn algorithm(&self) -> Option<ChecksumAlgorithm>;

    /// Build the manifest file path based on the given directory path.
    fn build_manifest_filepath(&self, dirpath: Option<&Path>) -> PathBuf {
        let working_dir = current_dir().unwrap();
        dirpath
            .unwrap_or(working_dir.as_path())
            .join(self.default_filename())
    }

    fn find_supported_filepath(&self, dirpath: &Path) -> Option<PathBuf> {
        if !dirpath.is_dir() {
            return None;
        }

        if let Ok(entries) = glob::glob(dirpath.join("*").to_str().unwrap()) {
            for entry in entries {
                if let Ok(path) = entry {
                    debug!("Checking if parser can handle file as manifest: {:?}", path);
                    if self.can_handle_filepath(&path) {
                        return Some(path);
                    }
                }
            }
        }

        None
    }

    /// Get the path to the manifest file in a directory.
    fn find_manifest_filepath(&self, dirpath: &Path) -> Option<PathBuf> {
        if !dirpath.is_dir() {
            return None;
        }

        let default_filepath = dirpath.join(self.default_filename());
        if default_filepath.is_file() {
            debug!("Found default manifest file: {:?}", default_filepath);
            return Some(default_filepath);
        }

        self.find_supported_filepath(dirpath)
    }

    /// Check if the parser can handle a given file path.
    fn can_handle_filepath(&self, filepath: &Path) -> bool {
        match filepath.file_name() {
            Some(filename) => {
                if let Some(filename_str) = filename.to_str() {
                    for supported_pattern in self.filename_patterns() {
                        if supported_pattern.is_match(filename_str) {
                            return true;
                        }
                    }
                }

                return false;
            }
            None => false,
        }
    }

    /// Check if the parser can handle a given directory.
    fn can_handle_dirpath(&self, dirpath: &Path) -> bool {
        if let Some(manifest_filepath) = self.find_manifest_filepath(dirpath) {
            return self.can_handle_filepath(&manifest_filepath);
        }

        false
    }

    /// Parse a manifest source.
    async fn parse(&self, source: &ManifestSource) -> Result<Manifest, ManifestError>;

    /// Parse a manifest from a str.
    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError>;

    /// Convert a manifest to a string.
    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError>;
}

/// The GNU implementation of parsing checksum / filename pairs.
async fn gnu_from_str(data: &str, algorithm: ChecksumAlgorithm) -> Result<Manifest, ManifestError> {
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
            algorithm,
            digest: digest.to_string(),
        };
        artifacts.insert(path.trim_start_matches('*').to_string(), checksum);
    }

    Ok(Manifest {
        version: None,
        artifacts,
    })
}

/// The GNU implementation of converting a manifest to checksum / filename pairs.
pub async fn gnu_to_string(manifest: &Manifest) -> Result<String, ManifestError> {
    let mut lines = Vec::with_capacity(manifest.artifacts.len());
    for (path, checksum) in manifest.artifacts.iter() {
        if checksum.mode == ChecksumMode::Text {
            lines.push(format!("{}  {}", checksum.digest, path));
        } else {
            lines.push(format!("{} {}", checksum.digest, path));
        }
    }

    Ok(lines.join("\n"))
}
