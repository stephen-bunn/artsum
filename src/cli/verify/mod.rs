use std::{cmp::max, path::PathBuf, sync::atomic::Ordering, time::Duration};

use display::DisplayManager;
use log::debug;
use task::VerifyTaskBuilder;

use crate::{
    checksum::ChecksumError,
    manifest::{ManifestError, ManifestSource},
};

mod display;
mod task;

#[derive(Debug)]
/// Options for the verify command
pub struct VerifyOptions {
    /// Path to the directory containing the files to verify
    pub dirpath: PathBuf,
    /// Path to the manifest file to verify
    pub manifest: Option<PathBuf>,
    /// Chunk size to use for generating checksums
    pub chunk_size: usize,
    /// Maximum number of workers to use
    pub max_workers: usize,
    /// Debug output enabled
    pub debug: bool,
    /// Show progress output
    pub show_progress: bool,
    /// Verbosity level
    pub verbosity: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Manifest Error: {0}")]
    ManifestError(#[from] ManifestError),

    #[error("Checksum Error: {0}")]
    ChecksumError(#[from] ChecksumError),

    #[error("Task Join Error: {0}")]
    JoinError(#[from] tokio::task::JoinError),

    #[error("Unknown Error: {0}")]
    Unknown(#[from] anyhow::Error),
}

pub type VerifyResult<T> = Result<T, VerifyError>;

pub async fn verify(options: VerifyOptions) -> VerifyResult<()> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!("No directory exists at {:?}", options.dirpath).into());
    }

    let manifest_source = if let Some(manifest_filepath) = options.manifest {
        ManifestSource::from_path(&manifest_filepath).ok_or_else(|| {
            anyhow::anyhow!("No manifest file found at {}", manifest_filepath.display())
        })?
    } else {
        ManifestSource::from_path(&options.dirpath).ok_or_else(|| {
            anyhow::anyhow!(
                "No manifest file found in directory {}",
                options.dirpath.display()
            )
        })?
    };

    let manifest_parser = manifest_source.parser();
    let manifest = manifest_parser.parse(&manifest_source).await?;

    let mut verify_tasks = Vec::with_capacity(manifest.artifacts.len());
    let verify_task_builder = VerifyTaskBuilder::new(options.max_workers, options.chunk_size);

    let display_manager_buffer_size = max(
        1024,
        options.max_workers * 8 + (options.max_workers.saturating_sub(4) * 4),
    );
    let mut display_manager = DisplayManager::new(
        display_manager_buffer_size,
        &verify_task_builder.counters,
        options.verbosity,
        options.debug,
    );
    display_manager.report_start(manifest_source).await?;

    if options.show_progress {
        display_manager.start_progress_worker().await?;
    }

    let dirpath = options.dirpath.clone();
    for (filename, expected) in &manifest.artifacts {
        let verify_task =
            verify_task_builder.verify_checksum(dirpath.clone(), &filename, &expected);
        verify_tasks.push(verify_task);
    }

    for task in verify_tasks {
        let task_result = task.await?;
        match task_result {
            Ok(result) => display_manager.report_task_result(result).await?,
            Err(error) => display_manager.report_task_error(error).await?,
        }
    }

    display_manager.report_progress(true).await?;
    display_manager.stop_progress_worker().await;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_manager.report_exit(sync_tx).await?;
    sync_rx.await.unwrap();

    if verify_task_builder.counters.invalid.load(Ordering::Relaxed) > 0 {
        std::process::exit(1);
    }

    Ok(())
}
