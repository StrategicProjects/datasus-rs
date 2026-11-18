//! Reader for DATASUS `.dbc` files (and plain `.dbf`).
//!
//! A DBC file is a dBase III header followed by the record area compressed
//! with the PKWare Data Compression Library.  This crate decompresses it
//! ([`blast`]), recognises the container layout ([`decode`]) and parses the
//! records into typed columns ([`Table`]).
//!
//! It is a port of the C reader of the [datasusr] R package and is meant to
//! produce the same values for the same options.  It has no dependencies so
//! it can be embedded anywhere (R packages, Python wheels, CLIs).
//!
//! ```no_run
//! use datasus_core::{open, ReadOptions};
//!
//! let table = open("RDPE2401.dbc", &ReadOptions::default())?;
//! println!("{} rows x {} columns", table.n_rows(), table.n_cols());
//! let columns = table.collect();
//! # Ok::<(), datasus_core::Error>(())
//! ```
//!
//! [datasusr]: https://github.com/StrategicProjects/datasusr

pub mod blast;
mod container;
mod dbf;

use std::fmt;
use std::path::Path;

pub use container::{decode, Container, DbfData};
pub use dbf::{
    repair_names, ColType, ColumnData, Cursor, Encoding, Field, ReadOptions, Sink, Table,
};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// PKWare DCL decompression failed; `code` is the `blast()` return code.
    Blast { code: i32, record_area: bool },
    /// Decompression worked but the result is not a DBF file.
    NotDbf,
    /// The DBF header is invalid.
    Header(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Blast { code, record_area: true } => {
                write!(f, "Failed to decompress DBC record area with blast(); code={code}")
            }
            Error::Blast { code, record_area: false } => {
                write!(f, "Failed to decompress DBC with blast(); code={code}")
            }
            Error::NotDbf => f.write_str(
                "Decompression succeeded, but output does not look like a valid DBF file.",
            ),
            Error::Header(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Read and decode a `.dbc`/`.dbf` file and plan how to read it.
pub fn open(path: impl AsRef<Path>, opts: &ReadOptions) -> Result<Table, Error> {
    from_bytes(std::fs::read(path)?, opts)
}

/// Same as [`open`], for a file already in memory.
pub fn from_bytes(bytes: Vec<u8>, opts: &ReadOptions) -> Result<Table, Error> {
    Table::new(decode(bytes)?, opts)
}
