use std::{fmt::Display, sync::Arc, time::Duration};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use super::task::WorkerSlotRegistry;
use crate::manifest::ManifestSource;

/// Maximum number of per-worker spinner rows visible at once.
const MAX_VISIBLE_WORKER_SPINNERS: usize = 8;

/// Trait for display error types.
///
/// Marker trait indicating a type can be used as a display error.
/// All implementations must be Display + Send + 'static.
pub trait DisplayError: Display + Send + Sync + 'static {}

/// Trait for display result types.
///
/// Marker trait indicating a type can be used as a display result.
/// All implementations must be Display + Send + 'static.
pub trait DisplayResult: Display + Send + Sync + 'static {}

/// Trait for display counter types.
///
/// Defines methods to track progress of ongoing operations.
pub trait DisplayCounters: Send + Sync + 'static {
    /// Returns the current count of processed items.
    fn current(&self) -> usize;

    /// Returns the total number of items to process, if known.
    fn total(&self) -> Option<usize>;
}

/// Trait for display context data.
///
/// Marker trait indicating a type can be used as a context for display messages.
pub trait DisplayContext: Send + Sync + 'static {}

/// Type alias for a function that processes display messages.
///
/// Takes a display message and verbosity level, and returns an
/// array of strings to be displayed on separate lines.
pub type DisplayMessageProcessor<DResult, DError, DCounters, DContext> =
    fn(DisplayMessage<DResult, DError, DCounters, DContext>, u8) -> Vec<String>;

/// Types of messages that can be sent to the display manager.
///
/// Generic over result type DResult, error type DError, and counter type DCounters.
pub enum DisplayMessage<
    DResult: DisplayResult,
    DError: DisplayError,
    DCounters: DisplayCounters,
    DContext: DisplayContext,
> {
    /// Indicates the start of an operation with a source context
    Start(ManifestSource, DContext),

    /// Contains a successful result to be displayed
    Result(DResult),

    /// Contains an error to be displayed
    Error(DError),

    /// Contains progress information about ongoing operations
    Progress {
        /// Counters tracking operation progress
        counters: Arc<DCounters>,

        /// Current number of processed items
        current: usize,

        /// Total number of items to process, if known
        total: Option<usize>,
    },

    /// Notifies the display layer that a worker slot's active file has changed.
    InProgress {
        /// Worker slot index (0-based)
        slot: usize,
        /// Current filename, or None when the slot becomes idle
        filename: Option<String>,
    },

    /// Indicates the end of all operations
    Exit,
}

/// Manages displaying results, errors, and progress information.
///
/// Acts as a central point for formatting and showing output from
/// concurrent operations. Provides progress bar functionality and
/// configurable verbosity levels.
pub struct DisplayManager<
    DResult: DisplayResult,
    DError: DisplayError,
    DCounters: DisplayCounters,
    DContext: DisplayContext,
> {
    /// Channel for sending display messages
    tx: Option<tokio::sync::mpsc::Sender<DisplayMessage<DResult, DError, DCounters, DContext>>>,

    /// Counters tracking task progress
    counters: Arc<DCounters>,

    /// Function that processes display messages into strings
    display_message_processor: DisplayMessageProcessor<DResult, DError, DCounters, DContext>,

    /// Task that consumes and displays messages
    display_message_consumer: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,

    /// Task that periodically produces progress messages
    progress_message_producer: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,

    /// When true, all display output is suppressed
    pub disabled: bool,

    /// Controls the level of detail in output
    pub verbosity: u8,

    /// Size of the message buffer
    pub buffer_size: usize,

    /// When true, progress bars are hidden
    no_progress: bool,

    /// Worker slot registry for tracking in-progress files
    worker_slot_registry: Option<Arc<WorkerSlotRegistry>>,

    /// Maximum number of concurrent workers
    max_workers: usize,

    /// The overall progress bar or spinner
    overall_progress_bar: Option<ProgressBar>,

    /// The multi-progress container
    multi_progress: Option<Arc<MultiProgress>>,
}

impl<
        DResult: DisplayResult,
        DError: DisplayError,
        DCounters: DisplayCounters,
        DContext: DisplayContext,
    > DisplayManager<DResult, DError, DCounters, DContext>
{
    /// Creates a new DisplayManager with the given counters and message processor.
    pub fn new(
        counters: Arc<DCounters>,
        message_processor: DisplayMessageProcessor<DResult, DError, DCounters, DContext>,
    ) -> Self {
        Self {
            tx: None,
            counters,
            display_message_processor: message_processor,
            disabled: false,
            verbosity: 0,
            display_message_consumer: None,
            progress_message_producer: None,
            buffer_size: 1024,
            no_progress: false,
            worker_slot_registry: None,
            max_workers: 1,
            overall_progress_bar: None,
            multi_progress: None,
        }
    }

    /// Sets the verbosity level of the display manager.
    pub fn with_verbosity(mut self, verbosity: u8) -> Self {
        self.verbosity = verbosity;
        self
    }

    /// Enables or disables all display output.
    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the size of the message buffer.
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Enables or disables progress bar display.
    pub fn with_no_progress(mut self, no_progress: bool) -> Self {
        self.no_progress = no_progress;
        self
    }

    /// Sets the worker slot registry and max workers for per-file spinner display.
    pub fn with_worker_slots(
        mut self,
        registry: Arc<WorkerSlotRegistry>,
        max_workers: usize,
    ) -> Self {
        self.worker_slot_registry = Some(registry);
        self.max_workers = max_workers;
        self
    }

    /// Starts the display manager with the given manifest source.
    ///
    /// Creates message channels, builds indicatif progress bars/spinners,
    /// and spawns background tasks for consuming and producing messages.
    pub async fn start(
        &mut self,
        manifest_source: ManifestSource,
        context: DContext,
    ) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }

        let multi_progress = Arc::new(MultiProgress::new());
        if self.no_progress {
            multi_progress.set_draw_target(ProgressDrawTarget::hidden());
        }

        let total = self.counters.total();

        // Build overall bar or spinner
        let overall_progress_bar = match total {
            Some(n) => {
                let progress_bar = multi_progress.add(ProgressBar::new(n as u64));
                progress_bar.set_style(bar_style());
                progress_bar
            }
            None => {
                let progress_bar = multi_progress.add(ProgressBar::new_spinner());
                progress_bar.set_style(spinner_style());
                progress_bar
            }
        };

        // Build per-worker item rows (capped, hidden when verbose)
        let show_worker_rows = self.verbosity == 0;
        let visible_spinner_count = if show_worker_rows {
            self.max_workers.min(MAX_VISIBLE_WORKER_SPINNERS)
        } else {
            0
        };
        let overflow_count = if show_worker_rows {
            self.max_workers.saturating_sub(MAX_VISIBLE_WORKER_SPINNERS)
        } else {
            0
        };
        let worker_spinners: Vec<ProgressBar> = (0..visible_spinner_count)
            .map(|_| {
                let pb = multi_progress.add(ProgressBar::new_spinner());
                pb.set_style(worker_item_style());
                pb
            })
            .collect();

        let overflow_progress_bar = if overflow_count > 0 {
            let progress_bar = multi_progress.add(ProgressBar::new_spinner());
            progress_bar.set_style(overflow_style());
            progress_bar.set_message(format!("(+{} more workers)", overflow_count));
            Some(progress_bar)
        } else {
            None
        };

        let (tx, rx) = tokio::sync::mpsc::channel(self.buffer_size);
        self.tx = Some(tx.clone());
        self.overall_progress_bar = Some(overall_progress_bar.clone());
        self.multi_progress = Some(Arc::clone(&multi_progress));

        // Spawn consumer
        self.display_message_consumer = Some(tokio::spawn(display_message_consumer(
            rx,
            self.display_message_processor,
            self.verbosity,
            self.no_progress,
            Arc::clone(&multi_progress),
            overall_progress_bar.clone(),
            worker_spinners.clone(),
        )));

        // Spawn producer (if progress enabled)
        if !self.no_progress {
            self.progress_message_producer = Some(tokio::spawn(progress_message_producer(
                tx.clone(),
                self.counters.clone(),
                self.worker_slot_registry.clone(),
                10,
                overall_progress_bar.clone(),
                worker_spinners.clone(),
                overflow_progress_bar,
            )));
        }

        tx.send(DisplayMessage::Start(manifest_source, context))
            .await?;
        Ok(())
    }

    /// Stops the display manager and waits for shutdown.
    pub async fn stop(&self, sync_tx: tokio::sync::oneshot::Sender<()>) -> anyhow::Result<()> {
        if let Some(handle) = &self.progress_message_producer {
            handle.abort();
        }

        if let Some(tx) = &self.tx {
            if let Err(err) =
                tokio::time::timeout(Duration::from_millis(100), tx.send(DisplayMessage::Exit))
                    .await
            {
                return Err(anyhow::anyhow!("Failed to send exit message: {}", err));
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(handle) = &self.display_message_consumer {
            handle.abort();
        }

        // Clear all bars from terminal
        if let Some(progress_bar) = &self.overall_progress_bar {
            progress_bar.finish_and_clear();
        }
        if let Some(multi_progress) = &self.multi_progress {
            let _ = multi_progress.clear();
        }

        sync_tx.send(()).unwrap();
        Ok(())
    }

    /// Reports a successful result to be displayed.
    pub async fn report_result(&self, result: DResult) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Result(result)).await?;
        }

        Ok(())
    }

    /// Reports an error to be displayed.
    pub async fn report_error(&self, error: DError) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Error(error)).await?;
        }
        Ok(())
    }

    /// Reports current progress to be displayed.
    pub async fn report_progress(&self) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Progress {
                counters: self.counters.clone(),
                current: self.counters.current(),
                total: self.counters.total(),
            })
            .await?;
        }
        Ok(())
    }
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template("{bar:40.cyan/dim}  {pos}/{len}  {msg}")
        .unwrap()
        .progress_chars("━╸ ")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.green}  {msg}")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", ""])
}

fn worker_item_style() -> ProgressStyle {
    ProgressStyle::with_template("  {msg:.dim}").unwrap()
}

fn overflow_style() -> ProgressStyle {
    ProgressStyle::with_template("  {msg:.dim}").unwrap()
}

async fn display_message_consumer<
    DResult: DisplayResult,
    DError: DisplayError,
    DCounters: DisplayCounters,
    DContext: DisplayContext,
>(
    mut rx: tokio::sync::mpsc::Receiver<DisplayMessage<DResult, DError, DCounters, DContext>>,
    message_processor: DisplayMessageProcessor<DResult, DError, DCounters, DContext>,
    verbosity: u8,
    no_progress: bool,
    multi_progress: Arc<MultiProgress>,
    overall_progress_bar: ProgressBar,
    worker_spinners: Vec<ProgressBar>,
) -> anyhow::Result<()> {
    while let Some(message) = rx.recv().await {
        match message {
            DisplayMessage::Exit => break,
            DisplayMessage::InProgress { slot, filename } => {
                if let Some(pb) = worker_spinners.get(slot) {
                    match filename {
                        Some(name) => pb.set_message(name),
                        None => pb.set_message(""),
                    }
                }
            }
            DisplayMessage::Progress {
                current,
                counters,
                total,
            } => {
                overall_progress_bar.set_position(current as u64);
                let status = message_processor(
                    DisplayMessage::Progress {
                        counters,
                        current,
                        total,
                    },
                    verbosity,
                );
                if let Some(msg) = status.first() {
                    overall_progress_bar.set_message(msg.clone());
                }
            }
            other => {
                for line in message_processor(other, verbosity) {
                    if no_progress {
                        println!("{}", line);
                    } else {
                        let _ = multi_progress.println(&line);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn progress_message_producer<
    DResult: DisplayResult,
    DError: DisplayError,
    DCounters: DisplayCounters,
    DContext: DisplayContext,
>(
    tx: tokio::sync::mpsc::Sender<DisplayMessage<DResult, DError, DCounters, DContext>>,
    counters: Arc<DCounters>,
    worker_slots: Option<Arc<WorkerSlotRegistry>>,
    refresh_millis: u64,
    overall_progress_bar: ProgressBar,
    _worker_spinners: Vec<ProgressBar>,
    _overflow_pb: Option<ProgressBar>,
) -> anyhow::Result<()> {
    let mut last_progress = 0;
    let mut last_snapshot: Vec<Option<String>> = Vec::new();
    let mut interval = tokio::time::interval(Duration::from_millis(refresh_millis));

    loop {
        interval.tick().await;

        // Tick overall bar for spinner animation (generate mode)
        overall_progress_bar.tick();

        // Send Progress message if counter changed
        let current = counters.current();
        if current != last_progress {
            last_progress = current;
            let _ = tx
                .send(DisplayMessage::Progress {
                    counters: counters.clone(),
                    current,
                    total: counters.total(),
                })
                .await;
        }

        // Diff slot snapshot and send InProgress messages for changed slots
        if let Some(registry) = &worker_slots {
            let snapshot = registry.snapshot();
            if last_snapshot.len() != snapshot.len() {
                last_snapshot = vec![None; snapshot.len()];
            }
            for (i, (new, old)) in snapshot.iter().zip(last_snapshot.iter()).enumerate() {
                if new != old {
                    let _ = tx
                        .send(DisplayMessage::InProgress {
                            slot: i,
                            filename: new.clone(),
                        })
                        .await;
                }
            }
            last_snapshot = snapshot;
        }
    }
}
