use std::{fmt::Display, io::Write, sync::Arc, time::Duration};

use crate::manifest::ManifestSource;

pub trait DisplayCounters: Send + Sync + 'static {
    fn current(&self) -> usize;
    fn total(&self) -> Option<usize>;
}

pub trait DisplayResult: Display + Send + Sync + 'static {}
pub trait DisplayError: Display + Send + Sync + 'static {}

pub type DisplayMessageProcessor<R, E, C> = fn(DisplayMessage<R, E, C>, u8) -> Option<String>;

pub enum DisplayMessage<R: DisplayResult, E: DisplayError, C: DisplayCounters> {
    Start(ManifestSource),
    Exit,
    Result(R),
    Error(E),
    Progress {
        counters: Arc<C>,
        current: usize,
        total: Option<usize>,
    },
}

pub struct DisplayManager<R: DisplayResult, E: DisplayError, C: DisplayCounters> {
    tx: Option<tokio::sync::mpsc::Sender<DisplayMessage<R, E, C>>>,
    counters: Arc<C>,
    display_message_processor: DisplayMessageProcessor<R, E, C>,
    display_message_consumer: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    progress_message_producer: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    progress_message_producer_refresh_millis: Option<u64>,
    pub disabled: bool,
    pub verbosity: u8,
    pub buffer_size: usize,
}

impl<R: DisplayResult, E: DisplayError, C: DisplayCounters> DisplayManager<R, E, C> {
    pub fn new(counters: Arc<C>, message_processor: DisplayMessageProcessor<R, E, C>) -> Self {
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

    pub fn with_verbosity(mut self, verbosity: u8) -> Self {
        self.verbosity = verbosity;
        self
    }

    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    pub fn with_progress(mut self, refresh_millis: u64) -> Self {
        self.progress_message_producer_refresh_millis = Some(refresh_millis);
        self
    }

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

    pub async fn report_result(&self, result: R) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Result(result)).await?;
        }

        Ok(())
    }

    pub async fn report_error(&self, error: E) -> anyhow::Result<()> {
        if let Some(tx) = &self.tx {
            tx.send(DisplayMessage::Error(error)).await?;
        }
        Ok(())
    }

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

async fn display_message_consumer<R: DisplayResult, E: DisplayError, C: DisplayCounters>(
    mut rx: tokio::sync::mpsc::Receiver<DisplayMessage<R, E, C>>,
    message_processor: DisplayMessageProcessor<R, E, C>,
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

async fn progress_message_producer<R: DisplayResult, E: DisplayError, C: DisplayCounters>(
    tx: tokio::sync::mpsc::Sender<DisplayMessage<R, E, C>>,
    counters: Arc<C>,
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
