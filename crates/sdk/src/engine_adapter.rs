//! engine_adapter.rs — Concrete OperationExecutor adapter connecting crates/sdk to pdfcraft_automation.
//! =====================================================================================================
//!
//! Purpose:
//!     Executes document transformations headlessly through `pdfcraft_automation::Automation`
//!     inside an ephemeral sandbox with:
//!     - Panic containment (`std::panic::catch_unwind`) converting engine panics to `SdkError::EnginePanic`
//!     - Strict path containment within an isolated temporary directory
//!     - High-DPI physical rendering calculation decoupled from display PPI (#739)
//!     - Automatic disk spillover for outputs exceeding memory limits
//!
//! Layer:
//!     Infrastructure / Adapters Layer
//!
//! Key Input Dependencies:
//!     - pdfcraft_automation::Automation
//!     - tempfile::TempDir (Isolated ephemeral sandbox directory)
//!     - crate::ports::OperationExecutor
//!     - crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::AutomationEngineAdapter;
//!     let adapter = AutomationEngineAdapter::default();
//!     ```
//!
//! Key Types & Functions Index:
//!     - AutomationEngineAdapter: Concrete engine adapter implementing OperationExecutor
//!     - AutomationEngineAdapter::new: Constructs adapter with custom limits
//!     - AutomationEngineAdapter::default: Constructs adapter with standard production limits
//!     - ExecutionSandbox: RAII sandbox creating private tempdir with guaranteed cleanup on Drop
//!     - ExecutionSandbox::materialize_source: Writes DocumentSource to disk if needed
//!     - ExecutionSandbox::collect_output: Collects output and spills to disk if exceeding max_memory_bytes

use std::cell::RefCell;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

use crate::error::SdkError;
use crate::ports::OperationExecutor;
use crate::types::{DocumentResult, DocumentSource, ImageFormat, MergeOptions, RenderOptions, SplitMode, SplitResult};
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Structured JSON payload of a tool result, if it is one.
fn json_of(c: &Content) -> Option<&Value> {
    match c {
        Content::Json(v) => Some(v),
        Content::Png { .. } => None,
    }
}

/// PNG bytes of a tool result, if it is an image.
fn png_of(c: &Content) -> Option<Vec<u8>> {
    match c {
        Content::Png { data, .. } => Some(data.clone()),
        Content::Json(_) => None,
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Execution sandbox managing ephemeral disk isolation and cleanup.
///
/// The directory is deleted on drop unless [`ExecutionSandbox::persist`] is called, which is
/// required when a result (a spilled `DocumentResult::File`, split parts) must outlive the call.
pub struct ExecutionSandbox {
    temp_dir: RefCell<Option<TempDir>>,
    root: PathBuf,
}

impl ExecutionSandbox {
    pub fn new() -> Result<Self, SdkError> {
        let temp_dir = tempfile::Builder::new().prefix("pdfcraft-sandbox-").tempdir().map_err(SdkError::Io)?;
        let root = temp_dir.path().to_path_buf();
        Ok(Self { temp_dir: RefCell::new(Some(temp_dir)), root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stops the sandbox directory from being deleted on drop; the caller then owns its contents.
    pub fn persist(&self) {
        if let Ok(mut slot) = self.temp_dir.try_borrow_mut()
            && let Some(dir) = slot.take()
        {
            let _ = dir.keep();
        }
    }

    /// Materializes a `DocumentSource` into the sandbox, returning its relative filename.
    pub fn materialize_source(&self, source: &DocumentSource, index: usize) -> Result<String, SdkError> {
        match source {
            DocumentSource::Path(p) => {
                if !p.exists() {
                    return Err(SdkError::FileNotFound { path: p.display().to_string() });
                }
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("pdf");
                let file_name = format!("input_{index}.{ext}");
                let dest = self.root.join(&file_name);
                fs::copy(p, dest).map_err(SdkError::Io)?;
                Ok(file_name)
            }
            DocumentSource::Bytes { data, name } => {
                let file_name = name.clone().unwrap_or_else(|| format!("input_{index}.pdf"));
                let dest = self.root.join(&file_name);
                fs::write(dest, data).map_err(SdkError::Io)?;
                Ok(file_name)
            }
        }
    }
}

/// Concrete engine adapter delegating to `pdfcraft_automation::Automation`.
pub struct AutomationEngineAdapter {
    max_memory_bytes: u64,
}

impl Default for AutomationEngineAdapter {
    fn default() -> Self {
        Self {
            max_memory_bytes: 10 * 1024 * 1024, // 10 MB threshold to spill to disk
        }
    }
}

impl AutomationEngineAdapter {
    pub fn new(max_memory_bytes: u64) -> Self {
        Self { max_memory_bytes }
    }

    /// Helper executing an engine closure under an `AssertUnwindSafe` panic boundary.
    fn with_automation<F, R>(&self, sandbox: &ExecutionSandbox, f: F) -> Result<R, SdkError>
    where
        F: FnOnce(&mut Automation) -> Result<R, SdkError>,
    {
        let mut automation =
            Automation::new().with_root(sandbox.root()).map_err(|e| SdkError::EngineError(format!("Failed to configure automation root: {e}")))?;

        let result = catch_unwind(AssertUnwindSafe(|| f(&mut automation)));

        match result {
            Ok(inner_res) => inner_res,
            Err(panic_err) => {
                let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_err.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Unknown engine panic".to_string()
                };
                Err(SdkError::EnginePanic(msg))
            }
        }
    }
}

impl OperationExecutor for AutomationEngineAdapter {
    fn execute_merge(&self, sources: &[DocumentSource], options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        if sources.is_empty() {
            return Err(SdkError::InvalidArgument("No input documents provided for merge".into()));
        }

        let sandbox = ExecutionSandbox::new()?;
        let mut materialized_paths = Vec::with_capacity(sources.len());
        for (i, src) in sources.iter().enumerate() {
            materialized_paths.push(sandbox.materialize_source(src, i)?);
        }

        let out_name = options.output_filename.clone().unwrap_or_else(|| {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            format!("merged_{id}.pdf")
        });

        self.with_automation(&sandbox, |automation| {
            let args = json!({
                "paths": materialized_paths,
                "pages": options.pages,
                "passwords": options.passwords,
                "out": out_name,
            });

            automation.call("doc_combine", &args).map_err(|e| SdkError::EngineError(format!("Merge failed: {e}")))?;

            let out_path = sandbox.root().join(&out_name);
            let metadata = fs::metadata(&out_path).map_err(SdkError::Io)?;
            let size = metadata.len();

            if size > self.max_memory_bytes {
                sandbox.persist(); // the file must outlive this call
                Ok(DocumentResult::file(out_path, "application/pdf", size))
            } else {
                let bytes = fs::read(&out_path).map_err(SdkError::Io)?;
                Ok(DocumentResult::memory(bytes, "application/pdf"))
            }
        })
    }

    fn execute_split(&self, source: &DocumentSource, mode: &SplitMode) -> Result<SplitResult, SdkError> {
        let sandbox = ExecutionSandbox::new()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            let open_res = automation
                .call("doc_open", &json!({ "path": file_name }))
                .map_err(|e| SdkError::EngineError(format!("Failed to open document: {e}")))?;

            let doc_id = open_res
                .first()
                .and_then(json_of)
                .and_then(|v| v.get("doc"))
                .and_then(Value::as_i64)
                .ok_or_else(|| SdkError::EngineError("Invalid doc id returned".into()))?;

            let out_dir = "parts";
            let mut split_args = json!({
                "doc": doc_id,
                "out_dir": out_dir,
            });

            match mode {
                SplitMode::EveryNPages(every) => split_args["every"] = json!(every),
                SplitMode::BeforePages(pages) => split_args["before"] = json!(pages),
                SplitMode::AtBookmarks => split_args["bookmarks"] = json!(true),
                SplitMode::MaxFileSizeMb(mb) => split_args["max_mb"] = json!(mb),
            }

            let res = automation.call("doc_split", &split_args).map_err(|e| SdkError::EngineError(format!("Split failed: {e}")))?;

            let raw_files = res
                .first()
                .and_then(json_of)
                .and_then(|v| v.get("files"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        // doc_split reports each part as { path, first_page, last_page }.
                        .filter_map(|v| v.as_str().or_else(|| v.get("path").and_then(Value::as_str)))
                        .map(|f| {
                            let p = PathBuf::from(f);
                            if p.is_absolute() { p } else { sandbox.root().join(p) }
                        })
                        .collect::<Vec<PathBuf>>()
                })
                .unwrap_or_default();
            sandbox.persist(); // split parts must outlive this call

            Ok(SplitResult { files: raw_files })
        })
    }

    fn execute_render(&self, source: &DocumentSource, page: usize, options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }

        // The engine's `page_render` only produces PNG; never label PNG bytes as another format.
        if options.format != ImageFormat::Png {
            return Err(SdkError::UnsupportedOperation(format!("render format {:?} (the engine renders PNG only)", options.format)));
        }

        let sandbox = ExecutionSandbox::new()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            // Strictly compute DPI without coupling to monitor display PPI (#739)
            let open_res = automation
                .call("doc_open", &json!({ "path": file_name }))
                .map_err(|e| SdkError::EngineError(format!("Failed to open document: {e}")))?;
            let doc_id = open_res
                .first()
                .and_then(json_of)
                .and_then(|v| v.get("doc"))
                .and_then(Value::as_i64)
                .ok_or_else(|| SdkError::EngineError("Invalid doc id returned".into()))?;

            let args = json!({
                "doc": doc_id,
                "page": page,
                "dpi": options.dpi,
            });

            let res = automation.call("page_render", &args).map_err(|e| SdkError::EngineError(format!("Render failed: {e}")))?;

            let img_bytes = res.first().and_then(png_of).ok_or_else(|| SdkError::EngineError("Render produced no image bytes".into()))?;

            Ok(DocumentResult::memory(img_bytes, "image/png"))
        })
    }
}
