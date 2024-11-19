mod sfv;

use crate::{checksum::Checksum, error::ManifestError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn available_parsers() -> Vec<Box<dyn ManifestParser>> {
    vec![Box::new(sfv::SFVParser::default())]
}

/// The format of a manifest file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ManifestFormat {
    SFV,
}

impl Default for ManifestFormat {
    fn default() -> Self {
        ManifestFormat::SFV
    }
}

/// A manifest file that contains a list of artifacts and their checksums.
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    pub artifacts: HashMap<Checksum, String>,
}

#[derive(Debug)]
pub struct ManifestSource {
    pub filepath: PathBuf,
    pub format: ManifestFormat,
}

/// A trait for parsers of manifest files.
#[async_trait]
pub trait ManifestParser {
    /// Get the format of the manifest file.
    fn get_format(&self) -> ManifestFormat;
    /// Get the path to the manifest file in a directory.
    fn get_manifest_file(&self, dirpath: &Path) -> Option<PathBuf>;
    /// Check if the parser can handle a given file path.
    fn can_handle_file(&self, filepath: &Path) -> bool;
    /// Check if the parser can handle a given directory.
    fn can_handle_dir(&self, dirpath: &Path) -> bool;
    /// Parse a manifest file.
    async fn parse(&self, data: &str) -> Result<Manifest, ManifestError>;
    /// Convert a manifest to a string.
    async fn try_to_string(&self, manifest: &Manifest) -> Result<String, ManifestError>;
}

/// Determine the manifest source for a given path.
pub fn get_manifest_source(path: &Path) -> Option<ManifestSource> {
    let resolved_path = path.canonicalize().ok()?;
    if resolved_path.is_file() {
        for parser in available_parsers() {
            if parser.can_handle_file(path) {
                return Some(ManifestSource {
                    filepath: resolved_path,
                    format: parser.get_format(),
                });
            }
        }
    } else if resolved_path.is_dir() {
        for parser in available_parsers() {
            if parser.can_handle_dir(path) {
                if let Some(manifest_file) = parser.get_manifest_file(path) {
                    return Some(ManifestSource {
                        filepath: manifest_file.canonicalize().ok()?,
                        format: parser.get_format(),
                    });
                }
            }
        }
    }

    None
}

/// Get a manifest parser for a given manifest source.
pub fn get_manifest_parser(manifest_source: &ManifestSource) -> Box<dyn ManifestParser> {
    match manifest_source.format {
        ManifestFormat::SFV => Box::new(sfv::SFVParser::default()),
    }
}
