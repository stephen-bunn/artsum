use super::{Manifest, ManifestFormat, ManifestParser};
use crate::error::ManifestError;
use async_trait::async_trait;
use std::path::Path;
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

    fn can_handle_dir(&self, dir: &Path) -> bool {
        dir.join(SFV_FILENAME).is_file()
    }

    fn can_handle_file(&self, filepath: &Path) -> bool {
        match filepath.file_name() {
            Some(filename) => filename == SFV_FILENAME,
            None => false,
        }
    }

    async fn parse(&self, s: &str) -> Result<Manifest, ManifestError> {
        toml::from_str(s).map_err(|e| ManifestError::DeserializeError(e))
    }

    async fn try_to_string(&self, manifest: &Manifest) -> Result<String, ManifestError> {
        toml::to_string(manifest).map_err(|e| ManifestError::SerializeError(e))
    }
}
