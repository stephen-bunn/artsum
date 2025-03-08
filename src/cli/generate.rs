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

use crate::{
    checksum::{Checksum, ChecksumAlgorithm},
    error::ChecksumError,
    manifest::{Manifest, ManifestFormat, ManifestParser},
};

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
    /// Size of chunks to use when calculating checksums
    pub chunk_size: u64,
    /// Maximum number of worker threads to use for checksum calculation
    pub max_workers: usize,
    /// Verbosity level for output
    pub verbosity: u8,
}

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
        algorithm: ChecksumAlgorithm,
        filepath: PathBuf,
    },
    Result(GenerateChecksumResult),
    Error(GenerateChecksumError),
    Progress(GenerateChecksumProgress),
    Exit {
        sync: tokio::sync::oneshot::Sender<()>,
        progress: GenerateChecksumProgress,
        filepath: PathBuf,
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
            DisplayMessage::Start {
                format,
                algorithm,
                filepath,
            } => {
                println!(
                    "{}",
                    format!(
                        "Generating {} ({}) to {}",
                        format,
                        algorithm,
                        filepath.display()
                    )
                    .dimmed()
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
            DisplayMessage::Exit {
                sync,
                progress,
                filepath,
            } => {
                print!("\r\x1B[K");
                println!("{}", progress);
                println!("{}", filepath.display());
                sync.send(()).unwrap();
                break;
            }
        }
        std::io::stdout().flush().unwrap();
    }

    Ok(())
}

pub async fn generate(options: GenerateOptions) -> Result<(), anyhow::Error> {
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!("Directory does not exist"));
    }

    let (display_tx, display_rx) = tokio::sync::mpsc::channel::<DisplayMessage>(100);
    tokio::spawn(run_display_worker(display_rx));

    let mut checksum_handles = Vec::new();
    let manifest_format = options.format.unwrap_or_default();
    let manifest_parser: Box<dyn ManifestParser> = manifest_format.get_parser();
    let checksum_algorithm = manifest_parser
        .algorithm()
        .unwrap_or(options.algorithm.unwrap_or(ChecksumAlgorithm::default()));
    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&options.dirpath)));

    display_tx
        .send(DisplayMessage::Start {
            format: manifest_format,
            algorithm: checksum_algorithm.clone(),
            filepath: manifest_filepath.clone(),
        })
        .await?;

    let counters = GenerateChecksumCounters {
        success: Arc::new(AtomicUsize::new(0)),
        error: Arc::new(AtomicUsize::new(0)),
    };

    let progress_display_tx = display_tx.clone();
    let progress_success_count = counters.success.clone();
    let progress_error_count = counters.error.clone();
    let display_progress_task = tokio::spawn(async move {
        let mut last_progress = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        loop {
            interval.tick().await;
            let success = progress_success_count.load(Ordering::Relaxed);
            let error = progress_error_count.load(Ordering::Relaxed);
            if (success + error) != last_progress {
                last_progress = success + error;
                progress_display_tx
                    .send(DisplayMessage::Progress(GenerateChecksumProgress {
                        success,
                        error,
                    }))
                    .await
                    .unwrap()
            }
        }
    });

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
                let checksum =
                    Checksum::from_file(&path, &algorithm, Some(options.chunk_size), None).await;
                match checksum {
                    Ok(checksum) => Ok(GenerateChecksumResult { checksum, filepath }),
                    Err(error) => Err(GenerateChecksumError {
                        filepath,
                        error: Some(error),
                        message: "Failed to calculate checksum".to_string(),
                    }),
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
                    if options.verbosity >= 1 {
                        display_tx
                            .send(DisplayMessage::Result(GenerateChecksumResult {
                                filepath: relative_filepath.to_string_lossy().to_string(),
                                checksum: result.checksum,
                            }))
                            .await?;
                    }
                    success_counter.fetch_add(1, Ordering::Relaxed);
                    artifacts.insert(relative_filepath.to_string_lossy().to_string(), checksum);
                } else {
                    error_counter.fetch_add(1, Ordering::Relaxed);
                    display_tx
                        .send(DisplayMessage::Error(GenerateChecksumError {
                            filepath: result.filepath,
                            message: "Failed to calculate relative path".to_string(),
                            error: None,
                        }))
                        .await?;
                }
            }
            Err(error) => {
                error_counter.fetch_add(1, Ordering::Relaxed);
                display_tx.send(DisplayMessage::Error(error)).await?;
            }
        }
    }

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
    display_tx
        .send(DisplayMessage::Exit {
            sync: sync_tx,
            progress: GenerateChecksumProgress {
                success: counters.success.load(Ordering::Relaxed),
                error: counters.error.load(Ordering::Relaxed),
            },
            filepath: manifest_filepath.clone(),
        })
        .await?;

    display_progress_task.abort();
    sync_rx.await?;

    Ok(())
}
