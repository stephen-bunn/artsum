use std::{
    collections::HashMap,
    fmt::Display,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use colored::Colorize;
use log::{debug, error, info, warn};

use crate::{
    checksum::{Checksum, ChecksumAlgorithm, ChecksumMode, ChecksumOptions},
    error::ChecksumError,
    manifest::{Manifest, ManifestFormat, ManifestParser},
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

type GenerateResult<T> = Result<T, GenerateError>;

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
    ManifestError(#[from] crate::error::ManifestError),

    #[error("Unknown Error: {0}")]
    Unknown(#[from] anyhow::Error),
}

struct ChecksumTaskProgress {
    success: usize,
    error: usize,
}

struct ChecksumTaskCounters {
    success: Arc<AtomicUsize>,
    error: Arc<AtomicUsize>,
}

impl Display for ChecksumTaskProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", format!("{} added", self.success).green())?;
        if self.error > 0 {
            write!(f, " {}", format!("{} errors", self.error).red())?;
        }
        Ok(())
    }
}

enum DisplayMessage {
    Start {
        format: ManifestFormat,
        filepath: PathBuf,
    },
    Result(ChecksumTaskResult),
    Error(ChecksumTaskError),
    Progress(ChecksumTaskProgress),
    Exit,
}

async fn display_worker(
    mut rx: tokio::sync::mpsc::Receiver<DisplayMessage>,
) -> Result<(), anyhow::Error> {
    let mut progress_visible = false;
    while let Some(msg) = rx.recv().await {
        if progress_visible {
            print!("\r\x1B[K");
            progress_visible = false;
        }
        match msg {
            DisplayMessage::Start { format, filepath } => {
                println!(
                    "Generating {} ({})",
                    filepath.canonicalize()?.display(),
                    format,
                );
            }
            DisplayMessage::Result(result) => {
                println!("{}", result);
            }
            DisplayMessage::Error(error) => {
                println!("{}", error);
            }
            DisplayMessage::Progress(progress) => {
                print!("{}", progress);
                progress_visible = true;
            }
            DisplayMessage::Exit => {
                break;
            }
        }
        std::io::stdout().flush().unwrap();
    }

    Ok(())
}

async fn progress_worker(
    tx: tokio::sync::mpsc::Sender<DisplayMessage>,
    counters: Arc<ChecksumTaskCounters>,
) -> Result<(), anyhow::Error> {
    let mut last_progress = 0;
    let mut interval = tokio::time::interval(Duration::from_millis(10));

    loop {
        interval.tick().await;

        let success = counters.success.load(Ordering::Relaxed);
        let error = counters.error.load(Ordering::Relaxed);

        if (success + error) != last_progress {
            last_progress = success + error;
            tx.send(DisplayMessage::Progress(ChecksumTaskProgress {
                success,
                error,
            }))
            .await?;
        }
    }
}

struct DisplayManager {
    tx: tokio::sync::mpsc::Sender<DisplayMessage>,
    counters: Arc<ChecksumTaskCounters>,
    display_task: Option<tokio::task::JoinHandle<Result<(), anyhow::Error>>>,
    progress_task: Option<tokio::task::JoinHandle<Result<(), anyhow::Error>>>,
    verbosity: u8,
    disabled: bool,
}

impl DisplayManager {
    fn new(buffer_size: usize, verbosity: u8, disabled: bool) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(buffer_size);
        let counters = Arc::new(ChecksumTaskCounters {
            success: Arc::new(AtomicUsize::new(0)),
            error: Arc::new(AtomicUsize::new(0)),
        });

        let mut display_task = None;
        if !disabled {
            display_task = Some(tokio::spawn(display_worker(rx)));
        }

        let manager = Self {
            tx,
            counters,
            display_task,
            progress_task: None,
            verbosity,
            disabled,
        };

        manager
    }

    async fn start_progress_worker(&mut self) -> anyhow::Result<()> {
        if self.disabled {
            warn!("Attempted to start progress worker when display manager is disabled");
            return Ok(());
        }

        self.progress_task = Some(tokio::spawn(progress_worker(
            self.tx.clone(),
            self.counters.clone(),
        )));
        Ok(())
    }

    async fn stop_progress_worker(&mut self) {
        if let Some(progress_task) = self.progress_task.take() {
            progress_task.abort();
        }
    }

    async fn report_start(
        &self,
        format: ManifestFormat,
        filepath: PathBuf,
    ) -> Result<(), anyhow::Error> {
        if !self.disabled {
            self.tx
                .send(DisplayMessage::Start { format, filepath })
                .await?
        }
        Ok(())
    }

    async fn report_checksum_result(
        &self,
        result: ChecksumTaskResult,
    ) -> Result<(), anyhow::Error> {
        self.counters.success.fetch_add(1, Ordering::Relaxed);
        if !self.disabled && self.verbosity >= 1 {
            self.tx.send(DisplayMessage::Result(result)).await?;
        }
        Ok(())
    }

    async fn report_error(&self, error: ChecksumTaskError) -> Result<(), anyhow::Error> {
        self.counters.error.fetch_add(1, Ordering::Relaxed);
        if !self.disabled {
            self.tx.send(DisplayMessage::Error(error)).await?;
        }
        Ok(())
    }

    async fn report_exit(
        &mut self,
        sync_tx: tokio::sync::oneshot::Sender<()>,
    ) -> Result<(), anyhow::Error> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Exit).await?;
        }

        if let Some(display_task) = self.display_task.take() {
            display_task.abort();
        }

        sync_tx.send(()).unwrap();
        Ok(())
    }
}

#[derive(Debug)]
struct ChecksumTaskResult {
    filepath: String,
    checksum: Checksum,
}

impl Display for ChecksumTaskResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            format!("{} {}", self.checksum, self.filepath).dimmed()
        )
    }
}

#[derive(Debug)]
struct ChecksumTaskError {
    filepath: String,
    message: String,
    error: Option<ChecksumError>,
}

impl Display for ChecksumTaskError {
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

struct ChecksumTaskBuilder {
    worker_semaphore: Arc<tokio::sync::Semaphore>,
    checksum_algorithm: ChecksumAlgorithm,
    checksum_mode: ChecksumMode,
    chunk_size: usize,
}

impl ChecksumTaskBuilder {
    fn new(
        max_workers: usize,
        algorithm: ChecksumAlgorithm,
        mode: ChecksumMode,
        chunk_size: usize,
    ) -> Self {
        Self {
            worker_semaphore: Arc::new(tokio::sync::Semaphore::new(max_workers)),
            checksum_algorithm: algorithm,
            checksum_mode: mode,
            chunk_size,
        }
    }

    fn build_task(
        &self,
        filepath: PathBuf,
    ) -> tokio::task::JoinHandle<Result<ChecksumTaskResult, ChecksumTaskError>> {
        let worker_permit = self.worker_semaphore.clone();
        let checksum_algorithm = self.checksum_algorithm.clone();
        let checksum_mode = self.checksum_mode.clone();
        let chunk_size = self.chunk_size;

        tokio::spawn(async move {
            let _permit = worker_permit
                .acquire()
                .await
                .expect("Failed to acquire worker permit");

            let checksum = Checksum::from_file(ChecksumOptions {
                filepath: filepath.clone(),
                algorithm: checksum_algorithm.clone(),
                mode: checksum_mode.clone(),
                chunk_size: Some(chunk_size),
                progress_callback: None,
            })
            .await;

            match checksum {
                Ok(checksum) => {
                    let generation_result = ChecksumTaskResult {
                        checksum,
                        filepath: filepath.to_string_lossy().to_string(),
                    };

                    info!("{:?}", generation_result);
                    Ok(generation_result)
                }
                Err(error) => {
                    let generation_error = ChecksumTaskError {
                        filepath: filepath.to_string_lossy().to_string(),
                        message: "Failed to calculate checksum".to_string(),
                        error: Some(error),
                    };

                    error!("{:?}", generation_error);
                    Err(generation_error)
                }
            }
        })
    }
}

pub async fn generate(options: GenerateOptions) -> GenerateResult<()> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!("Directory does not exist: {:?}", options.dirpath).into());
    }

    let manifest_format = options.format.unwrap_or_default();
    let manifest_parser: Box<dyn ManifestParser> = manifest_format.get_parser();
    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&options.dirpath)));

    let checksum_algorithm = manifest_parser.algorithm().unwrap_or(
        options
            .algorithm
            .clone()
            .unwrap_or(ChecksumAlgorithm::default()),
    );

    if let Some(algorithm) = options.algorithm.clone() {
        if algorithm != checksum_algorithm {
            let message = format!(
                "Unsupported algorithm {} for format {}, using algorithm {}",
                algorithm, manifest_format, checksum_algorithm
            );
            warn!("{}", message);
            if !options.debug {
                println!("{}", message.yellow());
            }
        }
    }

    let mut display_manager =
        DisplayManager::new(options.max_workers * 4, options.verbosity, options.debug);
    display_manager
        .report_start(manifest_format, manifest_filepath.clone())
        .await?;

    if options.show_progress {
        display_manager.start_progress_worker().await?;
    }

    let mut checksum_tasks = Vec::new();
    let checksum_task_builder = ChecksumTaskBuilder::new(
        options.max_workers,
        checksum_algorithm.clone(),
        options.mode.unwrap_or(ChecksumMode::default()),
        options.chunk_size,
    );

    // let worker_semaphore = Arc::new(tokio::sync::Semaphore::new(options.max_workers));
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
                continue;
            }

            let checksum_task = checksum_task_builder.build_task(path.clone());
            checksum_tasks.push(checksum_task);
        }
    }

    let mut artifacts = HashMap::<String, Checksum>::new();
    for handle in checksum_tasks {
        let result = handle.await?;
        match result {
            Ok(result) => {
                let checksum = result.checksum.clone();
                if let Some(relative_filepath) =
                    pathdiff::diff_paths(&result.filepath, options.dirpath.clone())
                {
                    artifacts.insert(relative_filepath.to_string_lossy().to_string(), checksum);
                    display_manager.report_checksum_result(result).await?;
                } else {
                    display_manager.report_checksum_result(result).await?;
                }
            }
            Err(error) => {
                display_manager.report_error(error).await?;
            }
        }
    }

    info!(
        "Writing manifest file: {:?}",
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
