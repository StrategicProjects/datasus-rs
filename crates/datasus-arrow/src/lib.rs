//! Apache Arrow front end for [`datasus_core`]: reads a DBC/DBF table as a
//! stream of record batches.
//!
//! Type mapping: integer → `Int32`, double → `Float64`, logical →
//! `Boolean`, date → `Date32`, character → `Utf8` (decoded from the file
//! encoding; never null, blank fields are empty strings).

use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Date32Builder, Float64Builder, Int32Builder, StringBuilder,
};
use arrow_array::{ArrayRef, RecordBatch, RecordBatchOptions, RecordBatchReader};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use datasus_core::{repair_names, ColType, Cursor, Encoding, Sink, Table};

/// Default number of rows per record batch.
pub const DEFAULT_BATCH_SIZE: usize = 65_536;

/// Arrow schema of the selected columns.  Names are made unique and, with
/// `clean_names`, lowercased (as datasusr does).
pub fn schema(table: &Table, clean_names: bool) -> SchemaRef {
    let names = repair_names(table.columns().map(|f| f.name.as_str()), clean_names);
    let fields: Vec<Field> = table
        .columns()
        .zip(names)
        .map(|(f, name)| {
            let (dt, nullable) = match f.col_type {
                Some(ColType::Integer) => (DataType::Int32, true),
                Some(ColType::Double) => (DataType::Float64, true),
                Some(ColType::Logical) => (DataType::Boolean, true),
                Some(ColType::Date) => (DataType::Date32, true),
                _ => (DataType::Utf8, false),
            };
            Field::new(name, dt, nullable)
        })
        .collect();
    Arc::new(Schema::new(fields))
}

enum Builder {
    Int(Int32Builder),
    Double(Float64Builder),
    Logical(BooleanBuilder),
    Date(Date32Builder),
    Text(StringBuilder),
}

struct Builders {
    cols: Vec<Builder>,
    encoding: Encoding,
    scratch: String,
}

impl Sink for Builders {
    #[inline]
    fn int(&mut self, col: usize, _: usize, value: Option<i32>) {
        if let Builder::Int(b) = &mut self.cols[col] {
            b.append_option(value);
        }
    }
    #[inline]
    fn double(&mut self, col: usize, _: usize, value: Option<f64>) {
        if let Builder::Double(b) = &mut self.cols[col] {
            b.append_option(value);
        }
    }
    #[inline]
    fn logical(&mut self, col: usize, _: usize, value: Option<bool>) {
        if let Builder::Logical(b) = &mut self.cols[col] {
            b.append_option(value);
        }
    }
    #[inline]
    fn date(&mut self, col: usize, _: usize, value: Option<i32>) {
        if let Builder::Date(b) = &mut self.cols[col] {
            b.append_option(value);
        }
    }
    #[inline]
    fn text(&mut self, col: usize, _: usize, raw: &[u8]) {
        if let Builder::Text(b) = &mut self.cols[col] {
            self.scratch.clear();
            self.encoding.decode_into(raw, &mut self.scratch);
            b.append_value(&self.scratch);
        }
    }
}

/// Iterator of record batches over a [`Table`].
pub struct BatchReader {
    table: Table,
    schema: SchemaRef,
    cursor: Cursor,
    batch_size: usize,
    builders: Builders,
}

impl BatchReader {
    pub fn new(table: Table, clean_names: bool, batch_size: usize) -> BatchReader {
        let schema = schema(&table, clean_names);
        let cols = table
            .columns()
            .map(|f| match f.col_type {
                Some(ColType::Integer) => Builder::Int(Int32Builder::new()),
                Some(ColType::Double) => Builder::Double(Float64Builder::new()),
                Some(ColType::Logical) => Builder::Logical(BooleanBuilder::new()),
                Some(ColType::Date) => Builder::Date(Date32Builder::new()),
                _ => Builder::Text(StringBuilder::new()),
            })
            .collect();
        let builders = Builders { cols, encoding: table.encoding(), scratch: String::new() };
        BatchReader {
            table,
            schema,
            cursor: Cursor::default(),
            batch_size: batch_size.max(1),
            builders,
        }
    }

    /// The table being read (row count, warnings, unmatched `select` names).
    pub fn table(&self) -> &Table {
        &self.table
    }
}

impl Iterator for BatchReader {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        let n = self.table.read_rows(&mut self.cursor, self.batch_size, &mut self.builders);
        if n == 0 {
            return None;
        }
        let arrays: Vec<ArrayRef> = self
            .builders
            .cols
            .iter_mut()
            .map(|b| match b {
                Builder::Int(b) => Arc::new(b.finish()) as ArrayRef,
                Builder::Double(b) => Arc::new(b.finish()),
                Builder::Logical(b) => Arc::new(b.finish()),
                Builder::Date(b) => Arc::new(b.finish()),
                Builder::Text(b) => Arc::new(b.finish()),
            })
            .collect();
        // The row count is given explicitly so a selection that matched no
        // column still yields batches with the right number of rows.
        let options = RecordBatchOptions::new().with_row_count(Some(n));
        Some(RecordBatch::try_new_with_options(self.schema.clone(), arrays, &options))
    }
}

impl RecordBatchReader for BatchReader {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}
