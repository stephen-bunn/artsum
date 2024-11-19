mod sfv;

use crate::{checksum::Checksum, error::ManifestError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The format of a manifest file.
#[derive(Clone, Copy, Debug)]
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

/// A trait for parsers of manifest files.
#[async_trait]
pub trait ManifestParser {
    /// Get the format of the manifest file.
    fn get_format(&self) -> ManifestFormat;
    /// Check if the parser can handle a given directory.
    fn can_handle_dir(&self, dir: &Path) -> bool;
    /// Check if the parser can handle a given file path.
    fn can_handle_file(&self, filepath: &Path) -> bool;
    /// Parse a manifest file.
    async fn parse(&self, data: &str) -> Result<Manifest, ManifestError>;
    /// Convert a manifest to a string.
    async fn try_to_string(&self, manifest: &Manifest) -> Result<String, ManifestError>;
}

/// Get a list of available manifest parsers.
fn get_available_parsers() -> Vec<Box<dyn ManifestParser>> {
    vec![Box::new(sfv::SFVParser::default())]
}

/// Get a parser for a given file path.
pub fn get_parser(filepath: &Path) -> Option<Box<dyn ManifestParser>> {
    for parser in get_available_parsers() {
        if parser.can_handle_file(filepath) {
            return Some(parser);
        }
    }

    None
}
