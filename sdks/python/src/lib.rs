//! PyO3 Native Module Binding for `pdfcraft-sdk` (Issue #872).
//!
//! # Architecture Reference
//! Module: sdks/python/src/lib.rs
//! Purpose:
//!   Provides high-performance, in-process C-ABI Python bindings directly linking
//!   to `pdfcraft-sdk` for serverless or local execution without HTTP overhead.

use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use pdfcraft_sdk::{
    DocumentResult, DocumentSource, LocalClient, MergeOptions, PdfCraftClient, RenderOptions, SdkError, SplitMode,
};

fn map_sdk_err(err: SdkError) -> PyErr {
    match err {
        SdkError::InvalidArgument(msg) => PyValueError::new_err(msg),
        SdkError::FileNotFound { path } => PyIOError::new_err(format!("File not found: {path}")),
        SdkError::Io(e) => PyIOError::new_err(e.to_string()),
        other => PyValueError::new_err(other.to_string()),
    }
}

/// In-process native engine driver exposed to Python.
#[pyclass]
struct NativeLocalClient {
    client: LocalClient,
}

#[pymethods]
impl NativeLocalClient {
    #[new]
    fn new() -> Self {
        Self {
            client: LocalClient::default(),
        }
    }

    /// Merges a list of file paths or raw byte buffers.
    #[pyo3(signature = (sources, output_filename=None))]
    fn merge<'py>(
        &self,
        py: Python<'py>,
        sources: Vec<Bound<'py, PyAny>>,
        output_filename: Option<String>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let mut rust_sources = Vec::with_capacity(sources.len());

        for item in sources {
            if let Ok(bytes) = item.downcast::<PyBytes>() {
                rust_sources.push(DocumentSource::from_bytes(bytes.as_bytes().to_vec(), None));
            } else if let Ok(s) = item.extract::<String>() {
                rust_sources.push(DocumentSource::from_path(s));
            } else {
                return Err(PyValueError::new_err(
                    "Each source must be either a file path string or bytes buffer",
                ));
            }
        }

        let options = MergeOptions {
            output_filename,
            ..Default::default()
        };

        let result = self.client.merge(&rust_sources, options).map_err(map_sdk_err)?;

        let dict = PyDict::new(py);
        match result {
            DocumentResult::Memory { data, mime_type } => {
                dict.set_item("type", "memory")?;
                dict.set_item("data", PyBytes::new(py, &data))?;
                dict.set_item("mime_type", mime_type)?;
            }
            DocumentResult::File { path, mime_type, size_bytes } => {
                dict.set_item("type", "file")?;
                dict.set_item("path", path.to_string_lossy().to_string())?;
                dict.set_item("mime_type", mime_type)?;
                dict.set_item("size_bytes", size_bytes)?;
            }
        }

        Ok(dict)
    }

    /// Splits a PDF document into multiple files.
    #[pyo3(signature = (source, every_n_pages=None))]
    fn split(&self, source: Bound<'_, PyAny>, every_n_pages: Option<usize>) -> PyResult<Vec<String>> {
        let rust_source = if let Ok(bytes) = source.downcast::<PyBytes>() {
            DocumentSource::from_bytes(bytes.as_bytes().to_vec(), None)
        } else if let Ok(s) = source.extract::<String>() {
            DocumentSource::from_path(s)
        } else {
            return Err(PyValueError::new_err("Source must be a file path string or bytes buffer"));
        };

        let mode = SplitMode::EveryNPages(every_n_pages.unwrap_or(1));
        let res = self.client.split(&rust_source, mode).map_err(map_sdk_err)?;

        Ok(res.files.into_iter().map(|p| p.to_string_lossy().to_string()).collect())
    }

    /// Renders a single page to an image.
    #[pyo3(signature = (source, page, dpi=150.0))]
    fn render_page<'py>(
        &self,
        py: Python<'py>,
        source: Bound<'py, PyAny>,
        page: usize,
        dpi: f32,
    ) -> PyResult<Bound<'py, PyDict>> {
        let rust_source = if let Ok(bytes) = source.downcast::<PyBytes>() {
            DocumentSource::from_bytes(bytes.as_bytes().to_vec(), None)
        } else if let Ok(s) = source.extract::<String>() {
            DocumentSource::from_path(s)
        } else {
            return Err(PyValueError::new_err("Source must be a file path string or bytes buffer"));
        };

        let options = RenderOptions {
            dpi,
            ..Default::default()
        };

        let result = self.client.render_page(&rust_source, page, options).map_err(map_sdk_err)?;

        let dict = PyDict::new(py);
        match result {
            DocumentResult::Memory { data, mime_type } => {
                dict.set_item("type", "memory")?;
                dict.set_item("data", PyBytes::new(py, &data))?;
                dict.set_item("mime_type", mime_type)?;
            }
            DocumentResult::File { path, mime_type, size_bytes } => {
                dict.set_item("type", "file")?;
                dict.set_item("path", path.to_string_lossy().to_string())?;
                dict.set_item("mime_type", mime_type)?;
                dict.set_item("size_bytes", size_bytes)?;
            }
        }

        Ok(dict)
    }
}

/// PyO3 Python Module definition.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<NativeLocalClient>()?;
    Ok(())
}
