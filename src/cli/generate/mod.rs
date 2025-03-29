mod display;
mod task;

use std::{borrow::Cow, cmp::max, collections::HashMap, io, path::PathBuf, time::Duration};

use log::{debug, info};

use crate::{
    checksum::{ChecksumAlgorithm, ChecksumMode},
    manifest::{Manifest, ManifestFormat, ManifestSource},
};
use display::DisplayManager;
use task::GenerateTaskBuilder;

const DEFAULT_GLOB_PATTERN: &str = "**/*";

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
    /// Optional glob pattern to filter files
    pub glob: Option<String>,
    /// Optional list of file patterns to include in the manifest
    pub include: Option<Vec<String>>,
    /// Optional list of file patterns to exclude from the manifest
    pub exclude: Option<Vec<String>>,
    /// Size of chunks to use when calculating checksums
    pub chunk_size: usize,
    /// Maximum number of worker threads to use for checksum calculation
    pub max_workers: usize,
    /// Debug output is enabled
    pub debug: bool,
    /// Hide display output
    pub no_display: bool,
    /// Hide progress output
    pub no_progress: bool,
    /// Verbosity level for output
    pub verbosity: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    InvalidRegex(#[from] regex::Error),

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

    let include_patterns: Vec<regex::Regex> = match options.include {
        Some(include) => include
            .iter()
            .map(|pattern| regex::Regex::new(pattern).map_err(GenerateError::InvalidRegex))
            .collect::<Result<Vec<regex::Regex>, _>>()?,
        None => vec![],
    };

    let exclude_patterns: Vec<regex::Regex> = match options.exclude {
        Some(exclude) => exclude
            .iter()
            .map(|pattern| regex::Regex::new(pattern).map_err(GenerateError::InvalidRegex))
            .collect::<Result<Vec<regex::Regex>, _>>()?,
        None => vec![],
    };

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
        options.no_display || options.debug,
    );

    display_manager
        .report_start(ManifestSource {
            filepath: manifest_filepath.clone(),
            format: manifest_format,
        })
        .await?;
    if !options.no_progress && !options.debug {
        display_manager.start_progress_worker().await?;
    }

    let glob_pattern = options
        .dirpath
        .join(options.glob.unwrap_or(String::from(DEFAULT_GLOB_PATTERN)));
    for entry in glob::glob_with(
        glob_pattern.to_str().unwrap_or(DEFAULT_GLOB_PATTERN),
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

            let path_string = path.to_string_lossy();
            if !exclude_patterns.is_empty()
                && exclude_patterns.iter().any(|p| p.is_match(&path_string))
            {
                debug!("Excluding checksum generation for {:?}", path);
                continue;
            }

            if include_patterns.len() > 0 {
                if include_patterns.iter().any(|p| p.is_match(&path_string)) {
                    debug!("Including checksum generation for {:?}", path);
                    generate_tasks.push(generate_task_builder.generate_checksum(&path));
                }
            } else {
                generate_tasks.push(generate_task_builder.generate_checksum(&path));
            }
        }
    }

    let mut artifacts = HashMap::with_capacity(generate_tasks.len());
    for task in generate_tasks {
        let task_result = task.await?;
        match task_result {
            Ok(result) => {
                let checksum = result.checksum.clone();
                if let Some(relative_filepath) =
                    pathdiff::diff_paths(&result.filename, &options.dirpath)
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
