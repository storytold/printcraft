//! storage.rs — Durable Job Repository with atomic state transitions and terminal immutability.
//! ==============================================================================================
//!
//! Purpose:
//!     Enforces atomic compare-and-swap state transitions and guarantees immutability
//!     of terminal job states (Succeeded, Failed, Canceled) across both in-memory and durable backends.
//!
//! Layer:
//!     REST Service / Storage & State Management Layer
//!
//! Key Input Dependencies:
//!     - crate::models::{JobId, JobRecord, JobState}
//!     - pdfcraft_sdk::{DocumentResult, SplitResult}
//!     - thiserror::Error
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::storage::{InMemoryJobRepository, JobRepository};
//!     use pdfcraft_rest::models::{JobId, JobState};
//!     let repo = InMemoryJobRepository::new();
//!     repo.transition(&JobId::new("job-1"), JobState::Queued, JobState::Running, None, None, None);
//!     ```
//!
//! Key Types & Functions Index:
//!     - JobError: Storage errors (NotFound, TerminalStateConflict, StateConflict, StorageFailure)
//!     - JobRepository: Repository trait (create, get, transition, delete, list)
//!     - InMemoryJobRepository: Lock-protected in-memory implementation
//!     - DurableFileJobRepository: Crash-safe append/atomic JSON disk journal with recovery on startup

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

use crate::models::{JobId, JobRecord, JobState};
use pdfcraft_sdk::{DocumentResult, SplitResult};

/// Structured storage and transition errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum JobError {
    #[error("Job not found: {0}")]
    NotFound(JobId),

    #[error("Cannot transition terminal job {id} in state {current:?} to {attempted:?}")]
    TerminalStateConflict { id: JobId, current: JobState, attempted: JobState },

    #[error("State conflict for job {id}: expected state {expected:?}, but current state is {actual:?}")]
    StateConflict { id: JobId, expected: JobState, actual: JobState },

    #[error("Storage I/O failure: {0}")]
    StorageFailure(String),
}

/// Abstract repository port for durable job persistence.
pub trait JobRepository: Send + Sync {
    /// Inserts a new job record.
    fn create(&self, job: JobRecord) -> Result<(), JobError>;

    /// Fetches a job by ID.
    fn get(&self, id: &JobId) -> Result<Option<JobRecord>, JobError>;

    /// Performs an atomic compare-and-swap transition from `expected` to `next`.
    fn transition(
        &self,
        id: &JobId,
        expected: JobState,
        next: JobState,
        error: Option<String>,
        result: Option<DocumentResult>,
        split_result: Option<SplitResult>,
    ) -> Result<(), JobError>;

    /// Deletes a job record. Returns true if removed, false if not found.
    fn delete(&self, id: &JobId) -> Result<bool, JobError>;

    /// Lists all current jobs.
    fn list(&self) -> Result<Vec<JobRecord>, JobError>;
}

/// In-Memory implementation of `JobRepository` utilizing atomic read-write locking.
#[derive(Debug, Default)]
pub struct InMemoryJobRepository {
    jobs: RwLock<HashMap<JobId, JobRecord>>,
}

impl InMemoryJobRepository {
    pub fn new() -> Self {
        Self { jobs: RwLock::new(HashMap::new()) }
    }
}

impl JobRepository for InMemoryJobRepository {
    fn create(&self, job: JobRecord) -> Result<(), JobError> {
        let mut map = self.jobs.write().map_err(|e| JobError::StorageFailure(e.to_string()))?;
        map.insert(job.id.clone(), job);
        Ok(())
    }

    fn get(&self, id: &JobId) -> Result<Option<JobRecord>, JobError> {
        let map = self.jobs.read().map_err(|e| JobError::StorageFailure(e.to_string()))?;
        Ok(map.get(id).cloned())
    }

    fn transition(
        &self,
        id: &JobId,
        expected: JobState,
        next: JobState,
        error: Option<String>,
        result: Option<DocumentResult>,
        split_result: Option<SplitResult>,
    ) -> Result<(), JobError> {
        let mut map = self.jobs.write().map_err(|e| JobError::StorageFailure(e.to_string()))?;
        let record = map.get_mut(id).ok_or_else(|| JobError::NotFound(id.clone()))?;

        // 1. Terminal state immutability invariant
        if record.state.is_terminal() {
            return Err(JobError::TerminalStateConflict { id: id.clone(), current: record.state, attempted: next });
        }

        // 2. Atomic CAS state check
        if record.state != expected {
            return Err(JobError::StateConflict { id: id.clone(), expected, actual: record.state });
        }

        // 3. Mutate record
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

        record.state = next;
        record.updated_at = now;
        if next == JobState::Succeeded {
            record.progress_pct = 100;
        } else if next == JobState::Running {
            record.progress_pct = 10;
        }

        if let Some(err) = error {
            record.error = Some(err);
        }
        if let Some(res) = result {
            record.result = Some(res);
        }
        if let Some(split) = split_result {
            record.split_result = Some(split);
        }

        Ok(())
    }

    fn delete(&self, id: &JobId) -> Result<bool, JobError> {
        let mut map = self.jobs.write().map_err(|e| JobError::StorageFailure(e.to_string()))?;
        Ok(map.remove(id).is_some())
    }

    fn list(&self) -> Result<Vec<JobRecord>, JobError> {
        let map = self.jobs.read().map_err(|e| JobError::StorageFailure(e.to_string()))?;
        Ok(map.values().cloned().collect())
    }
}

/// Durable file-journaled implementation of `JobRepository`.
/// Persists records to individual atomic JSON files under a directory.
#[derive(Debug)]
pub struct DurableFileJobRepository {
    dir: PathBuf,
    in_memory: InMemoryJobRepository,
}

impl DurableFileJobRepository {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, JobError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| JobError::StorageFailure(e.to_string()))?;

        let in_memory = InMemoryJobRepository::new();

        // Load existing records on startup
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json")
                    && let Ok(content) = fs::read_to_string(&path)
                    && let Ok(record) = serde_json::from_str::<JobRecord>(&content)
                {
                    let _ = in_memory.create(record);
                }
            }
        }

        Ok(Self { dir, in_memory })
    }

    fn persist_record(&self, record: &JobRecord) -> Result<(), JobError> {
        let target_path = self.dir.join(format!("{}.json", record.id.0));
        let temp_path = self.dir.join(format!("{}.tmp", record.id.0));

        let json_data = serde_json::to_string_pretty(record).map_err(|e| JobError::StorageFailure(e.to_string()))?;

        // Atomic write via temporary file
        let mut file = File::create(&temp_path).map_err(|e| JobError::StorageFailure(e.to_string()))?;
        file.write_all(json_data.as_bytes()).map_err(|e| JobError::StorageFailure(e.to_string()))?;
        file.sync_all().map_err(|e| JobError::StorageFailure(e.to_string()))?;

        fs::rename(&temp_path, &target_path).map_err(|e| JobError::StorageFailure(e.to_string()))?;

        Ok(())
    }
}

impl JobRepository for DurableFileJobRepository {
    fn create(&self, job: JobRecord) -> Result<(), JobError> {
        self.persist_record(&job)?;
        self.in_memory.create(job)
    }

    fn get(&self, id: &JobId) -> Result<Option<JobRecord>, JobError> {
        self.in_memory.get(id)
    }

    fn transition(
        &self,
        id: &JobId,
        expected: JobState,
        next: JobState,
        error: Option<String>,
        result: Option<DocumentResult>,
        split_result: Option<SplitResult>,
    ) -> Result<(), JobError> {
        self.in_memory.transition(id, expected, next, error, result, split_result)?;
        if let Some(record) = self.in_memory.get(id)? {
            self.persist_record(&record)?;
        }
        Ok(())
    }

    fn delete(&self, id: &JobId) -> Result<bool, JobError> {
        let removed = self.in_memory.delete(id)?;
        if removed {
            let file_path = self.dir.join(format!("{}.json", id.0));
            let _ = fs::remove_file(file_path);
        }
        Ok(removed)
    }

    fn list(&self) -> Result<Vec<JobRecord>, JobError> {
        self.in_memory.list()
    }
}
