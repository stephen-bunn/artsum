use std::{
    io::Write,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use colored::Colorize;
use log::warn;

use crate::{cli::refresh::task::RefreshTaskStatus, manifest::ManifestSource};

use super::task::{RefreshTaskCounters, RefreshTaskError, RefreshTaskResult};

pub enum DisplayMessage {
    Start(ManifestSource),
    Result(RefreshTaskResult),
    Error(RefreshTaskError),
    Progress {
        total: usize,
        updated: usize,
        unchanged: usize,
        removed: usize,
        newline: bool,
    },
    Exit,
}

pub struct DisplayManager<'a> {
    tx: tokio::sync::mpsc::Sender<DisplayMessage>,
    counters: &'a Arc<RefreshTaskCounters>,
    counters_total: usize,
    display_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    progress_task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    #[allow(dead_code)]
    verbosity: u8,
    disabled: bool,
}

impl<'a> DisplayManager<'a> {
    pub fn new(
        buffer_size: usize,
        counters: &'a Arc<RefreshTaskCounters>,
        counters_total: usize,
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
            counters_total,
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
            self.counters_total,
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

    pub async fn report_task_result(&self, result: RefreshTaskResult) -> anyhow::Result<()> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Result(result)).await?;
        }

        Ok(())
    }

    pub async fn report_task_error(&self, error: RefreshTaskError) -> anyhow::Result<()> {
        if !self.disabled {
            self.tx.send(DisplayMessage::Error(error)).await?;
        }

        Ok(())
    }

    pub async fn report_progress(&self, newline: bool) -> anyhow::Result<()> {
        if !self.disabled {
            let updated = self.counters.updated.load(Ordering::Relaxed);
            let unchanged = self.counters.unchanged.load(Ordering::Relaxed);
            let removed = self.counters.removed.load(Ordering::Relaxed);
            let total = updated + unchanged + removed;
            self.tx
                .send(DisplayMessage::Progress {
                    updated,
                    unchanged,
                    removed,
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
                    "Refreshing {} ({})",
                    manifest_source.filepath.display(),
                    manifest_source.format
                );
            }
            DisplayMessage::Result(result) => {
                if match result.status {
                    RefreshTaskStatus::Updated { old: _, new: _ } => true,
                    RefreshTaskStatus::Removed => true,
                    RefreshTaskStatus::Unchanged { checksum: _ } => verbosity >= 1,
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
                updated,
                unchanged,
                removed,
                newline,
            } => {
                let mut parts = vec![];
                if updated > 0 {
                    parts.push(format!("{} updated", updated).green().to_string());
                }
                if unchanged > 0 {
                    parts.push(format!("{} unchanged", unchanged).blue().to_string());
                }
                if removed > 0 {
                    parts.push(format!("{} removed", removed).yellow().to_string());
                }

                parts.push(
                    format!("[{}/{}]", updated + unchanged + removed, total)
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
    counters: Arc<RefreshTaskCounters>,
    counters_total: usize,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));

    loop {
        interval.tick().await;

        let updated = counters.updated.load(Ordering::Relaxed);
        let unchanged = counters.unchanged.load(Ordering::Relaxed);
        let removed = counters.removed.load(Ordering::Relaxed);

        tx.send(DisplayMessage::Progress {
            total: counters_total,
            updated,
            unchanged,
            removed,
            newline: false,
        })
        .await?;
    }
}
