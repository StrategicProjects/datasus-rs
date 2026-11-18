# datasus-rs

[![CI](https://github.com/StrategicProjects/datasus-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/StrategicProjects/datasus-rs/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/datasuspy)](https://pypi.org/project/datasuspy/)
[![DOI](https://zenodo.org/badge/1462245859.svg)](https://zenodo.org/badge/latestdoi/1462245859)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/StrategicProjects/datasus-rs/blob/main/LICENSE)

Reader for DATASUS `.dbc` files (and plain `.dbf`) in Rust, shared by three
front ends:

| Crate | What it is |
|---|---|
| `crates/datasus-core` | The reader: PKWare DCL decompression + dBase parsing. No dependencies. |
| `crates/datasus-arrow` | Core → Apache Arrow record batches. |
| `crates/datasus-cli` | `datasus` command: convert to Parquet/CSV, inspect, decompress. |
| `crates/datasus-py` | `datasuspy` Python package (PyO3 + maturin). |

The core is a port of the C reader of the
[datasusr](https://github.com/StrategicProjects/datasusr) R package and is
tested to return the same values for the same options (see
[Validation](#validation)). Catalog, FTP download and cache are **not** part
of this project; they live in datasusr.

## CLI

```sh
cargo install --path crates/datasus-cli

datasus convert RDPE2401.dbc                       # -> RDPE2401.parquet (zstd)
datasus convert *.dbc --out-dir parquet/           # many files
datasus convert RDPE2401.dbc -o rd.csv --select uf_zi,val_tot --n-max 1000
datasus schema RDPE2401.dbc                        # fields and inferred types
datasus decompress RDPE2401.dbc                    # -> RDPE2401.dbf
```

`convert` options mirror `read_datasus_dbc()`: `--select`, `--n-max`,
`--no-trim`, `--encoding`, `--no-guess-types`, `--col-type NAME=TYPE`,
`--parse-dates`, `--keep-names`. Outputs are written to a `.part` file and
renamed on success; existing outputs need `--force`.

## Python

```sh
pip install datasuspy
```

From source: `cd crates/datasus-py && maturin develop --release` inside a
virtualenv.

```python
import datasuspy

table = datasuspy.read_dbc("RDPE2401.dbc", select=["uf_zi", "val_tot"])  # pyarrow.Table
df = table.to_pandas()

import polars as pl
df = pl.from_arrow(datasuspy.read_dbc("RDPE2401.dbc"))

# Streaming, without pyarrow: any Arrow PyCapsule consumer works
reader = datasuspy.open_dbc("RDPE2401.dbc", batch_size=100_000)
```

## Rust

```rust
use datasus_core::{open, ReadOptions};

let table = open("RDPE2401.dbc", &ReadOptions::default())?;
let columns = table.collect();           // Vec<ColumnData>
```

To fill your own buffers without an intermediate copy, implement
`datasus_core::Sink` and call `Table::read_rows`.

## Behaviour (same as datasusr 0.1.1)

- Records flagged as deleted (`*`) are dropped; `n_max` counts live records.
- Character fields are never null: a blank field is an empty string.
- Numeric, date and logical fields are always trimmed before parsing;
  `trim_ws` only affects character fields. A value ends at the first NUL.
- `N`/`F` fields with no decimals are integers unless a value does not fit
  a 32-bit integer (then the column is double). `i32::MIN` is not accepted
  as an integer (it is R's `NA`).
- `D` fields are text unless `parse_dates` is set; invalid dates are null.
- A header declaring more records than the file holds is a warning.
- Decompression stops at the record count declared in the header and never
  goes past 4 GiB (`decode_with_limit` changes the bound), so a crafted file
  cannot expand without limit.

Known differences from the C reader:

- Whitespace is always the six ASCII characters of the "C" locale. The C
  reader uses `isspace()`, which on macOS under a UTF-8 locale also trims
  `0xA0` (NBSP).
- Latin-1 text is decoded as strict ISO-8859-1. R on macOS and Windows
  converts `latin1` strings as Windows-1252, so bytes `0x80`–`0x9F` come out
  differently there.
- With `encoding = "utf-8"`, invalid sequences become U+FFFD.
- Hexadecimal floats and `nan(...)`, accepted by `strtod()`, are null.

## Validation

```sh
cargo test                                   # unit + ported datasusr tests
cargo build --release -p datasus-cli

# Oracle: compare against datasusr's C reader (needs R with datasusr + arrow)
Rscript tests/oracle/compare.R <path to datasus binary> \
  <datasusr repo>/tests/testthat/helper-dbf.R  [real .dbc files...]

cd crates/datasus-py && maturin develop --release && pytest tests
```

## License

MIT. `blast.rs` is a translation of Mark Adler's `blast.c` (zlib license).
