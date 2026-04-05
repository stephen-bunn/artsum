use std::{cmp::Ordering, future::Future, pin::Pin, sync::Arc};

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

/// Registry tracking which files are currently being processed by worker slots.
///
/// Each slot corresponds to a concurrency permit. When a worker acquires a slot,
/// it sets the filename being processed; when done, it clears the slot.
pub struct WorkerSlotRegistry {
    slots: Vec<std::sync::Mutex<Option<String>>>,
}

impl WorkerSlotRegistry {
    /// Creates a new registry with the given number of slots.
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            slots: (0..capacity).map(|_| std::sync::Mutex::new(None)).collect(),
        })
    }

    /// Finds the first idle slot, marks it occupied, and returns its index.
    pub fn acquire(&self) -> usize {
        for (i, slot) in self.slots.iter().enumerate() {
            let mut guard = slot.lock().unwrap();
            if guard.is_none() {
                *guard = Some(String::new());
                return i;
            }
        }
        0 // fallback: shouldn't happen with semaphore parity
    }

    /// Sets the filename displayed for a slot (None = release/idle).
    pub fn set(&self, index: usize, name: Option<String>) {
        if let Some(slot) = self.slots.get(index) {
            *slot.lock().unwrap() = name;
        }
    }

    /// Returns a point-in-time snapshot of all slot values.
    pub fn snapshot(&self) -> Vec<Option<String>> {
        self.slots
            .iter()
            .map(|s| s.lock().unwrap().clone())
            .collect()
    }
}

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

    /// Set of spawned tasks that yields results in completion order
    pub tasks: tokio::task::JoinSet<Result<TResult, TError>>,

    /// Registry tracking which files are being processed by each worker slot
    pub worker_slots: Arc<WorkerSlotRegistry>,

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
            tasks: tokio::task::JoinSet::new(),
            worker_slots: WorkerSlotRegistry::new(1),
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
    pub fn with_max_workers(mut self, max_workers: usize) -> Self {
        if max_workers == 0 {
            panic!("max_workers must be greater than 0");
        }

        let available_permits = self.worker_semaphore.available_permits();
        match max_workers.cmp(&available_permits) {
            Ordering::Greater => {
                self.worker_semaphore
                    .add_permits(max_workers - available_permits);
            }
            Ordering::Less => {
                self.worker_semaphore
                    .forget_permits(available_permits - max_workers);
            }
            Ordering::Equal => {}
        }

        self.worker_slots = WorkerSlotRegistry::new(max_workers);
        self
    }

    /// Spawns a new task with the given options.
    ///
    /// # Arguments
    ///
    /// * `options` - Configuration options for the task.
    /// * `filename` - Optional display name for tracking in-progress files.
    pub fn spawn(&mut self, options: TOptions, filename: Option<String>) {
        let worker_semaphore = self.worker_semaphore.clone();
        let counters = self.counters.clone();
        let task_processor = self.task_processor;
        let worker_slots = self.worker_slots.clone();

        self.tasks.spawn(async move {
            let permit = worker_semaphore
                .acquire()
                .await
                .map_err(TaskManagerError::TaskPermitFailure);

            let slot_index = worker_slots.acquire();
            worker_slots.set(slot_index, filename);

            let result = task_processor(options, counters).await;

            worker_slots.set(slot_index, None);
            drop(permit);
            result
        });
    }

    /// Returns the next completed task result, or `None` if all tasks are done.
    /// Results are yielded in completion order, not spawn order.
    pub async fn join_next(
        &mut self,
    ) -> Option<Result<Result<TResult, TError>, tokio::task::JoinError>> {
        self.tasks.join_next().await
    }
}
