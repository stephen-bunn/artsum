use std::{future::Future, pin::Pin, sync::Arc};

/// Common error types for task management operations.
#[derive(Debug, thiserror::Error)]
pub enum TaskManagerError {
    /// Error when a task join operation fails
    #[error("Failed to join task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    /// Error when acquiring a semaphore permit fails
    #[error("Failed to acquire task worker permit, {0}")]
    TaskPermitFailure(#[from] tokio::sync::AcquireError),
}

/// Trait for task error types.
///
/// Marker trait that indicates a type can be used as a task error.
/// All implementations must be Send + Sync + 'static.
pub trait TaskError: Send + Sync + 'static {}

/// Trait for task result types.
///
/// Marker trait that indicates a type can be used as a task result.
/// All implementations must be Send + Sync + 'static.
pub trait TaskResult: Send + Sync + 'static {}

/// Trait for task counter types.
///
/// Marker trait that indicates a type can be used to count and track tasks.
/// All implementations must be Send + Sync + 'static.
pub trait TaskCounters: Send + Sync + 'static {}

/// Trait for task option types.
///
/// Marker trait that indicates a type can be used to configure tasks.
/// All implementations must be Send + Sync + 'static.
pub trait TaskOptions: Send + Sync + 'static {}

/// Type alias for a future that returns a task result or error.
///
/// This represents an asynchronous operation that will eventually produce
/// either a successful result of type TResult or an error of type TError.
pub type TaskProcessorResult<TResult, TError> =
    Pin<Box<dyn Future<Output = Result<TResult, TError>> + Send + 'static>>;

/// Type alias for a function that processes tasks.
///
/// Takes task options TOptions and counters TCounters, and returns a future that resolves
/// to a result of type TResult or an error of type TError.
pub type TaskProcessor<TResult, TError, TCounters, TOptions> =
    fn(TOptions, Arc<TCounters>) -> TaskProcessorResult<TResult, TError>;

/// Manager for concurrent task execution.
///
/// Handles spawning and tracking of asynchronous tasks with configurable
/// concurrency limits. Acts as a builder with method chaining for configuration.
pub struct TaskManager<
    TResult: TaskResult,
    TError: TaskError,
    TCounters: TaskCounters,
    TOptions: TaskOptions,
> {
    /// Counter tracking task progress
    pub counters: Arc<TCounters>,

    /// Collection of spawned task handles
    pub tasks: Vec<tokio::task::JoinHandle<Result<TResult, TError>>>,

    /// Function that processes individual tasks
    task_processor: TaskProcessor<TResult, TError, TCounters, TOptions>,

    /// Semaphore controlling maximum concurrent workers
    worker_semaphore: Arc<tokio::sync::Semaphore>,
}

impl<TResult: TaskResult, TError: TaskError, TCounters: TaskCounters, TOptions: TaskOptions>
    TaskManager<TResult, TError, TCounters, TOptions>
{
    /// Creates a new TaskManager instance.
    ///
    /// # Arguments
    ///
    /// * `counters` - Shared counters for tracking task progress.
    /// * `task_processor` - Function to process individual tasks.
    pub fn new(
        counters: Arc<TCounters>,
        task_processor: TaskProcessor<TResult, TError, TCounters, TOptions>,
    ) -> Self {
        Self {
            counters,
            task_processor,
            tasks: Vec::new(),
            worker_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    /// Configures the maximum number of concurrent workers.
    ///
    /// # Arguments
    ///
    /// * `max_workers` - Maximum number of workers allowed.
    ///
    /// # Panics
    ///
    /// Panics if `max_workers` is set to 0.
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

    /// Configures the task capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Number of tasks to preallocate space for.
    pub fn with_task_capacity(mut self, capacity: usize) -> Self {
        self.tasks = Vec::with_capacity(capacity);
        self
    }

    /// Spawns a new task with the given options.
    ///
    /// # Arguments
    ///
    /// * `options` - Configuration options for the task.
    pub async fn spawn(&mut self, options: TOptions) {
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
