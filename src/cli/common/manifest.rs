use std::{collections::HashMap, path::Path, time::SystemTime};

use log::info;

use crate::{
    checksum::Checksum,
    manifest::{Manifest, ManifestParser},
};

/// Default number of completed artifacts between manifest flushes.
pub const DEFAULT_FLUSH_BATCH_SIZE: usize = 25;

/// Returns `true` if the file at `filepath` has not been modified since the
/// given reference time (typically the manifest file's own mtime captured at
/// startup). When the file's mtime cannot be read (e.g. permission error),
/// returns `false` so the file gets reprocessed.
pub fn artifact_mtime_unchanged(filepath: &Path, reference_mtime: SystemTime) -> bool {
    filepath
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|file_mtime| file_mtime <= reference_mtime)
        .unwrap_or(false)
}

/// Serializes the full artifact map and writes it atomically to the manifest
/// file. Content is first written to a sibling temporary file, then renamed
/// into place so that a crash mid-write never corrupts the manifest.
pub async fn flush_manifest(
    manifest_filepath: &Path,
    manifest_parser: &dyn ManifestParser,
    artifacts: &HashMap<String, Checksum>,
) -> Result<(), std::io::Error> {
    info!(
        "Flushing manifest ({} artifacts) to {:?}",
        artifacts.len(),
        manifest_filepath
    );
    let content = manifest_parser
        .to_string(&Manifest {
            artifacts: artifacts.clone(),
        })
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let tmp_path = manifest_filepath.with_extension("tmp");
    tokio::fs::write(&tmp_path, content).await?;
    tokio::fs::rename(&tmp_path, manifest_filepath).await
}
