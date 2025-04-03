use std::{cmp::max, io, path::PathBuf, sync::atomic::Ordering, time::Duration};

use colored::Colorize;
use log::debug;
use task::{
    VerifyTaskBuilder, VerifyTaskCounters, VerifyTaskError, VerifyTaskResult, VerifyTaskStatus,
};

use super::common::display::{DisplayManager, DisplayMessage};
use crate::manifest::ManifestSource;

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
    /// No display output
    pub no_display: bool,
    /// No progress output
    pub no_progress: bool,
    /// Verbosity level
    pub verbosity: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    #[error("Failed to join checksum verification task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    #[error("Unknown error occurred, {0}")]
    Unknown(#[from] anyhow::Error),
}

pub type VerifyResult<T> = Result<T, VerifyError>;

fn display_message_processor(
    message: DisplayMessage<VerifyTaskResult, VerifyTaskError, VerifyTaskCounters>,
    verbosity: u8,
) -> Option<String> {
    match message {
        DisplayMessage::Start(manifest_source) => Some(format!(
            "Verifying {} ({})",
            manifest_source.filepath.display(),
            manifest_source.format
        )),
        DisplayMessage::Result(result) => {
            if match result.status {
                VerifyTaskStatus::Invalid => true,
                VerifyTaskStatus::Missing => verbosity >= 1,
                VerifyTaskStatus::Valid => verbosity >= 2,
            } {
                return Some(format!("{}", result));
            }

            None
        }
        DisplayMessage::Error(error) => Some(format!("{}", error)),
        DisplayMessage::Progress {
            counters,
            current,
            total,
        } => {
            let mut parts = vec![format!("{} valid", counters.valid.load(Ordering::Relaxed))
                .green()
                .to_string()];
            if counters.invalid.load(Ordering::Relaxed) > 0 {
                parts.push(
                    format!("{} invalid", counters.invalid.load(Ordering::Relaxed))
                        .bold()
                        .red()
                        .to_string(),
                );
            }
            if counters.missing.load(Ordering::Relaxed) > 0 {
                parts.push(
                    format!("{} missing", counters.missing.load(Ordering::Relaxed))
                        .yellow()
                        .to_string(),
                );
            }

            if let Some(total) = total {
                parts.push(format!("[{}/{}]", current, total).dimmed().to_string());
            }

            Some(parts.join(" "))
        }
        DisplayMessage::Exit => None,
    }
}

pub async fn verify(options: VerifyOptions) -> VerifyResult<()> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No directory exists at {:?}", options.dirpath),
        )
        .into());
    }

    let dirpath = options.dirpath.clone();
    let manifest_source = if let Some(manifest_filepath) = options.manifest {
        ManifestSource::from_path(&manifest_filepath).ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No manifest file found at {:?}", manifest_filepath),
        ))?
    } else {
        ManifestSource::from_path(&options.dirpath).ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No manifest file found in directory {:?}", dirpath),
        ))?
    };

    let manifest_parser = manifest_source.parser();
    let manifest = manifest_parser.parse(&manifest_source).await?;

    let mut verify_tasks = Vec::with_capacity(manifest.artifacts.len());
    let verify_task_builder = VerifyTaskBuilder::new(
        options.max_workers,
        options.chunk_size,
        manifest.artifacts.len(),
    );

    let display_counters = verify_task_builder.counters.clone();
    let mut display_manager =
        DisplayManager::<VerifyTaskResult, VerifyTaskError, VerifyTaskCounters>::new(
            display_counters,
            display_message_processor,
        )
        .with_disabled(options.no_display || options.debug)
        .with_verbosity(options.verbosity)
        .with_buffer_size(max(
            1024,
            options.max_workers * 8 + (options.max_workers.saturating_sub(4) * 4),
        ));

    if !options.no_progress && !options.debug {
        display_manager = display_manager.with_progress(10);
    }

    display_manager.start(manifest_source).await?;

    let dirpath = options.dirpath.clone();
    for (filename, expected) in &manifest.artifacts {
        verify_tasks.push(verify_task_builder.verify_checksum(
            dirpath.clone(),
            &filename,
            &expected,
        ));
    }

    for task in verify_tasks {
        let task_result = task
            .await
            .or_else(|err| Err(VerifyError::TaskJoinFailure(err)))?;
        match task_result {
            Ok(result) => display_manager.report_result(result).await?,
            Err(error) => display_manager.report_error(error).await?,
        }
    }

    display_manager.report_progress().await?;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_manager.stop(sync_tx).await?;
    sync_rx.await.unwrap();

    if verify_task_builder.counters.invalid.load(Ordering::Relaxed) > 0 {
        std::process::exit(1);
    }

    Ok(())
}
