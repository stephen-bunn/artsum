use std::{
    fmt::Display,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use colored::Colorize;
use log::{error, info};

use crate::{
    checksum::{Checksum, ChecksumAlgorithm, ChecksumError, ChecksumMode, ChecksumOptions},
    cli::common::display::{DisplayCounters, DisplayError, DisplayResult},
};

#[derive(Debug, Clone)]
pub enum RefreshTaskStatus {
    Updated { old: Checksum, new: Checksum },
    Unchanged { checksum: Checksum },
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

#[derive(Debug, Clone)]
pub struct RefreshTaskResult {
    pub filename: String,
    pub status: RefreshTaskStatus,
}

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

#[derive(Debug)]
pub struct RefreshTaskError {
    pub filename: String,
    pub error: ChecksumError,
}

impl DisplayError for RefreshTaskError {}
impl Display for RefreshTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.filename.dimmed(), self.error)
    }
}

pub struct RefreshTaskCounters {
    pub total: Arc<AtomicUsize>,
    pub updated: Arc<AtomicUsize>,
    pub unchanged: Arc<AtomicUsize>,
    pub removed: Arc<AtomicUsize>,
    pub error: Arc<AtomicUsize>,
}

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

pub struct RefreshTaskBuilder {
    worker_semaphore: Arc<tokio::sync::Semaphore>,
    chunk_size: usize,
    pub counters: Arc<RefreshTaskCounters>,
}

impl RefreshTaskBuilder {
    pub fn new(max_workers: usize, chunk_size: usize, total: usize) -> Self {
        let counters = Arc::new(RefreshTaskCounters {
            total: Arc::new(AtomicUsize::new(total)),
            updated: Arc::new(AtomicUsize::new(0)),
            unchanged: Arc::new(AtomicUsize::new(0)),
            removed: Arc::new(AtomicUsize::new(0)),
            error: Arc::new(AtomicUsize::new(0)),
        });

        Self {
            worker_semaphore: Arc::new(tokio::sync::Semaphore::new(max_workers)),
            chunk_size,
            counters,
        }
    }

    pub fn refresh_checksum(
        &self,
        filepath: &PathBuf,
        checksum: &Checksum,
        checksum_algorithm: Option<ChecksumAlgorithm>,
        checksum_mode: Option<ChecksumMode>,
    ) -> tokio::task::JoinHandle<Result<RefreshTaskResult, RefreshTaskError>> {
        let worker_permit = self.worker_semaphore.clone();
        let checksum_algorithm = checksum_algorithm.unwrap_or(checksum.algorithm);
        let checksum_mode = checksum_mode.unwrap_or(checksum.mode);
        let chunk_size = self.chunk_size;
        let counters = self.counters.clone();

        let filepath = filepath.clone();
        let filename = String::from(filepath.to_string_lossy());
        let checksum = checksum.clone();

        tokio::spawn(async move {
            let _permit = worker_permit
                .acquire()
                .await
                .expect("Failed to acquire worker permit");

            if !filepath.is_file() {
                counters.removed.fetch_add(1, Ordering::Relaxed);
                return Ok(RefreshTaskResult {
                    filename,
                    status: RefreshTaskStatus::Removed,
                });
            }

            let new_checksum = Checksum::from_file(ChecksumOptions {
                filepath,
                algorithm: checksum_algorithm,
                mode: checksum_mode,
                chunk_size: Some(chunk_size),
                progress_callback: None,
            })
            .await;

            match new_checksum {
                Ok(new_checksum) => {
                    if new_checksum == checksum {
                        let refresh_result = RefreshTaskResult {
                            filename,
                            status: RefreshTaskStatus::Unchanged {
                                checksum: new_checksum,
                            },
                        };

                        info!("{:?}", refresh_result);
                        counters.unchanged.fetch_add(1, Ordering::Relaxed);
                        Ok(refresh_result)
                    } else {
                        let refresh_result = RefreshTaskResult {
                            filename,
                            status: RefreshTaskStatus::Updated {
                                old: checksum,
                                new: new_checksum,
                            },
                        };

                        info!("{:?}", refresh_result);
                        counters.updated.fetch_add(1, Ordering::Relaxed);
                        Ok(refresh_result)
                    }
                }
                Err(err) => {
                    let refresh_error = RefreshTaskError {
                        filename,
                        error: err,
                    };

                    error!("{:?}", refresh_error);
                    counters.error.fetch_add(1, Ordering::Relaxed);
                    Err(refresh_error)
                }
            }
        })
    }
}
