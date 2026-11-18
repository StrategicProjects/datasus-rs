# datasuspy

[![CI](https://github.com/StrategicProjects/datasus-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/datasus-rs/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/datasuspy)](https://pypi.org/project/datasuspy/)
[![DOI](https://zenodo.org/badge/1462245859.svg)](https://zenodo.org/badge/latestdoi/1462245859)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/StrategicProjects/datasus-rs/blob/main/LICENSE)

Fast reader for DATASUS `.dbc` files (and plain `.dbf`), returning
[Apache Arrow](https://arrow.apache.org/) tables. The reader is written in
Rust and shares its core with the
[datasusr](https://github.com/StrategicProjects/datasusr) R package's reader,
against which it is tested value by value.

```sh
pip install datasuspy
```

```python
import datasuspy

table = datasuspy.read_dbc("RDPE2401.dbc")        # pyarrow.Table
df = table.to_pandas()

import polars as pl
df = pl.from_arrow(datasuspy.read_dbc("RDPE2401.dbc", select=["uf_zi", "val_tot"]))
```

`datasuspy` only reads files that are already on disk. It does not download
from the DATASUS FTP.

## Options

`read_dbc(path, **options)` and `open_dbc(path, **options)` take:

| Option | Default | Meaning |
|---|---|---|
| `select` | `None` | Columns to keep (case-insensitive). Unmatched names warn. |
| `n_max` | `None` | Maximum number of rows. |
| `trim_ws` | `True` | Trim whitespace from character fields. |
| `encoding` | `"latin1"` | `"latin1"` or `"utf-8"`. |
| `guess_types` | `True` | Scan numeric fields to tell integers from floats. |
| `col_types` | `None` | `{name: "character" \| "integer" \| "double" \| "logical" \| "date"}`. |
| `parse_dates` | `False` | Return DBF date fields as dates instead of text. |
| `clean_names` | `True` | Lowercase the column names. |
| `batch_size` | `65536` | Rows per Arrow record batch. |

`open_dbc()` returns a `DbcReader`: a one-shot Arrow stream implementing the
[PyCapsule interface](https://arrow.apache.org/docs/format/CDataInterface/PyCapsuleInterface.html)
(`__arrow_c_stream__`), so it can be consumed in batches by
`pyarrow.RecordBatchReader.from_stream`, polars, DuckDB and others.

```python
import pyarrow as pa
import pyarrow.parquet as pq

reader = pa.RecordBatchReader.from_stream(datasuspy.open_dbc("RDPE2401.dbc"))
with pq.ParquetWriter("rd.parquet", reader.schema) as writer:
    for batch in reader:
        writer.write_batch(batch)
```

## Behaviour

- Records flagged as deleted are dropped; `n_max` counts live records.
- Character columns are never null: a blank field is an empty string.
- Numeric fields with no decimals are `int32` unless a value does not fit;
  then the column is `float64`. Unparseable values are null.
- Date fields (`D`) are text unless `parse_dates=True`; invalid dates are null.
- Latin-1 is decoded as strict ISO-8859-1.

Source, issues and the command-line converter:
<https://github.com/StrategicProjects/datasus-rs>

MIT license. The decompressor is a translation of Mark Adler's `blast.c`
(zlib license).
