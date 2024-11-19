use super::{Manifest, ManifestFormat, ManifestParser};
use crate::error::ManifestError;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use toml;

pub const SFV_FILENAME: &str = "sfv.toml";
pub struct SFVParser {}

impl Default for SFVParser {
    fn default() -> Self {
        SFVParser {}
    }
}

#[async_trait]
impl ManifestParser for SFVParser {
    fn get_format(&self) -> ManifestFormat {
        ManifestFormat::SFV
    }

    fn get_manifest_file(&self, dirpath: &Path) -> Option<PathBuf> {
        if !dirpath.is_dir() {
            return None;
        }

        let manifest_file = dirpath.join(SFV_FILENAME);
        if !manifest_file.is_file() {
            return None;
        }

        Some(manifest_file)
    }

    fn can_handle_file(&self, filepath: &Path) -> bool {
        match filepath.file_name() {
            Some(filename) => filename == SFV_FILENAME,
            None => false,
        }
    }

    fn can_handle_dir(&self, dirpath: &Path) -> bool {
        if let Some(manifest_file) = self.get_manifest_file(dirpath) {
            return self.can_handle_file(&manifest_file);
        }

        false
    }

    async fn parse(&self, data: &str) -> Result<Manifest, ManifestError> {
        toml::from_str(data).map_err(|e| ManifestError::DeserializeError(e))
    }

    async fn try_to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        toml::to_string(manifest).map_err(|e| ManifestError::SerializeError(e))
    }
}
