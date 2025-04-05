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
    display::{DisplayCounters, DisplayError, DisplayManager, DisplayMessage, DisplayResult},
    task::{TaskCounters, TaskError, TaskManager, TaskOptions, TaskProcessorResult, TaskResult},
};

use crate::{
    checksum::{Checksum, ChecksumError, ChecksumOptions},
    manifest::ManifestSource,
};

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

#[derive(Debug)]
pub enum VerifyTaskStatus {
    Valid,
    Invalid,
    Missing,
}

impl VerifyTaskStatus {
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

#[derive(Debug)]
pub struct VerifyTaskResult {
    pub status: VerifyTaskStatus,
    pub filename: String,
    pub actual: Option<Checksum>,
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

#[derive(Debug)]
pub struct VerifyTaskError {
    pub filepath: String,
    pub message: String,
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

pub struct VerifyTaskCounters {
    pub total: Arc<AtomicUsize>,
    pub valid: Arc<AtomicUsize>,
    pub invalid: Arc<AtomicUsize>,
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

struct VerifyTaskOptions {
    pub dirpath: PathBuf,
    pub filename: String,
    pub expected: Checksum,
    pub chunk_size: usize,
}

impl TaskOptions for VerifyTaskOptions {}

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
        Err(e) => {
            let error = VerifyTaskError {
                filepath: filename,
                message: String::from("Failed to calculate checksum"),
                error: Some(e),
            };

            error!("{:?}", error);
            Err(error)
        }
    }
}

fn pinned_task_processor(
    options: VerifyTaskOptions,
    counters: Arc<VerifyTaskCounters>,
) -> TaskProcessorResult<VerifyTaskResult, VerifyTaskError> {
    Box::pin(async move { task_processor(options, counters).await })
}

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

    let mut task_manager = TaskManager::<
        VerifyTaskResult,
        VerifyTaskError,
        VerifyTaskCounters,
        VerifyTaskOptions,
    >::new(task_counters.clone(), pinned_task_processor)
    .with_task_capacity(manifest.artifacts.len())
    .with_max_workers(options.max_workers);

    let mut display_manager =
        DisplayManager::<VerifyTaskResult, VerifyTaskError, VerifyTaskCounters>::new(
            task_counters.clone(),
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
