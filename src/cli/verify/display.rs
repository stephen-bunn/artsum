use std::{
    io::Write,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use colored::Colorize;
use log::warn;

use crate::{cli::verify::task::VerifyTaskStatus, manifest::ManifestSource};

use super::task::{VerifyTaskCounters, VerifyTaskError, VerifyTaskResult};

pub enum DisplayMessage {
    Start(ManifestSource),
    Result(VerifyTaskResult),
    Error(VerifyTaskError),
    Progress {
        total: usize,
        valid: usize,
        invalid: usize,
        missing: usize,
        newline: bool,
    },
    Exit,
}

pub struct DisplayManager<'a> {
    tx: tokio::sync::mpsc::Sender<DisplayMessage>,
    counters: &'a Arc<VerifyTaskCounters>,
    display_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    progress_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    #[allow(dead_code)]
    verbosity: u8,
    disabled: bool,
}

impl<'a> DisplayManager<'a> {
    pub fn new(
        buffier_size: usize,
        counters: &'a Arc<VerifyTaskCounters>,
        verbosity: u8,
        disabled: bool,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(buffier_size);
        let mut display_task = None;
        if !disabled {
            display_task = Some(tokio::spawn(display_worker(rx, verbosity)));
        }

        Self {
            tx,
            counters,
            display_task,
            progress_task: None,
            verbosity,
            disabled,
        }
    }

    pub async fn start_progress_worker(&mut self) -> anyhow::Result<()> {
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

    pub async fn stop_progress_worker(&mut self) {
        if let Some(progress_task) = self.progress_task.take() {
            progress_task.abort();
        }
    }

    pub async fn report_start(&self, manifest_source: ManifestSource) -> anyhow::Result<()> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Start(manifest_source)).await?;
        }

        Ok(())
    }

    pub async fn report_task_result(&self, result: VerifyTaskResult) -> anyhow::Result<()> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Result(result)).await?;
        }

        Ok(())
    }

    pub async fn report_task_error(&self, error: VerifyTaskError) -> anyhow::Result<()> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Error(error)).await?;
        }

        Ok(())
    }

    pub async fn report_progress(&self, newline: bool) -> anyhow::Result<()> {
        if !self.disabled {
            let valid = self.counters.valid.load(Ordering::Relaxed);
            let invalid = self.counters.invalid.load(Ordering::Relaxed);
            let missing = self.counters.missing.load(Ordering::Relaxed);
            let total = valid + invalid + missing;
            self.tx
                .send(DisplayMessage::Progress {
                    valid,
                    invalid,
                    missing,
                    total,
                    newline,
                })
                .await?;
        }

        Ok(())
    }

    pub async fn report_exit(
        &mut self,
        sync_tx: tokio::sync::oneshot::Sender<()>,
    ) -> anyhow::Result<()> {
        self.stop_progress_worker().await;
        if !self.disabled {
            if let Err(err) = tokio::time::timeout(
                Duration::from_millis(100),
                self.tx.send(DisplayMessage::Exit),
            )
            .await
            {
                return Err(anyhow::anyhow!("Failed to send exit message: {}", err));
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(display_task) = self.display_task.take() {
            let _ = display_task.await;
        }

        sync_tx.send(()).unwrap();
        Ok(())
    }
}

async fn display_worker(
    mut rx: tokio::sync::mpsc::Receiver<DisplayMessage>,
    verbosity: u8,
) -> anyhow::Result<()> {
    let mut progress_visible = false;
    fn clear_progress(progress_visible: &mut bool) {
        if *progress_visible {
            print!("\r\x1B[K");
            *progress_visible = false;
        }
    }

    while let Some(msg) = rx.recv().await {
        match msg {
            DisplayMessage::Start(manifest_source) => {
                clear_progress(&mut progress_visible);
                println!(
                    "Verifying {} ({})",
                    manifest_source.filepath.display(),
                    manifest_source.format
                );
            }
            DisplayMessage::Result(result) => {
                if match result.status {
                    VerifyTaskStatus::Invalid => true,
                    VerifyTaskStatus::Missing => verbosity >= 1,
                    VerifyTaskStatus::Valid => verbosity >= 2,
                } {
                    clear_progress(&mut progress_visible);
                    println!("{}", result);
                }
            }
            DisplayMessage::Error(error) => {
                clear_progress(&mut progress_visible);
                println!("{}", error);
            }
            DisplayMessage::Progress {
                total,
                valid,
                invalid,
                missing,
                newline,
            } => {
                let mut parts = vec![format!("{} valid", valid).green().to_string()];
                if invalid > 0 {
                    parts.push(format!("{} invalid", invalid).bold().red().to_string());
                }
                if missing > 0 {
                    parts.push(format!("{} missing", missing).yellow().to_string());
                }

                parts.push(
                    format!("[{}/{}]", valid + invalid + missing, total)
                        .dimmed()
                        .to_string(),
                );

                clear_progress(&mut progress_visible);
                print!("{}", parts.join(" "));
                if newline {
                    println!();
                }

                progress_visible = true;
            }
            DisplayMessage::Exit => {
                break;
            }
        }
        std::io::stdout().flush()?;
    }

    Ok(())
}

async fn progress_worker(
    tx: tokio::sync::mpsc::Sender<DisplayMessage>,
    counters: Arc<VerifyTaskCounters>,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));

    loop {
        interval.tick().await;

        let valid = counters.valid.load(Ordering::Relaxed);
        let invalid = counters.invalid.load(Ordering::Relaxed);
        let missing = counters.missing.load(Ordering::Relaxed);
        let total = valid + invalid + missing;

        tx.send(DisplayMessage::Progress {
            total,
            valid,
            invalid,
            missing,
            newline: false,
        })
        .await?;
    }
}
