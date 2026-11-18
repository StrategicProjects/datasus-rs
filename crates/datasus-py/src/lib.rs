//! Python bindings: `datasuspy._core`.
//!
//! The reader is exported through the Arrow PyCapsule interface
//! (`__arrow_c_stream__`), so no Arrow Python library is needed at build
//! time and any Arrow consumer (pyarrow, polars, DuckDB, ...) can read it.

use std::path::PathBuf;

use arrow_array::ffi_stream::FFI_ArrowArrayStream;
use datasus_arrow::BatchReader;
use datasus_core::{ColType, Encoding, Error, ReadOptions};
use pyo3::exceptions::{PyOSError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;

/// A `.dbc`/`.dbf` file opened for reading; an Arrow stream that can be
/// consumed once.
#[pyclass(module = "datasuspy")]
struct DbcReader {
    reader: Option<BatchReader>,
    n_rows: usize,
    columns: Vec<(String, String)>,
    warnings: Vec<String>,
    unmatched_select: Vec<String>,
}

#[pymethods]
impl DbcReader {
    /// Number of rows the stream will produce.
    #[getter]
    fn n_rows(&self) -> usize {
        self.n_rows
    }

    /// Number of selected columns.
    #[getter]
    fn n_cols(&self) -> usize {
        self.columns.len()
    }

    /// `(name, type)` of the selected columns, with the original DBF names.
    #[getter]
    fn columns(&self) -> Vec<(String, String)> {
        self.columns.clone()
    }

    /// Non-fatal problems found in the file.
    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    /// Names in `select` that matched no column.
    #[getter]
    fn unmatched_select(&self) -> Vec<String> {
        self.unmatched_select.clone()
    }

    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &mut self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        // Schema negotiation is optional in the protocol; the native schema
        // is always returned.
        let _ = requested_schema;
        let reader = self
            .reader
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("this DbcReader was already consumed"))?;
        let stream = FFI_ArrowArrayStream::new(Box::new(reader));
        PyCapsule::new_with_value(py, stream, c"arrow_array_stream")
    }

    fn __repr__(&self) -> String {
        format!(
            "<datasuspy.DbcReader {} rows x {} columns{}>",
            self.n_rows,
            self.columns.len(),
            if self.reader.is_none() { ", consumed" } else { "" }
        )
    }
}

#[pyfunction]
#[pyo3(signature = (path, *, select=None, n_max=None, trim_ws=true, encoding="latin1",
                    guess_types=true, col_types=None, parse_dates=false, clean_names=true,
                    batch_size=65_536))]
#[allow(clippy::too_many_arguments)]
fn open_dbc(
    py: Python<'_>,
    path: PathBuf,
    select: Option<Vec<String>>,
    n_max: Option<u64>,
    trim_ws: bool,
    encoding: &str,
    guess_types: bool,
    col_types: Option<Vec<(String, String)>>,
    parse_dates: bool,
    clean_names: bool,
    batch_size: usize,
) -> PyResult<DbcReader> {
    let encoding = Encoding::from_name(encoding).ok_or_else(|| {
        PyValueError::new_err(format!(
            "unsupported encoding {encoding:?}: use 'latin1' or 'utf-8'"
        ))
    })?;
    let col_types = col_types
        .unwrap_or_default()
        .into_iter()
        .map(|(name, ty)| match ColType::from_name(&ty) {
            Some(t) => Ok((name, t)),
            None => Err(PyValueError::new_err(format!(
                "invalid type {ty:?} for column {name:?}: use 'character', 'integer', \
                 'double', 'logical' or 'date'"
            ))),
        })
        .collect::<PyResult<Vec<_>>>()?;
    if batch_size == 0 {
        return Err(PyValueError::new_err("batch_size must be at least 1"));
    }

    let opts = ReadOptions { select, n_max, trim_ws, encoding, guess_types, col_types, parse_dates };

    // Reading and decompressing the file needs no Python state.
    let table = py.detach(|| datasus_core::open(&path, &opts)).map_err(|e| match e {
        Error::Io(e) => PyOSError::new_err(format!("{}: {e}", path.display())),
        other => PyValueError::new_err(format!("{}: {other}", path.display())),
    })?;

    Ok(DbcReader {
        n_rows: table.n_rows(),
        columns: table
            .columns()
            .map(|f| (f.name.clone(), f.col_type.map_or("character", ColType::name).to_string()))
            .collect(),
        warnings: table.warnings().to_vec(),
        unmatched_select: table.unmatched_select().to_vec(),
        reader: Some(BatchReader::new(table, clean_names, batch_size)),
    })
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<DbcReader>()?;
    m.add_function(wrap_pyfunction!(open_dbc, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
