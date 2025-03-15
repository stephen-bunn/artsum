mod display;
mod task;

use std::{borrow::Cow, cmp::max, collections::HashMap, io, path::PathBuf, time::Duration};

use display::DisplayManager;
use log::{debug, info};
use task::GenerateTaskBuilder;

use crate::{
    checksum::{ChecksumAlgorithm, ChecksumMode},
    manifest::{Manifest, ManifestFormat, ManifestSource},
};

#[derive(Debug)]
/// Options for generating checksums
pub struct GenerateOptions {
    /// Path to the directory to generate checksums for
    pub dirpath: PathBuf,
    /// Optional output file path for the manifest
    pub output: Option<PathBuf>,
    /// Optional checksum algorithm to use
    pub algorithm: Option<ChecksumAlgorithm>,
    /// Optional format for the manifest
    pub format: Option<ManifestFormat>,
    /// Optional checksum mode to use
    pub mode: Option<ChecksumMode>,
    /// Size of chunks to use when calculating checksums
    pub chunk_size: usize,
    /// Maximum number of worker threads to use for checksum calculation
    pub max_workers: usize,
    /// Debug output is enabled
    pub debug: bool,
    /// Show progress output
    pub show_progress: bool,
    /// Verbosity level for output
    pub verbosity: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to glob pattern, {0}")]
    PatternGlobFailed(#[from] glob::PatternError),

    #[error("{0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    #[error("Unsupported manifest algorithm {algorithm} for format {format}, expected {expected}")]
    UnsupportedManifestAlgorithm {
        algorithm: ChecksumAlgorithm,
        format: ManifestFormat,
        expected: ChecksumAlgorithm,
    },

    #[error("Failed to join checksum generation task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    #[error("Unknown error occurred, {0}")]
    Unknown(#[from] anyhow::Error),
}

pub type GenerateResult<T> = Result<T, GenerateError>;

pub async fn generate(options: GenerateOptions) -> GenerateResult<()> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No directory exists at {:?}", options.dirpath),
        )
        .into());
    }

    let manifest_format = options.format.unwrap_or_default();
    let manifest_parser = manifest_format.parser();
    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&options.dirpath)));

    let checksum_algorithm = manifest_parser
        .algorithm()
        .unwrap_or_else(|| options.algorithm.unwrap_or_default());

    if let Some(algorithm) = options.algorithm {
        if algorithm != checksum_algorithm {
            return Err(GenerateError::UnsupportedManifestAlgorithm {
                algorithm,
                format: manifest_format,
                expected: checksum_algorithm,
            });
        }
    }

    let mut generate_tasks = Vec::new();
    let generate_task_builder = GenerateTaskBuilder::new(
        options.max_workers,
        checksum_algorithm,
        options.mode.unwrap_or(ChecksumMode::default()),
        options.chunk_size,
    );

    let display_manager_buffer_size = max(
        1024,
        options.max_workers * 8 + (options.max_workers.saturating_sub(4) * 4),
    );
    let mut display_manager = DisplayManager::new(
        display_manager_buffer_size,
        &generate_task_builder.counters,
        options.verbosity,
        options.debug,
    );

    display_manager
        .report_start(ManifestSource {
            filepath: manifest_filepath.clone(),
            format: manifest_format,
        })
        .await?;
    if options.show_progress {
        display_manager.start_progress_worker().await?;
    }

    let glob_pattern = options.dirpath.join("**/*");
    for entry in glob::glob_with(
        glob_pattern.to_str().unwrap_or("**/*"),
        glob::MatchOptions {
            case_sensitive: false,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        },
    )? {
        if let Ok(path) = entry {
            if !path.exists()
                || path.is_dir()
                || path.is_symlink()
                || options.dirpath.join(&path) == manifest_filepath
            {
                debug!("Skipping path {:?}", path);
                continue;
            }

            let generate_task = generate_task_builder.generate_checksum(&path);
            generate_tasks.push(generate_task);
        }
    }

    let mut artifacts = HashMap::with_capacity(generate_tasks.len());
    for task in generate_tasks {
        let task_result = task.await?;
        match task_result {
            Ok(result) => {
                let checksum = result.checksum.clone();
                if let Some(relative_filepath) =
                    pathdiff::diff_paths(&result.filepath, &options.dirpath)
                {
                    artifacts.insert(
                        Cow::from(relative_filepath.to_string_lossy()).into_owned(),
                        checksum,
                    );
                    display_manager.report_task_result(result).await?;
                }
            }
            Err(error) => {
                display_manager.report_task_error(error).await?;
            }
        }
    }

    info!("Writing manifest to {:?}", manifest_filepath);
    tokio::fs::write(
        &manifest_filepath,
        manifest_parser
            .to_string(&Manifest {
                version: None,
                artifacts,
            })
            .await?,
    )
    .await?;

    display_manager.report_progress(true).await?;
    display_manager.stop_progress_worker().await;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_manager.report_exit(sync_tx).await?;
    sync_rx.await.unwrap();

    Ok(())
}
