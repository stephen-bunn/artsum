pub mod sfv;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::{checksum::Checksum, error::ManifestError};

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
}

impl Default for ManifestFormat {
    fn default() -> Self {
        ManifestFormat::SFV
    }
}

impl ManifestFormat {
    /// Get the parser for the manifest format.
    pub fn get_parser(&self) -> Box<dyn ManifestParser> {
        match self {
            ManifestFormat::SFV => Box::new(sfv::SFVParser::default()),
        }
    }
}

/// A manifest file that contains a list of artifacts and their checksums.
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    /// Optional version of the manifest file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    /// A map of checksums to artifact file paths.
    pub artifacts: HashMap<Checksum, String>,
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
    pub fn get_parser(&self) -> Box<dyn ManifestParser> {
        self.format.get_parser()
    }

    /// Create a `ManifestSource` from a given file path.
    pub fn from_path(path: &Path) -> Option<Self> {
        let resolved_path = path.canonicalize().ok()?;
        for format in ManifestFormat::iter() {
            let parser = format.get_parser();

            if resolved_path.is_file() && parser.can_handle_file(path) {
                return Some(ManifestSource {
                    filepath: resolved_path,
                    format,
                });
            } else if resolved_path.is_dir() && parser.can_handle_dir(path) {
                if let Some(manifest_file) = parser.try_get_manifest_filepath(path) {
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
    /// Build the manifest file path based on the given directory path.
    fn build_manifest_filepath(&self, dirpath: Option<&Path>) -> PathBuf;
    /// Get the path to the manifest file in a directory.
    fn try_get_manifest_filepath(&self, dirpath: &Path) -> Option<PathBuf>;
    /// Check if the parser can handle a given file path.
    fn can_handle_file(&self, filepath: &Path) -> bool;
    /// Check if the parser can handle a given directory.
    fn can_handle_dir(&self, dirpath: &Path) -> bool;
    /// Parse a manifest file.
    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError>;
    async fn from_manifest_source(
        &self,
        source: &ManifestSource,
    ) -> Result<Manifest, ManifestError>;
    /// Convert a manifest to a string.
    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError>;
}
