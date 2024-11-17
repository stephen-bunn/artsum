use std::collections::HashMap;
use std::io::{Error, ErrorKind};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use toml;

use crate::checksum::Checksum;

/// A manifest file that contains a list of artifacts and their checksums.
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    pub artifacts: HashMap<String, Checksum>,
}

impl Manifest {
    /// Read a manifest file from a JSON string.
    pub async fn from_json(s: &str) -> Result<Self, Error> {
        let manifest: Manifest = serde_json::from_str(s)?;
        Ok(manifest)
    }

    /// Convert a manifest file to a JSON string.
    pub async fn to_json(&self) -> Result<String, Error> {
        Ok(serde_json::to_string(self)?)
    }

    /// Read a manifest file from a TOML string.
    pub async fn from_toml(s: &str) -> Result<Self, Error> {
        let manifest: Manifest =
            toml::from_str(s).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
        Ok(manifest)
    }

    /// Convert a manifest file to a TOML string.
    pub async fn to_toml(&self) -> Result<String, Error> {
        Ok(toml::to_string(self).map_err(|e| Error::new(ErrorKind::InvalidData, e))?)
    }

    /// Read a manifest from a specified file path.
    pub async fn from_file(filepath: &Path) -> Result<Self, Error> {
        if !filepath.is_file() {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("File not found {:?}", filepath),
            ));
        }

        let mut file = File::open(filepath).await?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;

        let manifest: Manifest = match filepath.extension() {
            Some(ext) => match ext.to_str() {
                Some("json") => Manifest::from_json(&contents).await?,
                Some("toml") => Manifest::from_toml(&contents).await?,
                _ => {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "Unsupported manifest file extension",
                    ))
                }
            },
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "Unknown manifest file extension",
                ))
            }
        };

        Ok(manifest)
    }
}
