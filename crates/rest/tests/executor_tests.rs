//! executor_tests.rs — Unit and concurrency tests for BlockingExecutor.
//! =========================================================================
//!
//! Purpose:
//!     Verifies bounded worker concurrency, queue wait timeouts under load,
//!     and execution deadline enforcement.
//!
//! Layer:
//!     Test Suite / REST Executor Tests

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pdfcraft_rest::executor::{BlockingExecutor, ExecutionError, ExecutorConfig};
use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, OperationExecutor, RenderOptions, SdkError, SplitMode, SplitResult};

/// Mock executor simulating long-running compute jobs.
struct SlowMockExecutor {
    delay: Duration,
    call_count: AtomicUsize,
}

impl OperationExecutor for SlowMockExecutor {
    fn execute_merge(&self, _sources: &[DocumentSource], _options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        Ok(DocumentResult::memory(b"%PDF-1.7 mock output".to_vec(), "application/pdf"))
    }

    fn execute_split(&self, _source: &DocumentSource, _mode: &SplitMode) -> Result<SplitResult, SdkError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        Ok(SplitResult { files: vec![] })
    }

    fn execute_render(&self, _source: &DocumentSource, _page: usize, _options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        Ok(DocumentResult::memory(b"mock-png".to_vec(), "image/png"))
    }
}

#[test]
fn test_bounded_executor_capacity_and_queue_timeout() {
    let mock = Arc::new(SlowMockExecutor { delay: Duration::from_millis(300), call_count: AtomicUsize::new(0) });

    // Configure pool with capacity = 1 and short queue timeout of 50ms
    let config = ExecutorConfig { max_concurrency: 1, queue_timeout: Duration::from_millis(50), exec_timeout: Duration::from_secs(5) };
    let executor = Arc::new(BlockingExecutor::new(mock, config));

    assert_eq!(executor.available_capacity(), 1);

    // Spawn 1st worker to occupy capacity for 300ms
    let ex1 = Arc::clone(&executor);
    let handle1 = std::thread::spawn(move || {
        let dummy = DocumentSource::from_bytes(b"%PDF-dummy".to_vec(), None);
        ex1.execute_merge(&[dummy], &MergeOptions::default())
    });

    // Small delay to ensure worker 1 acquires the permit
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(executor.available_capacity(), 0);

    // 2nd worker should time out waiting in queue after 50ms
    let t0 = Instant::now();
    let dummy = DocumentSource::from_bytes(b"%PDF-dummy".to_vec(), None);
    let result2 = executor.execute_merge(&[dummy], &MergeOptions::default());
    let elapsed = t0.elapsed();

    assert!(elapsed >= Duration::from_millis(45), "Did not wait for timeout: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(200), "Waited too long: {elapsed:?}");

    match result2 {
        Err(ExecutionError::QueueTimeout { limit }) => {
            assert_eq!(limit, 1);
        }
        other => panic!("Expected QueueTimeout, got {other:?}"),
    }

    // 1st worker completes successfully
    let res1 = handle1.join().unwrap();
    assert!(res1.is_ok());

    // Capacity is restored
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(executor.available_capacity(), 1);
}

#[test]
fn test_executor_execution_timeout() {
    let mock = Arc::new(SlowMockExecutor { delay: Duration::from_millis(500), call_count: AtomicUsize::new(0) });

    // Configure pool with capacity = 2, queue timeout = 1s, exec timeout = 50ms
    let config = ExecutorConfig { max_concurrency: 2, queue_timeout: Duration::from_secs(1), exec_timeout: Duration::from_millis(50) };
    let executor = BlockingExecutor::new(mock, config);

    let dummy = DocumentSource::from_bytes(b"%PDF-dummy".to_vec(), None);
    let res = executor.execute_merge(&[dummy], &MergeOptions::default());

    match res {
        Err(ExecutionError::ExecutionTimeout(d)) => {
            assert_eq!(d, Duration::from_millis(50));
        }
        other => panic!("Expected ExecutionTimeout, got {other:?}"),
    }
}
