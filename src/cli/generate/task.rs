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

use crate::checksum::{Checksum, ChecksumAlgorithm, ChecksumError, ChecksumMode, ChecksumOptions};

#[derive(Debug)]
pub struct GenerateTaskResult {
    pub filename: String,
    pub checksum: Checksum,
}

impl Display for GenerateTaskResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            format!("{} {}", self.checksum, self.filename).dimmed()
        )
    }
}

#[derive(Debug)]
pub struct GenerateTaskError {
    pub filename: String,
    pub message: String,
    pub error: Option<ChecksumError>,
}

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

pub struct GenerateTaskCounters {
    pub success: Arc<AtomicUsize>,
    pub error: Arc<AtomicUsize>,
}

pub struct GenerateTaskBuilder {
    worker_semaphore: Arc<tokio::sync::Semaphore>,
    checksum_algorithm: ChecksumAlgorithm,
    checksum_mode: ChecksumMode,
    chunk_size: usize,
    pub counters: Arc<GenerateTaskCounters>,
}

impl GenerateTaskBuilder {
    pub fn new(
        max_workers: usize,
        checksum_algorithm: ChecksumAlgorithm,
        checksum_mode: ChecksumMode,
        chunk_size: usize,
    ) -> Self {
        let counters = Arc::new(GenerateTaskCounters {
            success: Arc::new(AtomicUsize::new(0)),
            error: Arc::new(AtomicUsize::new(0)),
        });

        Self {
            worker_semaphore: Arc::new(tokio::sync::Semaphore::new(max_workers)),
            checksum_algorithm,
            checksum_mode,
            chunk_size,
            counters,
        }
    }

    pub fn generate_checksum(
        &self,
        filepath: &PathBuf,
    ) -> tokio::task::JoinHandle<Result<GenerateTaskResult, GenerateTaskError>> {
        let worker_permit = self.worker_semaphore.clone();
        let checksum_algorithm = self.checksum_algorithm;
        let checksum_mode = self.checksum_mode;
        let chunk_size = self.chunk_size;
        let counters = self.counters.clone();

        let filepath = filepath.clone();
        let filename = String::from(filepath.to_string_lossy());

        tokio::spawn(async move {
            let _permit = worker_permit
                .acquire()
                .await
                .expect("Failed to acquire worker permit");

            let checksum = Checksum::from_file(ChecksumOptions {
                filepath,
                algorithm: checksum_algorithm.clone(),
                mode: checksum_mode.clone(),
                chunk_size: Some(chunk_size),
                progress_callback: None,
            })
            .await;

            match checksum {
                Ok(checksum) => {
                    let generation_result = GenerateTaskResult { checksum, filename };

                    info!("{:?}", generation_result);
                    counters.success.fetch_add(1, Ordering::Relaxed);
                    Ok(generation_result)
                }
                Err(err) => {
                    let generation_error = GenerateTaskError {
                        filename,
                        message: String::from("Failed to calculate checksum"),
                        error: Some(err),
                    };

                    error!("{:?}", generation_error);
                    counters.error.fetch_add(1, Ordering::Relaxed);
                    Err(generation_error)
                }
            }
        })
    }
}
