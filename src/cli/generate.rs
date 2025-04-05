use std::{
    borrow::Cow,
    cmp::max,
    collections::HashMap,
    fmt::Display,
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use colored::Colorize;
use log::{debug, error, info};

use super::common::{
    display::{DisplayCounters, DisplayError, DisplayManager, DisplayMessage, DisplayResult},
    task::{TaskCounters, TaskError, TaskOptions, TaskProcessorResult, TaskResult},
};
use crate::{
    checksum::{Checksum, ChecksumAlgorithm, ChecksumError, ChecksumMode, ChecksumOptions},
    cli::common::task::TaskManager,
    manifest::{Manifest, ManifestFormat, ManifestSource},
};

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

#[derive(Debug)]
pub struct GenerateTaskResult {
    pub filename: String,
    pub checksum: Checksum,
}

impl TaskResult for GenerateTaskResult {}
impl DisplayResult for GenerateTaskResult {}
impl Display for GenerateTaskResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            format!("{} {}", self.checksum, self.filename).dimmed()
        )
    }
}

#[derive(Debug)]
pub struct GenerateTaskError {
    pub filename: String,
    pub message: String,
    pub error: Option<ChecksumError>,
}

impl TaskError for GenerateTaskError {}
impl DisplayError for GenerateTaskError {}
impl Display for GenerateTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {}{}",
            self.filename.dimmed(),
            self.message.red(),
            if let Some(error) = &self.error {
                format!(" ({})", error).red()
            } else {
                "".into()
            }
        )
    }
}

pub struct GenerateTaskCounters {
    pub success: Arc<AtomicUsize>,
    pub error: Arc<AtomicUsize>,
}

impl TaskCounters for GenerateTaskCounters {}
impl DisplayCounters for GenerateTaskCounters {
    fn current(&self) -> usize {
        self.success.load(Ordering::Relaxed) + self.error.load(Ordering::Relaxed)
    }

    fn total(&self) -> Option<usize> {
        None
    }
}

struct GenerateTaskOptions {
    pub filepath: PathBuf,
    pub algorithm: ChecksumAlgorithm,
    pub mode: ChecksumMode,
    pub chunk_size: usize,
}

impl TaskOptions for GenerateTaskOptions {}

async fn task_processor(
    options: GenerateTaskOptions,
    counters: Arc<GenerateTaskCounters>,
) -> Result<GenerateTaskResult, GenerateTaskError> {
    let filepath = options.filepath.clone();
    let filename = String::from(filepath.to_string_lossy());
    let algorithm = options.algorithm;
    let mode = options.mode;

    if !filepath.is_file() {
        return Err(GenerateTaskError {
            filename,
            message: String::from("File does not exist"),
            error: Some(ChecksumError::IoError(io::Error::new(
                io::ErrorKind::NotFound,
                "File does not exist",
            ))),
        });
    }

    let checksum = Checksum::from_file(ChecksumOptions {
        filepath,
        algorithm,
        mode,
        chunk_size: Some(options.chunk_size),
        progress_callback: None,
    })
    .await;

    match checksum {
        Ok(checksum) => {
            let task_result = GenerateTaskResult { filename, checksum };

            info!("{:?}", task_result);
            counters.success.fetch_add(1, Ordering::Relaxed);
            Ok(task_result)
        }
        Err(err) => {
            let error = GenerateTaskError {
                filename,
                message: String::from("Failed to generate checksum"),
                error: Some(err),
            };

            error!("{:?}", error);
            counters.error.fetch_add(1, Ordering::Relaxed);
            Err(error)
        }
    }
}

fn pinned_task_processor(
    options: GenerateTaskOptions,
    counters: Arc<GenerateTaskCounters>,
) -> TaskProcessorResult<GenerateTaskResult, GenerateTaskError> {
    Box::pin(async move { task_processor(options, counters).await })
}

fn display_message_processor(
    message: DisplayMessage<GenerateTaskResult, GenerateTaskError, GenerateTaskCounters>,
    verbosity: u8,
) -> Option<String> {
    match message {
        DisplayMessage::Start(manifest_source) => Some(format!(
            "Generating {} ({})",
            manifest_source
                .filepath
                .canonicalize()
                .unwrap_or(manifest_source.filepath)
                .display(),
            manifest_source.format,
        )),
        DisplayMessage::Result(result) => {
            if verbosity >= 1 {
                return Some(format!("{}", result));
            }

            None
        }
        DisplayMessage::Error(error) => Some(format!("{}", error)),
        DisplayMessage::Progress { counters, .. } => {
            let mut parts = vec![
                format!("{} added", counters.success.load(Ordering::Relaxed))
                    .green()
                    .to_string(),
            ];

            let error = counters.error.load(Ordering::Relaxed);
            if error > 0 {
                parts.push(format!("{} errors", error).red().to_string());
            }

            Some(parts.join(" "))
        }
        DisplayMessage::Exit => None,
    }
}

pub async fn generate(options: GenerateOptions) -> Result<(), GenerateError> {
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
    let checksum_mode = options.mode.unwrap_or(ChecksumMode::default());
    let checksum_chunk_size = options.chunk_size;

    if let Some(algorithm) = options.algorithm {
        if algorithm != checksum_algorithm {
            return Err(GenerateError::UnsupportedManifestAlgorithm {
                algorithm,
                format: manifest_format,
                expected: checksum_algorithm,
            });
        }
    }

    let task_counters = Arc::new(GenerateTaskCounters {
        success: Arc::new(AtomicUsize::new(0)),
        error: Arc::new(AtomicUsize::new(0)),
    });
    let mut task_manager = TaskManager::<
        GenerateTaskResult,
        GenerateTaskError,
        GenerateTaskCounters,
        GenerateTaskOptions,
    >::new(task_counters.clone(), pinned_task_processor)
    .with_max_workers(options.max_workers);

    let mut display_manager = DisplayManager::<
        GenerateTaskResult,
        GenerateTaskError,
        GenerateTaskCounters,
    >::new(task_counters.clone(), display_message_processor)
    .with_disabled(options.no_display || options.debug)
    .with_verbosity(options.verbosity)
    .with_buffer_size(max(
        1024,
        options.max_workers * 8 + (options.max_workers.saturating_sub(4) * 4),
    ));

    if !options.no_progress && !options.debug {
        display_manager = display_manager.with_progress(10);
    }

    display_manager
        .start(ManifestSource {
            filepath: manifest_filepath.clone(),
            format: manifest_format,
        })
        .await?;

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
                    task_manager
                        .spawn(GenerateTaskOptions {
                            filepath: path,
                            algorithm: checksum_algorithm,
                            mode: checksum_mode,
                            chunk_size: checksum_chunk_size,
                        })
                        .await;
                }
            } else {
                task_manager
                    .spawn(GenerateTaskOptions {
                        filepath: path,
                        algorithm: checksum_algorithm,
                        mode: checksum_mode,
                        chunk_size: checksum_chunk_size,
                    })
                    .await;
            }
        }
    }

    let mut artifacts = HashMap::with_capacity(task_manager.tasks.len());
    for task in task_manager.tasks {
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
                    display_manager.report_result(result).await?;
                }
            }
            Err(error) => {
                display_manager.report_error(error).await?;
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

    display_manager.report_progress().await?;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_manager.stop(sync_tx).await?;
    sync_rx.await.unwrap();

    Ok(())
}
