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

#[derive(Debug)]
struct GenerateChecksumResult {
    filepath: String,
    checksum: Checksum,
}

impl Display for GenerateChecksumResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            format!("{} {}", self.checksum, self.filepath).dimmed()
        )
    }
}

#[derive(Debug)]
struct GenerateChecksumError {
    filepath: String,
    message: String,
    error: Option<ChecksumError>,
}

impl Display for GenerateChecksumError {
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

struct GenerateChecksumProgress {
    success: usize,
    error: usize,
}

struct GenerateChecksumCounters {
    success: Arc<AtomicUsize>,
    error: Arc<AtomicUsize>,
}

impl Display for GenerateChecksumProgress {
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
    Result(GenerateChecksumResult),
    Error(GenerateChecksumError),
    Progress(GenerateChecksumProgress),
    Exit {
        sync: tokio::sync::oneshot::Sender<()>,
        progress: GenerateChecksumProgress,
    },
}

async fn run_display_worker(
    mut display_rx: tokio::sync::mpsc::Receiver<DisplayMessage>,
) -> Result<(), anyhow::Error> {
    let mut progress_visible = false;
    while let Some(msg) = display_rx.recv().await {
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
            DisplayMessage::Exit { sync, progress } => {
                print!("\r\x1B[K");
                println!("{}", progress);
                sync.send(()).unwrap();
                break;
            }
        }
        std::io::stdout().flush().unwrap();
    }

    Ok(())
}

pub async fn generate(options: GenerateOptions) -> Result<(), anyhow::Error> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!("Directory does not exist"));
    }

    let display_buffer_size = options.max_workers * 4;
    let (display_tx, display_rx) =
        tokio::sync::mpsc::channel::<DisplayMessage>(display_buffer_size);
    let using_display = !options.debug;
    if using_display {
        tokio::spawn(run_display_worker(display_rx));
    }

    let mut checksum_handles = Vec::new();
    let manifest_format = options.format.unwrap_or_default();
    let manifest_parser: Box<dyn ManifestParser> = manifest_format.get_parser();
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

    let checksum_mode = options.mode.unwrap_or(ChecksumMode::default());
    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&options.dirpath)));

    if using_display {
        display_tx
            .send(DisplayMessage::Start {
                format: manifest_format,
                filepath: manifest_filepath.clone(),
            })
            .await?;
    }

    let counters = GenerateChecksumCounters {
        success: Arc::new(AtomicUsize::new(0)),
        error: Arc::new(AtomicUsize::new(0)),
    };

    let mut display_progress_task: Option<tokio::task::JoinHandle<()>> = None;
    if options.show_progress {
        let progress_display_tx = display_tx.clone();
        let progress_success_count = counters.success.clone();
        let progress_error_count = counters.error.clone();
        display_progress_task = Some(tokio::spawn(async move {
            let mut last_progress = 0;
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            loop {
                interval.tick().await;
                let success = progress_success_count.load(Ordering::Relaxed);
                let error = progress_error_count.load(Ordering::Relaxed);
                if (success + error) != last_progress {
                    last_progress = success + error;
                    if using_display {
                        progress_display_tx
                            .send(DisplayMessage::Progress(GenerateChecksumProgress {
                                success,
                                error,
                            }))
                            .await
                            .unwrap()
                    }
                }
            }
        }));
    }

    let worker_semaphore = Arc::new(tokio::sync::Semaphore::new(options.max_workers));
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

            let algorithm = checksum_algorithm.clone();
            let worker_permit = worker_semaphore.clone();

            let checksum_handle: tokio::task::JoinHandle<
                Result<GenerateChecksumResult, GenerateChecksumError>,
            > = tokio::spawn(async move {
                // Acquire a worker permit to limit the number of concurrent checksum calculations
                let _permit = worker_permit
                    .acquire()
                    .await
                    .expect("Failed to acquire worker permit");

                let filepath = path.to_string_lossy().to_string();
                let checksum = Checksum::from_file(ChecksumOptions {
                    filepath: path.clone(),
                    algorithm: algorithm.clone(),
                    mode: checksum_mode.clone(),
                    chunk_size: Some(options.chunk_size),
                    progress_callback: None,
                })
                .await;
                match checksum {
                    Ok(checksum) => {
                        let checksum_result = GenerateChecksumResult { checksum, filepath };
                        info!("{:?}", checksum_result);
                        Ok(checksum_result)
                    }
                    Err(error) => {
                        let checksum_error = GenerateChecksumError {
                            filepath,
                            message: "Failed to calculate checksum".to_string(),
                            error: Some(error),
                        };
                        error!("{:?}", checksum_error);
                        Err(checksum_error)
                    }
                }
            });
            checksum_handles.push(checksum_handle);
        }
    }

    let success_counter = counters.success.clone();
    let error_counter = counters.error.clone();

    let mut artifacts = HashMap::<String, Checksum>::new();
    for handle in checksum_handles {
        let result = handle.await?;
        match result {
            Ok(result) => {
                let checksum = result.checksum.clone();
                if let Some(relative_filepath) =
                    pathdiff::diff_paths(&result.filepath, options.dirpath.clone())
                {
                    success_counter.fetch_add(1, Ordering::Relaxed);
                    artifacts.insert(relative_filepath.to_string_lossy().to_string(), checksum);
                    if options.verbosity >= 1 && using_display {
                        display_tx
                            .send(DisplayMessage::Result(GenerateChecksumResult {
                                filepath: relative_filepath.to_string_lossy().to_string(),
                                checksum: result.checksum,
                            }))
                            .await?;
                    }
                } else {
                    error_counter.fetch_add(1, Ordering::Relaxed);

                    if using_display {
                        display_tx
                            .send(DisplayMessage::Error(GenerateChecksumError {
                                filepath: result.filepath,
                                message: "Failed to calculate relative path".to_string(),
                                error: None,
                            }))
                            .await?;
                    }
                }
            }
            Err(error) => {
                error_counter.fetch_add(1, Ordering::Relaxed);
                if using_display {
                    display_tx.send(DisplayMessage::Error(error)).await?;
                }
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

    if using_display {
        let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
        display_tx
            .send(DisplayMessage::Exit {
                sync: sync_tx,
                progress: GenerateChecksumProgress {
                    success: counters.success.load(Ordering::Relaxed),
                    error: counters.error.load(Ordering::Relaxed),
                },
            })
            .await?;

        if let Some(display_progress_task) = display_progress_task {
            display_progress_task.abort();
        }

        if !options.debug {
            sync_rx.await?;
        }
    } else {
        if let Some(display_progress_task) = display_progress_task {
            display_progress_task.abort();
        }
    }

    Ok(())
}
