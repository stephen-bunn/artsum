use std::{
    cmp::max,
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
    checksum::{Checksum, ChecksumError, ChecksumOptions},
    manifest::ManifestSource,
};

/// Configuration options for verifying checksums.
///
/// Controls the behavior of the verify command, including file selection,
/// performance tuning, and display preferences.
#[derive(Debug)]
pub struct VerifyOptions {
    /// Path to the directory containing the files to verify
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

/// Possible errors that can occur during checksum verification.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// I/O errors when reading files or directories
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    /// Error when handling manifests
    #[error("{0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    /// Error when joining a task fails
    #[error("Failed to join checksum verification task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    /// Unknown or unexpected errors
    #[error("Unknown error occurred, {0}")]
    Unknown(#[from] anyhow::Error),
}

/// Status of a file verification task.
///
/// Represents the result of comparing a file's actual checksum
/// with its expected checksum from the manifest.
#[derive(Debug)]
pub enum VerifyTaskStatus {
    /// File exists and its checksum matches the expected value
    Valid,

    /// File exists but its checksum does not match the expected value
    Invalid,

    /// File specified in the manifest does not exist
    Missing,
}

impl VerifyTaskStatus {
    /// Returns a symbolic representation of the status.
    ///
    /// Used for concise display in terminal output.
    pub fn symbol(&self) -> &str {
        match self {
            VerifyTaskStatus::Valid => "✓",
            VerifyTaskStatus::Invalid => "✗",
            VerifyTaskStatus::Missing => "?",
        }
    }
}

impl Display for VerifyTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.symbol())
    }
}

/// Result of a verification task.
///
/// Contains the verification status and details about the
/// expected and actual checksums.
#[derive(Debug)]
pub struct VerifyTaskResult {
    /// Status of the verification (Valid, Invalid, or Missing)
    pub status: VerifyTaskStatus,

    /// Name of the file that was verified
    pub filename: String,

    /// Actual checksum of the file, if it exists
    pub actual: Option<Checksum>,

    /// Expected checksum from the manifest
    pub expected: Checksum,
}

impl TaskResult for VerifyTaskResult {}
impl DisplayResult for VerifyTaskResult {}
impl Display for VerifyTaskResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            VerifyTaskStatus::Valid => write!(
                f,
                "{} {}",
                format!("{} {}", self.status, self.filename).green(),
                format!("({})", self.expected).dimmed()
            ),
            VerifyTaskStatus::Invalid => {
                write!(
                    f,
                    "{} {}",
                    format!("{} {}", self.status, self.filename).bold().red(),
                    format!(
                        "({} != {})",
                        format!("{}", self.actual.as_ref().unwrap()).red(),
                        format!("{}", self.expected)
                    )
                    .dimmed()
                )
            }
            VerifyTaskStatus::Missing => write!(
                f,
                "{}",
                format!("{} {}", self.status, self.filename).yellow()
            ),
        }
    }
}

/// Error encountered during a verification task.
///
/// Contains details about what went wrong when verifying a file.
#[derive(Debug)]
pub struct VerifyTaskError {
    /// Path to the file that caused the error
    pub filepath: String,

    /// Human-readable error message
    pub message: String,

    /// Underlying checksum error, if available
    pub error: Option<ChecksumError>,
}

impl TaskError for VerifyTaskError {}
impl DisplayError for VerifyTaskError {}
impl Display for VerifyTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {}{}",
            self.filepath.dimmed(),
            self.message.red(),
            if let Some(error) = &self.error {
                format!(" ({})", error).red()
            } else {
                "".into()
            }
        )
    }
}

/// Counters for tracking the progress of verification tasks.
///
/// Maintains atomic counts of files in different verification states.
pub struct VerifyTaskCounters {
    /// Total number of files to verify
    pub total: Arc<AtomicUsize>,

    /// Number of files with valid checksums
    pub valid: Arc<AtomicUsize>,

    /// Number of files with invalid checksums
    pub invalid: Arc<AtomicUsize>,

    /// Number of files that are missing
    pub missing: Arc<AtomicUsize>,
}

impl TaskCounters for VerifyTaskCounters {}
impl DisplayCounters for VerifyTaskCounters {
    fn current(&self) -> usize {
        self.valid.load(Ordering::Relaxed)
            + self.invalid.load(Ordering::Relaxed)
            + self.missing.load(Ordering::Relaxed)
    }

    fn total(&self) -> Option<usize> {
        Some(self.total.load(Ordering::Relaxed))
    }
}

/// Options for configuring a verification task.
///
/// Contains all information needed to verify a file against its expected checksum.
struct VerifyTaskOptions {
    /// Path to the directory containing the file
    pub dirpath: PathBuf,

    /// Name of the file to verify (relative to dirpath)
    pub filename: String,

    /// Expected checksum from the manifest
    pub expected: Checksum,

    /// Size of chunks to use for checksum calculation (in bytes)
    pub chunk_size: usize,
}

impl TaskOptions for VerifyTaskOptions {}

/// Processes a verification task asynchronously.
///
/// Compares the actual checksum of a file with its expected value from the manifest.
/// Updates the appropriate counters based on the verification result.
///
/// # Arguments
///
/// * `options` - Configuration options for the verification task
/// * `counters` - Shared counters for tracking verification progress
///
/// # Returns
///
/// A Result containing either a successful verification result or an error
async fn task_processor(
    options: VerifyTaskOptions,
    counters: Arc<VerifyTaskCounters>,
) -> Result<VerifyTaskResult, VerifyTaskError> {
    let filepath = options.dirpath.join(options.filename.clone());
    let filename = options.filename;
    let expected = options.expected.clone();

    if !filepath.is_file() {
        return Ok(VerifyTaskResult {
            status: VerifyTaskStatus::Missing,
            filename,
            actual: None,
            expected,
        });
    }

    let actual = Checksum::from_file(ChecksumOptions {
        filepath,
        algorithm: expected.algorithm.clone(),
        mode: expected.mode.clone(),
        chunk_size: Some(options.chunk_size),
        progress_callback: None,
    })
    .await;

    match actual {
        Ok(actual) => {
            let status = if actual == expected {
                counters.valid.fetch_add(1, Ordering::Relaxed);
                VerifyTaskStatus::Valid
            } else {
                counters.invalid.fetch_add(1, Ordering::Relaxed);
                VerifyTaskStatus::Invalid
            };

            let result = VerifyTaskResult {
                status,
                filename,
                actual: Some(actual),
                expected,
            };

            info!("{:?}", result);
            Ok(result)
        }
        Err(error) => {
            let error = VerifyTaskError {
                filepath: filename,
                message: String::from("Failed to calculate checksum"),
                error: Some(error),
            };

            error!("{:?}", error);
            Err(error)
        }
    }
}

/// Wraps the asynchronous task processor in a pinned future.
///
/// Creates a boxed and pinned future that can be awaited by the task manager.
///
/// # Arguments
///
/// * `options` - Configuration options for the verification task
/// * `counters` - Shared counters for tracking verification progress
///
/// # Returns
///
/// A pinned future that resolves to a verification result or error
fn pinned_task_processor(
    options: VerifyTaskOptions,
    counters: Arc<VerifyTaskCounters>,
) -> TaskProcessorResult<VerifyTaskResult, VerifyTaskError> {
    Box::pin(async move { task_processor(options, counters).await })
}

struct VerifyDisplayContext {}

impl DisplayContext for VerifyDisplayContext {}

/// Processes display messages for the verify operation.
///
/// Formats messages based on verbosity levels and message types.
/// Returns a formatted string for display or None if the message should be suppressed.
///
/// # Arguments
///
/// * `message` - The display message to process
/// * `verbosity` - The verbosity level (higher values show more details)
fn display_message_processor(
    message: DisplayMessage<
        VerifyTaskResult,
        VerifyTaskError,
        VerifyTaskCounters,
        VerifyDisplayContext,
    >,
    verbosity: u8,
) -> Vec<String> {
    match message {
        DisplayMessage::Start(manifest_source, _context) => vec![format!(
            "Verifying {} ({})",
            manifest_source.filepath.display(),
            manifest_source.format
        )],
        DisplayMessage::Result(result) => {
            if match result.status {
                VerifyTaskStatus::Invalid => true,
                VerifyTaskStatus::Missing => verbosity >= 1,
                VerifyTaskStatus::Valid => verbosity >= 2,
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

            vec![parts.join(" ")]
        }
        DisplayMessage::Exit => vec![],
    }
}

/// Verifies files against checksums in a manifest file.
///
/// Reads a manifest file, compares the expected checksums against the actual
/// checksums of files, and reports any mismatches or missing files.
///
/// # Arguments
///
/// * `options` - Configuration options for the verification operation
///
/// # Returns
///
/// Ok(()) if the verification completed successfully (regardless of file validity),
/// or an error if the verification process itself failed
pub async fn verify(options: VerifyOptions) -> Result<(), VerifyError> {
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

    let task_counters = Arc::new(VerifyTaskCounters {
        total: Arc::new(AtomicUsize::new(manifest.artifacts.len())),
        valid: Arc::new(AtomicUsize::new(0)),
        invalid: Arc::new(AtomicUsize::new(0)),
        missing: Arc::new(AtomicUsize::new(0)),
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

    let display_context = VerifyDisplayContext {};
    display_manager
        .start(manifest_source, display_context)
        .await?;

    for (filename, expected) in &manifest.artifacts {
        task_manager
            .spawn(VerifyTaskOptions {
                dirpath: options.dirpath.clone(),
                filename: filename.clone(),
                expected: expected.clone(),
                chunk_size: options.chunk_size,
            })
            .await;
    }

    for task in task_manager.tasks {
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

    if task_counters.invalid.load(Ordering::Relaxed) > 0 {
        std::process::exit(1);
    }

    Ok(())
}
