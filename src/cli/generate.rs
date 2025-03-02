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

pub struct GenerateOptions {
    pub dirpath: PathBuf,
    pub output: Option<PathBuf>,
    pub algorithm: Option<ChecksumAlgorithm>,
    pub format: Option<ManifestFormat>,
    pub chunk_size: u64,
    pub max_workers: usize,
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

impl Display for GenerateChecksumProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", format!("{} generated", self.success).green())?;
        if self.error > 0 {
            write!(f, " {}", format!("{} errors", self.error).red())?;
        }
        Ok(())
    }
}

enum DisplayMessage {
    Result(GenerateChecksumResult),
    Error(GenerateChecksumError),
    Progress(GenerateChecksumProgress),
    Exit {
        sync: tokio::sync::oneshot::Sender<()>,
        progress: GenerateChecksumProgress,
        filename: String,
    },
}

pub async fn generate(options: GenerateOptions) -> Result<(), anyhow::Error> {
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!("Directory does not exist"));
    }

    let mut checksum_handles = Vec::new();
    let checksum_algorithm = options.algorithm.unwrap_or_default();
    let manifest_format = options.format.unwrap_or_default();
    let manifest_parser: Box<dyn ManifestParser> = manifest_format.get_parser();
    let manifest_filepath = options
        .output
        .unwrap_or(manifest_parser.build_manifest_filepath(Some(&options.dirpath)));

    let (display_tx, mut display_rx) = tokio::sync::mpsc::channel::<DisplayMessage>(100);
    tokio::spawn(async move {
        let mut progress_visible = false;
        while let Some(msg) = display_rx.recv().await {
            if progress_visible {
                print!("\r\x1B[K");
                progress_visible = false;
            }
            match msg {
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
                    filename,
                } => {
                    print!("\r\x1B[K");
                    println!("{} {}", progress, filename);
                    sync.send(()).unwrap();
                    break;
                }
            }
            std::io::stdout().flush().unwrap();
        }
    });

    let success_count = Arc::new(AtomicUsize::new(0));
    let error_count = Arc::new(AtomicUsize::new(0));

    let progress_display_tx = display_tx.clone();
    let progress_success_count = success_count.clone();
    let progress_error_count = error_count.clone();
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

    let success_counter = success_count.clone();
    let error_counter = error_count.clone();

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
                success: success_count.load(Ordering::Relaxed),
                error: error_count.load(Ordering::Relaxed),
            },
            filename: manifest_filepath.to_string_lossy().to_string(),
        })
        .await?;

    display_progress_task.abort();
    sync_rx.await?;

    Ok(())
}
