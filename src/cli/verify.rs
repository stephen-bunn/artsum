use std::{
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

use crate::{checksum::Checksum, manifest::ManifestSource};

pub struct VerifyOptions {
    /// Path to the directory containing the files to verify
    pub dirpath: PathBuf,
    /// Chunk size to use for generating checksums
    pub chunk_size: u64,
    /// Maximum number of workers to use
    pub max_workers: usize,
    /// Verbosity level
    pub verbosity: u8,
}

pub enum VerifyChecksumStatus {
    Valid,
    Invalid,
    Missing,
}

impl VerifyChecksumStatus {
    pub fn symbol(&self) -> &str {
        match self {
            VerifyChecksumStatus::Valid => "✓",
            VerifyChecksumStatus::Invalid => "✗",
            VerifyChecksumStatus::Missing => "?",
        }
    }
}

pub struct VerifyChecksumResult {
    pub status: VerifyChecksumStatus,
    pub actual: Option<Checksum>,
    pub expected: Checksum,
    pub filename: String,
}

impl Display for VerifyChecksumResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.status {
            VerifyChecksumStatus::Valid => {
                write!(
                    f,
                    "{} {}",
                    self.status.symbol().green(),
                    self.filename.green()
                )
            }
            VerifyChecksumStatus::Invalid => {
                write!(
                    f,
                    "{} {} {}",
                    self.status.symbol().red().bold(),
                    self.filename.red().bold(),
                    format!(
                        "({} != {})",
                        format!("{}", self.actual.as_ref().unwrap()).red(),
                        format!("{}", self.expected),
                    )
                    .dimmed()
                )
            }
            VerifyChecksumStatus::Missing => {
                write!(
                    f,
                    "{} {}",
                    self.status.symbol().yellow(),
                    self.filename.yellow()
                )
            }
        }
    }
}

pub struct VerifyChecksumProgress {
    pub total: usize,
    pub valid: usize,
    pub invalid: usize,
    pub missing: usize,
}

// test
impl Display for VerifyChecksumProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = vec![format!("{} valid", self.valid).green().to_string()];

        if self.invalid > 0 {
            parts.push(format!("{} invalid", self.invalid).red().bold().to_string());
        }

        if self.missing > 0 {
            parts.push(format!("{} missing", self.missing).yellow().to_string());
        }

        parts.push(
            format!(
                "[{}/{}]",
                self.valid + self.invalid + self.missing,
                self.total
            )
            .dimmed()
            .to_string(),
        );

        write!(f, "{}", parts.join(" "))
    }
}

enum DisplayMessage {
    Result(VerifyChecksumResult),
    Progress(VerifyChecksumProgress),
    Exit {
        sync: tokio::sync::oneshot::Sender<()>,
        progress: VerifyChecksumProgress,
    },
}

pub async fn verify(options: VerifyOptions) -> Result<(), anyhow::Error> {
    if !options.dirpath.is_dir() {
        return Err(anyhow::anyhow!(
            "{} is not a directory",
            options.dirpath.display()
        ));
    }

    let manifest_source = ManifestSource::from_path(&options.dirpath);
    if manifest_source.is_none() {
        return Err(anyhow::anyhow!(
            "No manifest found in {}",
            options.dirpath.display()
        ));
    }

    let manifest_source = manifest_source.unwrap();
    let manifest_parser = manifest_source.get_parser();
    let manifest = manifest_parser
        .from_manifest_source(&manifest_source)
        .await?;

    let mut verify_handles: Vec<
        tokio::task::JoinHandle<Result<VerifyChecksumResult, anyhow::Error>>,
    > = Vec::new();

    let artifacts: Vec<_> = manifest
        .artifacts
        .iter()
        .map(|(c, f)| (c.clone(), f.clone()))
        .collect();

    let total_count = artifacts.len();
    let valid_count = Arc::new(AtomicUsize::new(0));
    let invalid_count = Arc::new(AtomicUsize::new(0));
    let missing_count = Arc::new(AtomicUsize::new(0));

    let report_valid = valid_count.clone();
    let report_invalid = invalid_count.clone();
    let report_missing = missing_count.clone();

    let (display_tx, mut display_rx) = tokio::sync::mpsc::channel::<DisplayMessage>(10000);
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
                DisplayMessage::Progress(progress) => {
                    print!("{}", progress.to_string());
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
    });

    let progress_display_tx = display_tx.clone();
    let display_progress_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        loop {
            interval.tick().await;
            progress_display_tx
                .send(DisplayMessage::Progress(VerifyChecksumProgress {
                    total: total_count,
                    valid: report_valid.load(Ordering::Relaxed),
                    invalid: report_invalid.load(Ordering::Relaxed),
                    missing: report_missing.load(Ordering::Relaxed),
                }))
                .await
                .unwrap();
        }
    });

    let worker_semaphore = Arc::new(tokio::sync::Semaphore::new(options.max_workers));
    for (filename, checksum) in artifacts {
        let valid_counter = valid_count.clone();
        let invalid_counter = invalid_count.clone();
        let missing_counter = missing_count.clone();

        let worker_permit = worker_semaphore.clone();
        let filepath = options.dirpath.join(&filename);
        let expected = checksum;

        let verify_handle = tokio::spawn(async move {
            let _permit = worker_permit
                .acquire()
                .await
                .expect("Failed to accquire worker permit");

            if !filepath.is_file() {
                missing_counter.fetch_add(1, Ordering::Relaxed);
                return Ok(VerifyChecksumResult {
                    status: VerifyChecksumStatus::Missing,
                    actual: None,
                    expected: expected.clone(),
                    filename,
                });
            }

            let actual = Checksum::from_file(
                &filepath,
                &expected.algorithm,
                Some(options.chunk_size),
                None,
            )
            .await?;
            let status = if actual == expected {
                valid_counter.fetch_add(1, Ordering::Relaxed);
                VerifyChecksumStatus::Valid
            } else {
                invalid_counter.fetch_add(1, Ordering::Relaxed);
                VerifyChecksumStatus::Invalid
            };

            Ok(VerifyChecksumResult {
                status,
                actual: Some(actual),
                expected,
                filename,
            })
        });

        verify_handles.push(verify_handle);
    }

    let result_display_tx = display_tx.clone();
    for handle in verify_handles {
        let result = handle.await??;
        match result.status {
            VerifyChecksumStatus::Invalid => {
                result_display_tx
                    .send(DisplayMessage::Result(result))
                    .await
                    .unwrap();
            }
            VerifyChecksumStatus::Missing => {
                if options.verbosity >= 1 {
                    result_display_tx
                        .send(DisplayMessage::Result(result))
                        .await
                        .unwrap();
                }
            }
            VerifyChecksumStatus::Valid => {
                if options.verbosity >= 2 {
                    result_display_tx
                        .send(DisplayMessage::Result(result))
                        .await
                        .unwrap();
                }
            }
        }
    }

    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_tx
        .send(DisplayMessage::Exit {
            sync: sync_tx,
            progress: VerifyChecksumProgress {
                total: total_count,
                valid: valid_count.load(Ordering::Relaxed),
                invalid: invalid_count.load(Ordering::Relaxed),
                missing: missing_count.load(Ordering::Relaxed),
            },
        })
        .await?;

    display_progress_task.abort();
    sync_rx.await?;

    if (invalid_count.load(Ordering::Relaxed)) > 0 {
        std::process::exit(1);
    }

    Ok(())
}
