"""Offline tests with synthetic dBase III files (same cases as datasusr)."""

import datetime as dt
import struct

import pyarrow as pa
import pytest

import datasuspy


def make_dbf(path, fields, records, n_records=None):
    """fields: (name, type, len, dec); records: bytes of 1 + sum(len) each."""
    header_len = 32 + 32 * len(fields) + 1
    record_len = 1 + sum(f[2] for f in fields)
    n = len(records) if n_records is None else n_records
    buf = bytes([0x03, 24, 1, 1]) + struct.pack("<IHH", n, header_len, record_len) + bytes(20)
    for name, typ, length, dec in fields:
        buf += name.encode().ljust(11, b"\0") + typ.encode() + bytes(4)
        buf += bytes([length, dec]) + bytes(14)
    buf += b"\x0d" + b"".join(records) + b"\x1a"
    path.write_bytes(buf)
    return path


FIELDS = [
    ("NAME", "C", 5, 0),
    ("CNT", "N", 4, 0),
    ("VAL", "N", 6, 2),
    ("RATE", "F", 6, 0),
    ("DT", "D", 8, 0),
    ("OK", "L", 1, 0),
]
RECORDS = [
    b" ana    12  3.50   1.520240115T",
    b"*del    99  9.99     920200101T",
    b" bruno  -7 10.25     220231231F",
    b" caf\xe9                         ?",
]


@pytest.fixture
def dbf(tmp_path):
    return make_dbf(tmp_path / "basic.dbf", FIELDS, RECORDS)


def test_read_dbc_types_and_values(dbf):
    t = datasuspy.read_dbc(dbf)
    assert isinstance(t, pa.Table)
    assert t.column_names == ["name", "cnt", "val", "rate", "dt", "ok"]
    assert t.schema.types == [
        pa.string(), pa.int32(), pa.float64(), pa.float64(), pa.string(), pa.bool_(),
    ]
    assert t.to_pydict() == {
        "name": ["ana", "bruno", "café"],
        "cnt": [12, -7, None],
        "val": [3.5, 10.25, None],
        "rate": [1.5, 2.0, None],
        "dt": ["20240115", "20231231", ""],
        "ok": [True, False, None],
    }


def test_options(dbf):
    t = datasuspy.read_dbc(dbf, parse_dates=True, clean_names=False)
    assert t.column_names[0] == "NAME"
    assert t["DT"].to_pylist() == [dt.date(2024, 1, 15), dt.date(2023, 12, 31), None]

    t = datasuspy.read_dbc(dbf, select=["cnt", "NAME"], n_max=2)
    assert t.to_pydict() == {"name": ["ana", "bruno"], "cnt": [12, -7]}

    t = datasuspy.read_dbc(dbf, col_types={"cnt": "character", "Val": "integer"})
    assert t["cnt"].to_pylist() == ["12", "-7", ""]
    assert t["val"].to_pylist() == [None, None, None]

    assert datasuspy.read_dbc(dbf, batch_size=1).num_rows == 3


def test_warnings(dbf, tmp_path):
    with pytest.warns(UserWarning, match="nope"):
        t = datasuspy.read_dbc(dbf, select=["name", "nope"])
    assert t.column_names == ["name"]

    short = make_dbf(tmp_path / "short.dbf", [("ID", "N", 3, 0)], [b"   1", b"   2"], n_records=5)
    with pytest.warns(UserWarning, match="declares 5 records but only 2 are present"):
        assert datasuspy.read_dbc(short)["id"].to_pylist() == [1, 2]


def test_errors(dbf, tmp_path):
    with pytest.raises(OSError):
        datasuspy.read_dbc(tmp_path / "missing.dbc")
    junk = tmp_path / "junk.dbc"
    junk.write_bytes(bytes([0xFF, 0x00, 0x13, 0x37, 0x42]))
    with pytest.raises(ValueError):
        datasuspy.read_dbc(junk)
    with pytest.raises(ValueError, match="encoding"):
        datasuspy.read_dbc(dbf, encoding="cp1252")
    with pytest.raises(ValueError, match="invalid type"):
        datasuspy.read_dbc(dbf, col_types={"cnt": "float"})


def test_open_dbc_stream(dbf):
    reader = datasuspy.open_dbc(dbf, batch_size=2)
    assert (reader.n_rows, reader.n_cols) == (3, 6)
    assert reader.columns[:2] == [("NAME", "character"), ("CNT", "integer")]
    batches = list(pa.RecordBatchReader.from_stream(reader))
    assert [b.num_rows for b in batches] == [2, 1]
    with pytest.raises(RuntimeError, match="already consumed"):
        pa.table(reader)


def test_empty_table(tmp_path):
    p = make_dbf(tmp_path / "empty.dbf", [("ID", "N", 3, 0)], [b"*  1"])
    t = datasuspy.read_dbc(p)
    assert t.num_rows == 0 and t.column_names == ["id"]
