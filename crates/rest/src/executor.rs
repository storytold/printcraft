//! executor.rs — Bounded blocking executor with worker pool governance and queue wait timeouts.
//! ==============================================================================================
//!
//! Purpose:
//!     Guarantees system responsiveness under compute load by bounding worker concurrency,
//!     enforcing admission queue wait timeouts, and preventing event-loop / OS thread starvation.
//!
//! Layer:
//!     REST Service / Execution Governance Layer
//!
//! Key Input Dependencies:
//!     - pdfcraft_sdk::OperationExecutor
//!     - std::sync::{Arc, Condvar, Mutex}
//!     - thiserror::Error
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
//!     // Wrapped around an engine OperationExecutor
//!     ```
//!
//! Key Types & Functions Index:
//!     - ExecutionError: Errors during execution (QueueTimeout, ExecutionTimeout, WorkerPanic, Sdk)
//!     - ExecutorConfig: Configuration settings (max_concurrency, queue_timeout, exec_timeout)
//!     - BlockingExecutor: Concurrency governor wrapping OperationExecutor
//!     - BlockingExecutor::available_capacity: Count of currently idle worker slots
//!     - BlockingExecutor::execute_merge: Dispatches merge within bounded semaphore
//!     - BlockingExecutor::execute_split: Dispatches split within bounded semaphore
//!     - BlockingExecutor::execute_render: Dispatches render within bounded semaphore

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;

use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, OperationExecutor, RenderOptions, SdkError, SplitMode, SplitResult};

/// Errors encountered during blocking job execution.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("Queue wait timeout: executor worker capacity saturated (limit: {limit})")]
    QueueTimeout { limit: usize },

    #[error("Execution timeout: operation exceeded deadline ({0:?})")]
    ExecutionTimeout(Duration),

    #[error("Worker thread panicked: {0}")]
    WorkerPanic(String),

    #[error(transparent)]
    Sdk(#[from] SdkError),
}

impl ExecutionError {
    pub fn http_status(&self) -> u16 {
        match self {
            Self::QueueTimeout { .. } => 429,
            Self::ExecutionTimeout(_) => 504,
            Self::WorkerPanic(_) => 500,
            Self::Sdk(e) => e.http_status(),
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            Self::QueueTimeout { .. } => "WORKER_POOL_SATURATED",
            Self::ExecutionTimeout(_) => "EXECUTION_TIMEOUT",
            Self::WorkerPanic(_) => "WORKER_PANIC",
            Self::Sdk(e) => e.error_code(),
        }
    }
}

/// Counting semaphore with timeout support for bounding concurrent compute tasks.
struct Semaphore {
    permits: Mutex<usize>,
    cvar: Condvar,
    max_permits: usize,
}

impl Semaphore {
    fn new(permits: usize) -> Self {
        Self { permits: Mutex::new(permits), cvar: Condvar::new(), max_permits: permits }
    }

    fn acquire(&self, timeout: Duration) -> Result<Permit<'_>, ExecutionError> {
        let deadline = Instant::now() + timeout;
        let mut permits = self.permits.lock().map_err(|e| ExecutionError::WorkerPanic(e.to_string()))?;

        loop {
            if *permits > 0 {
                *permits -= 1;
                return Ok(Permit { semaphore: self });
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(ExecutionError::QueueTimeout { limit: self.max_permits });
            }

            let wait_dur = deadline.duration_since(now);
            let result = self.cvar.wait_timeout(permits, wait_dur).map_err(|e| ExecutionError::WorkerPanic(e.to_string()))?;

            permits = result.0;
            if result.1.timed_out() && *permits == 0 {
                return Err(ExecutionError::QueueTimeout { limit: self.max_permits });
            }
        }
    }

    fn release(&self) {
        if let Ok(mut permits) = self.permits.lock()
            && *permits < self.max_permits
        {
            *permits += 1;
            self.cvar.notify_one();
        }
    }

    fn available_permits(&self) -> usize {
        self.permits.lock().map(|p| *p).unwrap_or(0)
    }
}

/// RAII guard releasing a semaphore permit on drop.
struct Permit<'a> {
    semaphore: &'a Semaphore,
}

impl<'a> Drop for Permit<'a> {
    fn drop(&mut self) {
        self.semaphore.release();
    }
}

/// Configuration options for the blocking executor pool.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    pub max_concurrency: usize,
    pub queue_timeout: Duration,
    pub exec_timeout: Duration,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self { max_concurrency: 4, queue_timeout: Duration::from_secs(5), exec_timeout: Duration::from_secs(60) }
    }
}

/// Bounded worker executor for background document processing.
pub struct BlockingExecutor {
    engine: Arc<dyn OperationExecutor>,
    semaphore: Semaphore,
    config: ExecutorConfig,
}

impl BlockingExecutor {
    /// Creates a new blocking executor wrapping an engine port.
    pub fn new(engine: Arc<dyn OperationExecutor>, config: ExecutorConfig) -> Self {
        let permits = config.max_concurrency.max(1);
        Self { engine, semaphore: Semaphore::new(permits), config }
    }

    /// Number of currently idle worker execution slots.
    pub fn available_capacity(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Executes a merge operation within bounded worker capacity.
    pub fn execute_merge(&self, sources: &[DocumentSource], options: &MergeOptions) -> Result<DocumentResult, ExecutionError> {
        let _permit = self.semaphore.acquire(self.config.queue_timeout)?;

        let engine = Arc::clone(&self.engine);
        let src_vec = sources.to_vec();
        let opt = options.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("pdfcraft-worker-merge".into())
            .spawn(move || {
                let res = engine.execute_merge(&src_vec, &opt);
                let _ = tx.send(res);
            })
            .map_err(|e| ExecutionError::WorkerPanic(e.to_string()))?;

        match rx.recv_timeout(self.config.exec_timeout) {
            Ok(result) => result.map_err(ExecutionError::from),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(ExecutionError::ExecutionTimeout(self.config.exec_timeout)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let join_err = handle.join();
                let panic_msg = match join_err {
                    Err(p) => {
                        if let Some(s) = p.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = p.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "Unknown worker panic".to_string()
                        }
                    }
                    Ok(_) => "Worker disconnected unexpectedly".to_string(),
                };
                Err(ExecutionError::WorkerPanic(panic_msg))
            }
        }
    }

    /// Executes a split operation within bounded worker capacity.
    pub fn execute_split(&self, source: &DocumentSource, mode: &SplitMode) -> Result<SplitResult, ExecutionError> {
        let _permit = self.semaphore.acquire(self.config.queue_timeout)?;

        let engine = Arc::clone(&self.engine);
        let src = source.clone();
        let m = mode.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("pdfcraft-worker-split".into())
            .spawn(move || {
                let res = engine.execute_split(&src, &m);
                let _ = tx.send(res);
            })
            .map_err(|e| ExecutionError::WorkerPanic(e.to_string()))?;

        match rx.recv_timeout(self.config.exec_timeout) {
            Ok(result) => result.map_err(ExecutionError::from),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(ExecutionError::ExecutionTimeout(self.config.exec_timeout)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let join_err = handle.join();
                let panic_msg = match join_err {
                    Err(p) => {
                        if let Some(s) = p.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = p.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "Unknown worker panic".to_string()
                        }
                    }
                    Ok(_) => "Worker disconnected unexpectedly".to_string(),
                };
                Err(ExecutionError::WorkerPanic(panic_msg))
            }
        }
    }

    /// Executes a high-DPI page render operation within bounded worker capacity.
    pub fn execute_render(&self, source: &DocumentSource, page: usize, options: &RenderOptions) -> Result<DocumentResult, ExecutionError> {
        let _permit = self.semaphore.acquire(self.config.queue_timeout)?;

        let engine = Arc::clone(&self.engine);
        let src = source.clone();
        let opt = options.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("pdfcraft-worker-render".into())
            .spawn(move || {
                let res = engine.execute_render(&src, page, &opt);
                let _ = tx.send(res);
            })
            .map_err(|e| ExecutionError::WorkerPanic(e.to_string()))?;

        match rx.recv_timeout(self.config.exec_timeout) {
            Ok(result) => result.map_err(ExecutionError::from),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(ExecutionError::ExecutionTimeout(self.config.exec_timeout)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let join_err = handle.join();
                let panic_msg = match join_err {
                    Err(p) => {
                        if let Some(s) = p.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = p.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "Unknown worker panic".to_string()
                        }
                    }
                    Ok(_) => "Worker disconnected unexpectedly".to_string(),
                };
                Err(ExecutionError::WorkerPanic(panic_msg))
            }
        }
    }
}
