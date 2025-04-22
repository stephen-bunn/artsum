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
    display::{
        DisplayContext, DisplayCounters, DisplayError, DisplayManager, DisplayMessage,
        DisplayResult,
    },
    task::{TaskCounters, TaskError, TaskManager, TaskOptions, TaskProcessorResult, TaskResult},
};
use crate::{
    checksum::{Checksum, ChecksumAlgorithm, ChecksumError, ChecksumMode, ChecksumOptions},
    manifest::{Manifest, ManifestFormat, ManifestSource},
};

/// Default glob pattern used for finding files when none is specified.
const DEFAULT_GLOB_PATTERN: &str = "**/*";

/// Configuration options for generating checksums.
///
/// Controls the behavior of the generate command, including file selection,
/// checksum algorithm and mode, and display preferences.
#[derive(Debug)]
pub struct GenerateOptions {
    /// Path to the directory to generate checksums for
    pub dirpath: PathBuf,

    /// Optional output file path for the manifest
    ///
    /// If not provided, the manifest will be written to a default location
    /// based on the chosen format.
    pub output: Option<PathBuf>,

    /// Optional checksum algorithm to use
    ///
    /// If not provided, the algorithm will be determined by the manifest format
    /// or default to SHA-256.
    pub algorithm: Option<ChecksumAlgorithm>,

    /// Optional format for the manifest
    ///
    /// If not provided, defaults to Artsum format.
    pub format: Option<ManifestFormat>,

    /// Optional checksum mode to use
    ///
    /// If not provided, defaults to the standard mode for the algorithm.
    pub mode: Option<ChecksumMode>,

    /// Optional glob pattern to filter files
    ///
    /// If not provided, uses "**/*" to match all files.
    pub glob: Option<String>,

    /// Optional list of file patterns to include in the manifest
    ///
    /// Only files matching these regex patterns will be included.
    pub include: Option<Vec<String>>,

    /// Optional list of file patterns to exclude from the manifest
    ///
    /// Files matching these regex patterns will be excluded.
    pub exclude: Option<Vec<String>>,

    /// Size of chunks to use when calculating checksums (in bytes)
    ///
    /// Larger chunks improve performance but use more memory.
    pub chunk_size: usize,

    /// Maximum number of concurrent worker threads for checksum calculation
    pub max_workers: usize,

    /// When true, enables debug output and disables progress display
    pub debug: bool,

    /// When true, suppresses all display output
    pub no_display: bool,

    /// When true, suppresses progress bar display
    pub no_progress: bool,

    /// Controls verbosity level of command output
    ///
    /// Higher values produce more detailed output
    pub verbosity: u8,
}

/// Possible errors that can occur during checksum generation.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// I/O errors when reading files or directories.
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    /// Error when compiling regex patterns.
    #[error("{0}")]
    InvalidRegex(#[from] regex::Error),

    /// Error when globbing for files.
    #[error("Failed to glob pattern, {0}")]
    PatternGlobFailed(#[from] glob::PatternError),

    /// Error when handling manifests.
    #[error("{0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    /// Error when algorithm doesn't match format requirements.
    #[error("Unsupported manifest algorithm {algorithm} for format {format}, expected {expected}")]
    UnsupportedManifestAlgorithm {
        algorithm: ChecksumAlgorithm,
        format: ManifestFormat,
        expected: ChecksumAlgorithm,
    },

    #[error("Failed to join checksum generation task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    /// Unknown or unexpected errors.
    #[error("Unknown error occurred, {0}")]
    Unknown(#[from] anyhow::Error),
}

/// Represents the result of a checksum generation task.
#[derive(Debug)]
pub struct GenerateTaskResult {
    /// Name of the file that was processed.
    pub filename: String,

    /// Calculated checksum of the file.
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

/// Represents an error encountered during a checksum generation task.
#[derive(Debug)]
pub struct GenerateTaskError {
    /// Name of the file that caused the error.
    pub filename: String,

    /// Human-readable error message.
    pub message: String,

    /// Underlying checksum error, if available.
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

/// Counters for tracking the progress of checksum generation tasks.
pub struct GenerateTaskCounters {
    /// Number of tasks that completed successfully.
    pub success: Arc<AtomicUsize>,

    /// Number of tasks that encountered errors.
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

/// Options for configuring a checksum generation task.
///
/// Contains all information needed to calculate a checksum for a file.
struct GenerateTaskOptions {
    /// Path to the file to generate a checksum for
    pub filepath: PathBuf,

    /// Algorithm to use for checksum calculation
    pub algorithm: ChecksumAlgorithm,

    /// Mode to use for checksum calculation
    pub mode: ChecksumMode,

    /// Size of chunks to use for checksum calculation (in bytes)
    pub chunk_size: usize,
}

impl TaskOptions for GenerateTaskOptions {}

/// Processes a checksum generation task asynchronously.
///
/// Calculates the checksum for a file and updates the appropriate counters.
/// Returns a Result with either the successful task result or an error.
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
        Err(error) => {
            let task_error = GenerateTaskError {
                filename,
                message: String::from("Failed to generate checksum"),
                error: Some(error),
            };

            error!("{:?}", task_error);
            counters.error.fetch_add(1, Ordering::Relaxed);
            Err(task_error)
        }
    }
}

/// Wraps the asynchronous task processor in a pinned future.
///
/// Creates a boxed and pinned future that can be awaited by the task manager.
fn pinned_task_processor(
    options: GenerateTaskOptions,
    counters: Arc<GenerateTaskCounters>,
) -> TaskProcessorResult<GenerateTaskResult, GenerateTaskError> {
    Box::pin(async move { task_processor(options, counters).await })
}

/// Context for displaying messages during the generate operation.
struct GenerateDisplayContext {
    /// The manifest source file being processed.
    pub manifest_filepath: PathBuf,
    /// The directory path where the manifest file is located.
    pub manifest_dirpath: PathBuf,
    /// The checksum algorithm used for the operation.
    pub checksum_algorithm: ChecksumAlgorithm,
    /// The checksum mode used for the operation.
    pub checksum_mode: ChecksumMode,
}

impl DisplayContext for GenerateDisplayContext {}

/// Processes display messages for the generate operation.
///
/// Formats messages based on verbosity levels and message types.
/// Returns a formatted string for display or None if the message should be suppressed.
fn display_message_processor(
    message: DisplayMessage<
        GenerateTaskResult,
        GenerateTaskError,
        GenerateTaskCounters,
        GenerateDisplayContext,
    >,
    verbosity: u8,
) -> Vec<String> {
    match message {
        DisplayMessage::Start(manifest_source, context) => {
            let mut lines = vec![format!(
                "Generating manifest for {} ({})",
                context.manifest_dirpath.to_string_lossy(),
                context
                    .manifest_filepath
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            )];

            lines.push(
                format!(
                    "Using manifest format {} with checksum algorithm {}{}",
                    manifest_source.format,
                    context.checksum_algorithm,
                    if context.checksum_mode == ChecksumMode::Text {
                        format!(" ({})", context.checksum_mode)
                    } else {
                        String::new()
                    }
                )
                .dimmed()
                .to_string(),
            );

            return lines;
        }
        DisplayMessage::Result(result) => {
            if verbosity >= 1 {
                return vec![format!("{}", result)];
            }

            vec![]
        }
        DisplayMessage::Error(error) => vec![format!("{}", error)],
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

            vec![parts.join(" ")]
        }
        DisplayMessage::Exit => vec![],
    }
}

/// Generates checksums for files and creates a manifest file.
///
/// Discovers files in the directory using glob pattern matching,
/// calculates checksums in parallel, and writes the results to a manifest file.
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
    let manifest_dirpath = options.dirpath.canonicalize()?;

    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&manifest_dirpath)));

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
    let mut task_manager = TaskManager::new(task_counters.clone(), pinned_task_processor)
        .with_max_workers(options.max_workers);

    let mut display_manager = DisplayManager::new(task_counters.clone(), display_message_processor)
        .with_disabled(options.no_display || options.debug)
        .with_verbosity(options.verbosity)
        .with_buffer_size(max(
            1024,
            options.max_workers * 8 + (options.max_workers.saturating_sub(4) * 4),
        ));

    if !options.no_progress && !options.debug {
        display_manager = display_manager.with_progress(10);
    }

    let display_context = GenerateDisplayContext {
        manifest_filepath: manifest_filepath.clone(),
        manifest_dirpath: manifest_dirpath.clone(),
        checksum_algorithm,
        checksum_mode,
    };

    display_manager
        .start(
            ManifestSource {
                filepath: manifest_filepath.clone(),
                format: manifest_format,
            },
            display_context,
        )
        .await?;

    let glob_pattern =
        manifest_dirpath.join(options.glob.unwrap_or(String::from(DEFAULT_GLOB_PATTERN)));
    for entry in glob::glob_with(
        glob_pattern.to_str().unwrap_or(DEFAULT_GLOB_PATTERN),
        glob::MatchOptions {
            case_sensitive: false,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        },
    )? {
        if let Ok(path) = entry {
            if !path.exists() || path.is_dir() || path.is_symlink() {
                debug!("Skipping path {:?}", path);
                continue;
            }

            let canonical_path = path.canonicalize()?;
            if canonical_path == manifest_filepath {
                debug!("Skipping manifest file {:?}", path);
                continue;
            }

            let canonical_path_string = canonical_path.to_string_lossy();
            if !exclude_patterns.is_empty()
                && exclude_patterns
                    .iter()
                    .any(|p| p.is_match(&canonical_path_string))
            {
                debug!("Excluding checksum generation for {:?}", path);
                continue;
            }

            if include_patterns.len() > 0 {
                if include_patterns
                    .iter()
                    .any(|p| p.is_match(&canonical_path_string))
                {
                    debug!("Including checksum generation for {:?}", path);
                    task_manager
                        .spawn(GenerateTaskOptions {
                            filepath: canonical_path,
                            algorithm: checksum_algorithm,
                            mode: checksum_mode,
                            chunk_size: checksum_chunk_size,
                        })
                        .await;
                }
            } else {
                task_manager
                    .spawn(GenerateTaskOptions {
                        filepath: canonical_path,
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
                    pathdiff::diff_paths(&result.filename, &manifest_dirpath)
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
