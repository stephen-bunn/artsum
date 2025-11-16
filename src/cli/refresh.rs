use std::{
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
    manifest::{Manifest, ManifestSource},
};

/// Configuration options for the manifest refresh operation.
///
/// Controls the behavior of the refresh command, including where to find
/// manifest files, performance tuning, and display options.
#[derive(Debug)]
pub struct RefreshOptions {
    /// Path to the directory containing files referenced in the manifest
    pub dirpath: PathBuf,

    /// Optional explicit path to the manifest file
    ///
    /// If not provided, the command will search for manifest files in `dirpath`
    pub manifest: Option<PathBuf>,

    /// Size of chunks to use when calculating checksums (in bytes)
    ///
    /// Larger chunks improve performance but use more memory
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

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    /// Error when 'best' algorithm is used with non-artsum format.
    #[error("The 'best' algorithm option is only supported with the 'artsum' manifest format, not '{0}'")]
    BestAlgorithmRequiresArtsumFormat(crate::manifest::ManifestFormat),

    #[error("Failed to join checksum generation task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    #[error("Unknown error occurred, {0}")]
    Unknown(#[from] anyhow::Error),
}

/// Represents the status of a refresh task.
#[derive(Debug, Clone)]
pub enum RefreshTaskStatus {
    /// Indicates that the checksum was updated.
    Updated { old: Checksum, new: Checksum },
    /// Indicates that the checksum remains unchanged.
    Unchanged { checksum: Checksum },
    /// Indicates that the file was removed.
    Removed,
}

impl RefreshTaskStatus {
    pub fn symbol(&self) -> &str {
        match self {
            RefreshTaskStatus::Updated { .. } => "✓",
            RefreshTaskStatus::Unchanged { .. } => "=",
            RefreshTaskStatus::Removed => "✗",
        }
    }
}

impl Display for RefreshTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.symbol())
    }
}

/// Represents the result of a refresh task.
#[derive(Debug, Clone)]
pub struct RefreshTaskResult {
    /// Name of the file processed by the task.
    pub filename: String,
    /// Status of the refresh task.
    pub status: RefreshTaskStatus,
}

impl TaskResult for RefreshTaskResult {}
impl DisplayResult for RefreshTaskResult {}
impl Display for RefreshTaskResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.status {
            RefreshTaskStatus::Updated { old, new } => {
                write!(
                    f,
                    "{} {}",
                    format!("{} {}", self.status, self.filename).green(),
                    format!("({} -> {})", old, new).dimmed()
                )
            }
            RefreshTaskStatus::Unchanged { checksum } => {
                write!(
                    f,
                    "{} {}",
                    format!("{} {}", self.status, self.filename).blue(),
                    format!("({})", checksum).dimmed()
                )
            }
            RefreshTaskStatus::Removed => write!(
                f,
                "{}",
                format!("{} {}", self.status, self.filename).yellow()
            ),
        }
    }
}

/// Represents an error encountered during a refresh task.
#[derive(Debug)]
pub struct RefreshTaskError {
    /// Name of the file that caused the error.
    pub filename: String,
    /// Error details.
    pub error: ChecksumError,
}

impl TaskError for RefreshTaskError {}
impl DisplayError for RefreshTaskError {}
impl Display for RefreshTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.filename.dimmed(), self.error)
    }
}

/// Counters for tracking the progress of refresh tasks.
pub struct RefreshTaskCounters {
    /// Total number of tasks.
    pub total: Arc<AtomicUsize>,
    /// Number of tasks that updated checksums.
    pub updated: Arc<AtomicUsize>,
    /// Number of tasks with unchanged checksums.
    pub unchanged: Arc<AtomicUsize>,
    /// Number of tasks that removed files.
    pub removed: Arc<AtomicUsize>,
    /// Number of tasks that encountered errors.
    pub error: Arc<AtomicUsize>,
}

impl TaskCounters for RefreshTaskCounters {}
impl DisplayCounters for RefreshTaskCounters {
    fn current(&self) -> usize {
        self.updated.load(Ordering::Relaxed)
            + self.unchanged.load(Ordering::Relaxed)
            + self.removed.load(Ordering::Relaxed)
            + self.error.load(Ordering::Relaxed)
    }

    fn total(&self) -> Option<usize> {
        Some(self.total.load(Ordering::Relaxed))
    }
}

/// Options for configuring individual refresh tasks.
struct RefreshTaskOptions {
    /// Path to the file to be processed.
    pub filepath: PathBuf,
    /// Existing checksum of the file.
    pub checksum: Checksum,
    /// Algorithm to use for checksum calculation.
    pub checksum_algorithm: Option<ChecksumAlgorithm>,
    /// Mode to use for checksum calculation.
    pub checksum_mode: Option<ChecksumMode>,
    /// Size of chunks to use for checksum calculation.
    pub chunk_size: usize,
}

impl TaskOptions for RefreshTaskOptions {}

/// Processes a refresh task asynchronously.
///
/// Calculates the checksum of the file and determines its status.
async fn task_processor(
    options: RefreshTaskOptions,
    counters: Arc<RefreshTaskCounters>,
) -> Result<RefreshTaskResult, RefreshTaskError> {
    let filepath = options.filepath.clone();
    let filename = String::from(filepath.to_string_lossy());
    let checksum = options.checksum.clone();
    let algorithm = options.checksum_algorithm.unwrap_or(checksum.algorithm);
    let mode = options.checksum_mode.unwrap_or(checksum.mode);

    if !options.filepath.is_file() {
        counters.removed.fetch_add(1, Ordering::Relaxed);
        return Ok(RefreshTaskResult {
            filename,
            status: RefreshTaskStatus::Removed,
        });
    }

    let new_checksum = Checksum::from_file(ChecksumOptions {
        filepath,
        algorithm,
        mode,
        chunk_size: Some(options.chunk_size),
        progress_callback: None,
    })
    .await;

    match new_checksum {
        Ok(new_checksum) => {
            let status = if new_checksum == checksum {
                counters.unchanged.fetch_add(1, Ordering::Relaxed);
                RefreshTaskStatus::Unchanged { checksum }
            } else {
                counters.updated.fetch_add(1, Ordering::Relaxed);
                RefreshTaskStatus::Updated {
                    old: checksum,
                    new: new_checksum,
                }
            };

            let task_result = RefreshTaskResult { filename, status };

            info!("{:?}", task_result);
            Ok(task_result)
        }
        Err(error) => {
            let task_error = RefreshTaskError { filename, error };

            error!("{:?}", task_error);
            counters.error.fetch_add(1, Ordering::Relaxed);
            Err(task_error)
        }
    }
}

/// Wraps the asynchronous task processor in a pinned future.
fn pinned_task_processor(
    options: RefreshTaskOptions,
    counters: Arc<RefreshTaskCounters>,
) -> TaskProcessorResult<RefreshTaskResult, RefreshTaskError> {
    Box::pin(async move { task_processor(options, counters).await })
}

struct RefreshDisplayContext {}

impl DisplayContext for RefreshDisplayContext {}

/// Processes display messages for the refresh operation.
///
/// Formats messages based on verbosity and message type.
fn display_message_processor(
    message: DisplayMessage<
        RefreshTaskResult,
        RefreshTaskError,
        RefreshTaskCounters,
        RefreshDisplayContext,
    >,
    verbosity: u8,
) -> Vec<String> {
    match message {
        DisplayMessage::Start(manifest_source, _context) => vec![format!(
            "Refreshing {} ({})",
            manifest_source.filepath.display(),
            manifest_source.format
        )],
        DisplayMessage::Result(result) => {
            if match result.status {
                RefreshTaskStatus::Updated { .. } => true,
                RefreshTaskStatus::Removed => true,
                RefreshTaskStatus::Unchanged { .. } => verbosity >= 1,
            } {
                return vec![format!("{}", result)];
            }

            vec![]
        }
        DisplayMessage::Error(error) => vec![format!("{}", error)],
        DisplayMessage::Progress {
            counters,
            current,
            total,
        } => {
            let mut parts = vec![];
            let updated = counters.updated.load(Ordering::Relaxed);
            let unchanged = counters.unchanged.load(Ordering::Relaxed);
            let removed = counters.removed.load(Ordering::Relaxed);
            if updated > 0 {
                parts.push(format!("{} updated", updated).green().to_string());
            }
            if unchanged > 0 {
                parts.push(format!("{} unchanged", unchanged).blue().to_string());
            }
            if removed > 0 {
                parts.push(format!("{} removed", removed).yellow().to_string());
            }

            if let Some(total) = total {
                parts.push(format!("[{}/{}]", current, total).dimmed().to_string());
            }

            vec![parts.join(" ")]
        }
        DisplayMessage::Exit => vec![],
    }
}

/// Refreshes the manifest and updates checksums.
///
/// Reads the manifest file, calculates checksums for files, and writes
/// the updated manifest back to disk.
pub async fn refresh(options: RefreshOptions) -> Result<(), RefreshError> {
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
            io::ErrorKind::InvalidData,
            format!(
                "Invalid manifest file format found at {:?}",
                manifest_filepath
            ),
        ))?
    } else {
        ManifestSource::from_path(&options.dirpath).ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "No valid manifest file format found in directory {:?}",
                dirpath
            ),
        ))?
    };

    let manifest_filepath = manifest_source.filepath.clone();
    let manifest_parser = manifest_source.parser();
    let manifest = manifest_parser.parse(&manifest_source).await?;

    // Validate that 'best' algorithm is only used with artsum format
    if manifest_source.format != crate::manifest::ManifestFormat::ARTSUM {
        for checksum in manifest.artifacts.values() {
            if checksum.algorithm == ChecksumAlgorithm::Best {
                return Err(RefreshError::BestAlgorithmRequiresArtsumFormat(
                    manifest_source.format,
                ));
            }
        }
    }

    let task_counters = Arc::new(RefreshTaskCounters {
        total: Arc::new(AtomicUsize::new(manifest.artifacts.len())),
        updated: Arc::new(AtomicUsize::new(0)),
        unchanged: Arc::new(AtomicUsize::new(0)),
        removed: Arc::new(AtomicUsize::new(0)),
        error: Arc::new(AtomicUsize::new(0)),
    });
    let mut task_manager = TaskManager::new(task_counters.clone(), pinned_task_processor)
        .with_task_capacity(manifest.artifacts.len())
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

    let display_context = RefreshDisplayContext {};
    display_manager
        .start(manifest_source, display_context)
        .await?;

    for (filename, old) in &manifest.artifacts {
        task_manager
            .spawn(RefreshTaskOptions {
                filepath: PathBuf::from(filename),
                checksum: old.clone(),
                checksum_algorithm: Some(old.algorithm),
                checksum_mode: Some(old.mode),
                chunk_size: options.chunk_size,
            })
            .await;
    }

    let mut artifacts = HashMap::new();
    for task in task_manager.tasks {
        let task_result = task.await?;
        match task_result {
            Ok(result) => {
                let filename = result.filename.clone();
                let status = result.status.clone();
                match status {
                    RefreshTaskStatus::Removed => (),
                    RefreshTaskStatus::Updated { old: _, new } => {
                        artifacts.insert(filename, new);
                    }
                    RefreshTaskStatus::Unchanged { checksum } => {
                        artifacts.insert(filename, checksum);
                    }
                };

                display_manager.report_result(result).await?;
            }
            Err(error) => display_manager.report_error(error).await?,
        }
    }

    info!("Writing manifest to {:?}", manifest_filepath);
    tokio::fs::write(
        manifest_filepath,
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

    if task_counters.error.load(Ordering::Relaxed) > 0 {
        std::process::exit(1);
    }

    Ok(())
}
