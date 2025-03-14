mod display;
mod task;

use std::{collections::HashMap, path::PathBuf};

use display::DisplayManager;
use log::{debug, info};
use task::GenerateTaskBuilder;

use crate::{
    checksum::{Checksum, ChecksumAlgorithm, ChecksumError, ChecksumMode},
    manifest::{Manifest, ManifestFormat},
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
    #[error("Checksum Error: {0}")]
    ChecksumError(#[from] ChecksumError),

    #[error("IO Error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Task Join Error: {0}")]
    JoinError(#[from] tokio::task::JoinError),

    #[error("Pattern Error: {0}")]
    PatternError(#[from] glob::PatternError),

    #[error("Manifest Error: {0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    #[error("Unknown Error: {0}")]
    Unknown(#[from] anyhow::Error),
}

pub type GenerateResult<T> = Result<T, GenerateError>;

pub async fn generate(options: GenerateOptions) -> GenerateResult<()> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!("No directory exists at {:?}", options.dirpath).into());
    }

    let manifest_format = options.format.unwrap_or_default();
    let manifest_parser = manifest_format.parser();
    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&options.dirpath)));

    let checksum_algorithm = manifest_parser.algorithm().unwrap_or(
        options
            .algorithm
            .clone()
            .unwrap_or(ChecksumAlgorithm::default()),
    );

    if let Some(algorithm) = options.algorithm {
        if algorithm != checksum_algorithm {
            return Err(anyhow::anyhow!(
                "Unsupported algorithm {} for format {}",
                algorithm,
                manifest_format
            )
            .into());
        }
    }

    let mut generate_tasks = Vec::new();
    let generate_task_builder = GenerateTaskBuilder::new(
        options.max_workers,
        checksum_algorithm,
        options.mode.unwrap_or(ChecksumMode::default()),
        options.chunk_size,
    );

    let mut display_manager = DisplayManager::new(
        options.max_workers * 4,
        generate_task_builder.counters.clone(),
        options.verbosity,
        options.debug,
    );

    display_manager
        .report_start(manifest_format, manifest_filepath.clone())
        .await?;

    if options.show_progress {
        display_manager.start_progress_worker().await?;
    }

    for entry in glob::glob_with(
        format!("{}/**/*", options.dirpath.to_string_lossy()).as_str(),
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
                || manifest_filepath.ends_with(path.clone())
            {
                debug!("Skipping path {:?}", path);
                continue;
            }

            let checksum_task = generate_task_builder.build_task(path.clone());
            generate_tasks.push(checksum_task);
        }
    }

    let mut artifacts = HashMap::<String, Checksum>::new();
    for task in generate_tasks {
        let task_result = task.await?;
        match task_result {
            Ok(result) => {
                let checksum = result.checksum.clone();
                if let Some(relative_filepath) =
                    pathdiff::diff_paths(&result.filepath, &options.dirpath)
                {
                    artifacts.insert(relative_filepath.to_string_lossy().to_string(), checksum);
                    display_manager.report_task_result(result).await?;
                }
            }
            Err(error) => {
                display_manager.report_task_error(error).await?;
            }
        }
    }

    info!(
        "Writing manifest to {:?}",
        manifest_filepath.canonicalize()?
    );
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

    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_manager.report_exit(sync_tx).await?;
    display_manager.stop_progress_worker().await;
    sync_rx.await.unwrap();

    Ok(())
}
