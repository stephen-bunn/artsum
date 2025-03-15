use std::{
    io::Write,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use colored::Colorize;
use log::warn;

use super::task::{GenerateTaskCounters, GenerateTaskError, GenerateTaskResult};
use crate::manifest::ManifestSource;

pub enum DisplayMessage {
    Start(ManifestSource),
    Result(GenerateTaskResult),
    Error(GenerateTaskError),
    Progress {
        success: usize,
        error: usize,
        newline: bool,
    },
    Exit,
}

pub struct DisplayManager<'a> {
    tx: tokio::sync::mpsc::Sender<DisplayMessage>,
    counters: &'a Arc<GenerateTaskCounters>,
    display_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    progress_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    verbosity: u8,
    disabled: bool,
}

impl<'a> DisplayManager<'a> {
    pub fn new(
        buffer_size: usize,
        counters: &'a Arc<GenerateTaskCounters>,
        verbosity: u8,
        disabled: bool,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(buffer_size);
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
            self.tx.send(DisplayMessage::Start(manifest_source)).await?
        }
        Ok(())
    }

    pub async fn report_task_result(
        &self,
        result: GenerateTaskResult,
    ) -> Result<(), anyhow::Error> {
        if !self.disabled && self.verbosity >= 1 {
            self.tx.send(DisplayMessage::Result(result)).await?;
        }
        Ok(())
    }

    pub async fn report_task_error(&self, error: GenerateTaskError) -> Result<(), anyhow::Error> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Error(error)).await?;
        }
        Ok(())
    }

    pub async fn report_progress(&self, newline: bool) -> Result<(), anyhow::Error> {
        if !self.disabled && self.verbosity >= 1 {
            self.tx
                .send(DisplayMessage::Progress {
                    success: self.counters.success.load(Ordering::Relaxed),
                    error: self.counters.error.load(Ordering::Relaxed),
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
    _verbosity: u8,
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
                    "Generating {} ({})",
                    manifest_source.filepath.canonicalize()?.display(),
                    manifest_source.format,
                );
            }
            DisplayMessage::Result(result) => {
                clear_progress(&mut progress_visible);
                println!("{}", result);
            }
            DisplayMessage::Error(error) => {
                clear_progress(&mut progress_visible);
                println!("{}", error);
            }
            DisplayMessage::Progress {
                success,
                error,
                newline,
            } => {
                clear_progress(&mut progress_visible);
                print!("{}", format!("{} added", success).green());
                if error > 0 {
                    print!(" {}", format!("{} errors", error).red());
                }

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
    counters: Arc<GenerateTaskCounters>,
) -> anyhow::Result<()> {
    let mut last_progress = 0;
    let mut interval = tokio::time::interval(Duration::from_millis(10));

    loop {
        interval.tick().await;

        let success = counters.success.load(Ordering::Relaxed);
        let error = counters.error.load(Ordering::Relaxed);

        if (success + error) != last_progress {
            last_progress = success + error;
            tx.send(DisplayMessage::Progress {
                success,
                error,
                newline: false,
            })
            .await?;
        }
    }
}
