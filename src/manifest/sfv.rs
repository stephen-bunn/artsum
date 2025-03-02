use std::{
    env::current_dir,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use toml;

use super::{Manifest, ManifestParser, ManifestSource};
use crate::error::ManifestError;

pub const SFV_FILENAME: &str = "sfv.toml";
pub struct SFVParser {}

impl Default for SFVParser {
    fn default() -> Self {
        SFVParser {}
    }
}

#[async_trait]
impl ManifestParser for SFVParser {
    fn build_manifest_filepath(&self, dirpath: Option<&Path>) -> PathBuf {
        let working_dir = current_dir().unwrap();
        dirpath.unwrap_or(working_dir.as_path()).join(SFV_FILENAME)
    }

    fn try_get_manifest_filepath(&self, dirpath: &Path) -> Option<PathBuf> {
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
        if let Some(manifest_file) = self.try_get_manifest_filepath(dirpath) {
            return self.can_handle_file(&manifest_file);
        }

        false
    }

    async fn from_str(&self, data: &str) -> Result<Manifest, ManifestError> {
        toml::from_str(data).map_err(|e| ManifestError::DeserializeError(e))
    }

    async fn from_manifest_source(
        &self,
        manifest_source: &ManifestSource,
    ) -> Result<Manifest, ManifestError> {
        self.from_str(&std::fs::read_to_string(&manifest_source.filepath)?)
            .await
    }

    async fn to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        toml::to_string(manifest).map_err(|e| ManifestError::SerializeError(e))
    }
}
