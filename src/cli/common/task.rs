use std::{future::Future, pin::Pin, sync::Arc};

use super::display::{DisplayCounters, DisplayError, DisplayResult};

#[derive(Debug, thiserror::Error)]
pub enum TaskManagerError {
    #[error("Failed to join task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    #[error("Failed to acquire task worker permit, {0}")]
    TaskPermitFailure(#[from] tokio::sync::AcquireError),
}

pub trait TaskError: DisplayError {}
pub trait TaskResult: DisplayResult {}
pub trait TaskCounters: DisplayCounters {}
pub trait TaskOptions: Send + Sync + 'static {}

pub type TaskProcessorResult<R, E> = Pin<Box<dyn Future<Output = Result<R, E>> + Send + 'static>>;
pub type TaskProcessor<R, E, C, O> = fn(O, Arc<C>) -> TaskProcessorResult<R, E>;

pub struct TaskManager<R: TaskResult, E: TaskError, C: TaskCounters, O: TaskOptions> {
    pub counters: Arc<C>,
    pub tasks: Vec<tokio::task::JoinHandle<Result<R, E>>>,
    task_processor: TaskProcessor<R, E, C, O>,
    worker_semaphore: Arc<tokio::sync::Semaphore>,
}

impl<R: TaskResult, E: TaskError, C: TaskCounters, O: TaskOptions> TaskManager<R, E, C, O> {
    pub fn new(counters: Arc<C>, task_processor: TaskProcessor<R, E, C, O>) -> Self {
        Self {
            counters,
            task_processor,
            tasks: Vec::new(),
            worker_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub fn with_max_workers(self, max_workers: usize) -> Self {
        if max_workers == 0 {
            panic!("max_workers must be greater than 0");
        }

        let available_permits = self.worker_semaphore.available_permits();
        if max_workers > available_permits {
            self.worker_semaphore
                .add_permits(max_workers - available_permits);
        } else if max_workers < available_permits {
            self.worker_semaphore
                .forget_permits(available_permits - max_workers);
        }

        self
    }

    pub fn with_task_capacity(mut self, capacity: usize) -> Self {
        self.tasks = Vec::with_capacity(capacity);
        self
    }

    pub async fn spawn(&mut self, options: O) {
        let worker_semaphore = self.worker_semaphore.clone();
        let counters = self.counters.clone();
        let task_processor = self.task_processor;

        let task = tokio::spawn(async move {
            let permit = worker_semaphore
                .acquire()
                .await
                .map_err(|e| TaskManagerError::TaskPermitFailure(e));

            let result = task_processor(options, counters).await;

            drop(permit);
            result
        });

        self.tasks.push(task);
    }
}
