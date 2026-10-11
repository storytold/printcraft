//! models.rs — Models and DTOs for the REST Document Processing Service (Issue #872).
//! ====================================================================================
//!
//! Purpose:
//!     Provides strongly typed request/response transfer objects and durable job records
//!     for the on-premise REST processing daemon.
//!
//! Layer:
//!     REST Service / Data Transfer Layer
//!
//! Key Input Dependencies:
//!     - pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode, SplitResult}
//!     - serde::{Deserialize, Serialize}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::models::{JobId, JobRecord, JobState};
//!     let job_id = JobId::generate();
//!     let record = JobRecord::new_queued(job_id, "merge", None);
//!     ```
//!
//! Key Types & Functions Index:
//!     - JobId: Unique identifier for background jobs (generate, new, as_str)
//!     - JobState: Lifecycle state enum (Queued, Running, Succeeded, Failed, Canceled, is_terminal)
//!     - JobRecord: Durable persistent job representation with progress and result
//!     - JobResponse: External JSON view of a job for API responses
//!     - MergePayload: Inbound payload for merging documents
//!     - SplitPayload: Inbound payload for splitting documents
//!     - RenderPayload: Inbound payload for rendering page previews
//!     - SourcePayload: Inbound document source (Path or base64 Bytes)

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode, SplitResult};

static JOB_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Unique identifier for an asynchronous or durable processing job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub String);

impl JobId {
    /// Generates a unique, monotonically increasing job ID.
    pub fn generate() -> Self {
        let count = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Self(format!("job-{now:x}-{count:x}"))
    }

    /// Creates a JobId from a string slice.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle states of a background processing job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Job is admitted and awaiting worker capacity.
    Queued,
    /// Job is actively being processed by a worker thread.
    Running,
    /// Job completed successfully with an available output artifact.
    Succeeded,
    /// Job failed with an unrecoverable error.
    Failed,
    /// Job was canceled before completion.
    Canceled,
}

impl JobState {
    /// Returns true if this state is terminal (immutable).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

/// Persistent record representing a document job in storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub tenant_id: Option<String>,
    pub operation: String,
    pub state: JobState,
    pub created_at: u64,
    pub updated_at: u64,
    pub progress_pct: u8,
    pub error: Option<String>,
    pub result: Option<DocumentResult>,
    pub split_result: Option<SplitResult>,
}

impl JobRecord {
    /// Creates a new job record initialized in the Queued state.
    pub fn new_queued(id: JobId, operation: impl Into<String>, tenant_id: Option<String>) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

        Self {
            id,
            tenant_id,
            operation: operation.into(),
            state: JobState::Queued,
            created_at: now,
            updated_at: now,
            progress_pct: 0,
            error: None,
            result: None,
            split_result: None,
        }
    }
}

/// External API representation of a processing job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: String,
    pub operation: String,
    pub status: JobState,
    pub progress: u8,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_url: Option<String>,
}

impl From<&JobRecord> for JobResponse {
    fn from(r: &JobRecord) -> Self {
        let result_url = if r.state == JobState::Succeeded { Some(format!("/v1/jobs/{}/result", r.id.0)) } else { None };

        Self {
            id: r.id.0.clone(),
            operation: r.operation.clone(),
            status: r.state,
            progress: r.progress_pct,
            created_at: r.created_at,
            updated_at: r.updated_at,
            error: r.error.clone(),
            result_url,
        }
    }
}

/// Payload for merging multiple PDF documents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergePayload {
    pub files: Vec<SourcePayload>,
    #[serde(default)]
    pub options: MergeOptions,
    #[serde(default)]
    pub async_job: bool,
}

/// Payload for splitting a PDF document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitPayload {
    pub file: SourcePayload,
    pub mode: SplitMode,
    #[serde(default)]
    pub async_job: bool,
}

/// Payload for previewing / rendering a PDF page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderPayload {
    pub file: SourcePayload,
    pub page: usize,
    #[serde(default)]
    pub options: RenderOptions,
    #[serde(default)]
    pub async_job: bool,
}

/// Serializable representation of an input document source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SourcePayload {
    Path { path: PathBuf },
    Bytes { data_base64: String, name: Option<String> },
}

impl SourcePayload {
    /// Converts this payload into an engine `DocumentSource`.
    pub fn into_source(self) -> Result<DocumentSource, String> {
        match self {
            Self::Path { path } => Ok(DocumentSource::Path(path)),
            Self::Bytes { data_base64, name } => {
                // Decode base64 bytes
                let data = decode_base64(&data_base64)?;
                Ok(DocumentSource::Bytes { data, name })
            }
        }
    }
}

/// Simple RFC 4648 standard base64 decoder without unsafe code.
fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = clean.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("Invalid base64 payload length".into());
    }

    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = match bytes.get(i).map(|&b| decode_char(b)) {
            Some(Some(val)) => val,
            _ => return Err("Invalid base64 character".into()),
        };
        let b1 = match bytes.get(i + 1).map(|&b| decode_char(b)) {
            Some(Some(val)) => val,
            _ => return Err("Invalid base64 character".into()),
        };
        let (b2, pad2) = match bytes.get(i + 2) {
            Some(b'=') => (0u8, true),
            Some(&b) => match decode_char(b) {
                Some(val) => (val, false),
                None => return Err("Invalid base64 character".into()),
            },
            None => return Err("Truncated base64".into()),
        };
        let (b3, pad3) = match bytes.get(i + 3) {
            Some(b'=') => (0u8, true),
            Some(&b) => match decode_char(b) {
                Some(val) => (val, false),
                None => return Err("Invalid base64 character".into()),
            },
            None => return Err("Truncated base64".into()),
        };

        let chunk = ((b0 as u32) << 18) | ((b1 as u32) << 12) | ((b2 as u32) << 6) | (b3 as u32);
        out.push(((chunk >> 16) & 0xFF) as u8);
        if !pad2 {
            out.push(((chunk >> 8) & 0xFF) as u8);
        }
        if !pad3 && !pad2 {
            out.push((chunk & 0xFF) as u8);
        }
        i += 4;
    }
    Ok(out)
}

fn decode_char(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
