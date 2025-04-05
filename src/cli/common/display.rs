use std::{fmt::Display, io::Write, sync::Arc, time::Duration};

use crate::manifest::ManifestSource;

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

/// Type alias for a function that processes display messages.
///
/// Takes a display message and verbosity level, and returns an
/// optional formatted string to display.
pub type DisplayMessageProcessor<DResult, DError, DCounters> =
    fn(DisplayMessage<DResult, DError, DCounters>, u8) -> Option<String>;

/// Types of messages that can be sent to the display manager.
///
/// Generic over result type DResult, error type DEerror, and counter type DCounters.
pub enum DisplayMessage<DResult: DisplayResult, DError: DisplayError, DCounters: DisplayCounters> {
    /// Indicates the start of an operation with a source context
    Start(ManifestSource),

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

    /// Indicates the end of all operations
    Exit,
}

/// Manages displaying results, errors, and progress information.
///
/// Acts as a central point for formatting and showing output from
/// concurrent operations. Provides progress bar functionality and
/// configurable verbosity levels.
pub struct DisplayManager<DResult: DisplayResult, DError: DisplayError, DCounters: DisplayCounters>
{
    /// Channel for sending display messages
    tx: Option<tokio::sync::mpsc::Sender<DisplayMessage<DResult, DError, DCounters>>>,

    /// Counters tracking task progress
    counters: Arc<DCounters>,

    /// Function that processes display messages into strings
    display_message_processor: DisplayMessageProcessor<DResult, DError, DCounters>,

    /// Task that consumes and displays messages
    display_message_consumer: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,

    /// Task that periodically produces progress messages
    progress_message_producer: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,

    /// Interval between progress updates in milliseconds
    progress_message_producer_refresh_millis: Option<u64>,

    /// When true, all display output is suppressed
    pub disabled: bool,

    /// Controls the level of detail in output
    pub verbosity: u8,

    /// Size of the message buffer
    pub buffer_size: usize,
}

impl<DResult: DisplayResult, DError: DisplayError, DCounters: DisplayCounters>
    DisplayManager<DResult, DError, DCounters>
{
    /// Creates a new DisplayManager with the given counters and message processor.
    ///
    /// # Arguments
    ///
    /// * `counters` - Shared counters for tracking progress
    /// * `message_processor` - Function that formats display messages into strings
    pub fn new(
        counters: Arc<DCounters>,
        message_processor: DisplayMessageProcessor<DResult, DError, DCounters>,
    ) -> Self {
        Self {
            tx: None,
            counters,
            display_message_processor: message_processor,
            disabled: false,
            verbosity: 0,
            display_message_consumer: None,
            progress_message_producer: None,
            progress_message_producer_refresh_millis: None,
            buffer_size: 1024,
        }
    }

    /// Sets the verbosity level of the display manager.
    ///
    /// Higher verbosity values show more detailed information.
    ///
    /// # Arguments
    ///
    /// * `verbosity` - The verbosity level to set
    pub fn with_verbosity(mut self, verbosity: u8) -> Self {
        self.verbosity = verbosity;
        self
    }

    /// Enables or disables all display output.
    ///
    /// # Arguments
    ///
    /// * `disabled` - When true, suppresses all output
    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the size of the message buffer.
    ///
    /// Larger buffer sizes allow more messages to be queued
    /// before blocking the sender.
    ///
    /// # Arguments
    ///
    /// * `buffer_size` - Size of the message buffer in number of messages
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Enables progress bar display with the specified refresh rate.
    ///
    /// # Arguments
    ///
    /// * `refresh_millis` - Milliseconds between progress updates
    pub fn with_progress(mut self, refresh_millis: u64) -> Self {
        self.progress_message_producer_refresh_millis = Some(refresh_millis);
        self
    }

    /// Starts the display manager with the given manifest source.
    ///
    /// Creates message channels and spawns background tasks for
    /// consuming and producing messages.
    ///
    /// # Arguments
    ///
    /// * `manifest_source` - Source information for the manifest being processed
    ///
    /// # Errors
    ///
    /// Returns an error if message sending fails
    pub async fn start(&mut self, manifest_source: ManifestSource) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }

        let (tx, rx) = tokio::sync::mpsc::channel(self.buffer_size);
        self.tx = Some(tx.clone());
        self.display_message_consumer = Some(tokio::spawn(display_message_consumer(
            rx,
            self.display_message_processor,
            self.verbosity,
        )));

        if let Some(refresh_millis) = self.progress_message_producer_refresh_millis {
            self.progress_message_producer = Some(tokio::spawn(progress_message_producer(
                tx.clone(),
                self.counters.clone(),
                refresh_millis,
            )));
        }

        tx.send(DisplayMessage::Start(manifest_source)).await?;
        Ok(())
    }

    /// Stops the display manager and waits for shutdown.
    ///
    /// Sends an exit message, aborts background tasks, and signals
    /// completion through the provided channel.
    ///
    /// # Arguments
    ///
    /// * `sync_tx` - Channel to signal when shutdown is complete
    ///
    /// # Errors
    ///
    /// Returns an error if sending the exit message fails
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

        sync_tx.send(()).unwrap();
        Ok(())
    }

    /// Reports a successful result to be displayed.
    ///
    /// # Arguments
    ///
    /// * `result` - The result to report
    ///
    /// # Errors
    ///
    /// Returns an error if sending the message fails
    pub async fn report_result(&self, result: DResult) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Result(result)).await?;
        }

        Ok(())
    }

    /// Reports an error to be displayed.
    ///
    /// # Arguments
    ///
    /// * `error` - The error to report
    ///
    /// # Errors
    ///
    /// Returns an error if sending the message fails
    pub async fn report_error(&self, error: DError) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Error(error)).await?;
        }
        Ok(())
    }

    /// Reports current progress to be displayed.
    ///
    /// Sends a progress message with the current counter values.
    ///
    /// # Errors
    ///
    /// Returns an error if sending the message fails
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

async fn display_message_consumer<
    DResult: DisplayResult,
    DError: DisplayError,
    DCounters: DisplayCounters,
>(
    mut rx: tokio::sync::mpsc::Receiver<DisplayMessage<DResult, DError, DCounters>>,
    message_processor: DisplayMessageProcessor<DResult, DError, DCounters>,
    verbosity: u8,
) -> anyhow::Result<()> {
    let mut progress_visible = false;
    let clear_progress = |visible: &mut bool| {
        if *visible {
            print!("\r\x1B[K");
            *visible = false;
        }
    };

    while let Some(message) = rx.recv().await {
        if matches!(message, DisplayMessage::Exit) {
            break;
        }

        clear_progress(&mut progress_visible);
        if matches!(message, DisplayMessage::Progress { .. }) {
            if let Some(message) = message_processor(message, verbosity) {
                print!("{}", message);
                progress_visible = true;
            } else {
                continue;
            }
        } else {
            if let Some(message) = message_processor(message, verbosity) {
                println!("{}", message);
            } else {
                continue;
            }
        }

        std::io::stdout().flush()?;
    }

    Ok(())
}

async fn progress_message_producer<
    DResult: DisplayResult,
    DError: DisplayError,
    DCounters: DisplayCounters,
>(
    tx: tokio::sync::mpsc::Sender<DisplayMessage<DResult, DError, DCounters>>,
    counters: Arc<DCounters>,
    refresh_millis: u64,
) -> anyhow::Result<()> {
    let mut last_progress = 0;
    let mut interval = tokio::time::interval(Duration::from_millis(refresh_millis));

    loop {
        interval.tick().await;

        if counters.current() != last_progress {
            last_progress = counters.current();
            tx.send(DisplayMessage::Progress {
                counters: counters.clone(),
                current: last_progress,
                total: counters.total(),
            })
            .await?;
        }
    }
}
