"""Read DATASUS ``.dbc``/``.dbf`` files into Apache Arrow.

>>> import datasuspy
>>> table = datasuspy.read_dbc("RDPE2401.dbc")          # pyarrow.Table
>>> df = table.to_pandas()
>>> import polars as pl
>>> df = pl.from_arrow(datasuspy.read_dbc("RDPE2401.dbc", select=["uf_zi", "val_tot"]))
"""

from __future__ import annotations

import os
import warnings
from typing import TYPE_CHECKING, Mapping, Sequence

from ._core import DbcReader, __version__
from ._core import open_dbc as _open_dbc

if TYPE_CHECKING:
    import pyarrow

__all__ = ["DbcReader", "open_dbc", "read_dbc", "__version__"]


def open_dbc(
    path: str | os.PathLike,
    *,
    select: Sequence[str] | None = None,
    n_max: int | None = None,
    trim_ws: bool = True,
    encoding: str = "latin1",
    guess_types: bool = True,
    col_types: Mapping[str, str] | None = None,
    parse_dates: bool = False,
    clean_names: bool = True,
    batch_size: int = 65_536,
) -> DbcReader:
    """Open a ``.dbc`` or ``.dbf`` file as a stream of Arrow record batches.

    The file is read and decompressed here; records are decoded lazily, in
    batches of ``batch_size`` rows, when the stream is consumed. The result
    implements the Arrow PyCapsule interface (``__arrow_c_stream__``), so it
    can be handed once to ``pyarrow.RecordBatchReader.from_stream``,
    ``polars.from_arrow``, DuckDB, etc.

    Parameters
    ----------
    select
        Columns to keep (case-insensitive). ``None`` keeps all. Names that
        match no column raise a warning.
    n_max
        Maximum number of rows. Records flagged as deleted are always
        skipped and do not count.
    trim_ws
        Trim whitespace from character fields. Numeric, date and logical
        fields are always trimmed before parsing.
    encoding
        ``"latin1"`` (DATASUS files) or ``"utf-8"``.
    guess_types
        Scan numeric fields to tell integer columns from float columns.
    col_types
        Explicit types by column name (case-insensitive): ``"character"``,
        ``"integer"``, ``"double"``, ``"logical"`` or ``"date"``.
    parse_dates
        Return DBF date fields (type ``D``) as dates instead of text.
    clean_names
        Lowercase the column names.
    """
    if isinstance(select, str):
        select = [select]
    reader = _open_dbc(
        os.fspath(path),
        select=list(select) if select is not None else None,
        n_max=n_max,
        trim_ws=trim_ws,
        encoding=encoding,
        guess_types=guess_types,
        col_types=list(col_types.items()) if col_types else None,
        parse_dates=parse_dates,
        clean_names=clean_names,
        batch_size=batch_size,
    )
    for message in reader.warnings:
        warnings.warn(message, stacklevel=2)
    if reader.unmatched_select:
        warnings.warn(
            "names in `select` matched no column: " + ", ".join(reader.unmatched_select),
            stacklevel=2,
        )
    return reader


def read_dbc(path: str | os.PathLike, **kwargs) -> "pyarrow.Table":
    """Read a ``.dbc`` or ``.dbf`` file into a :class:`pyarrow.Table`.

    Takes the same keyword arguments as :func:`open_dbc`.
    """
    import pyarrow as pa

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        reader = open_dbc(path, **kwargs)
    for w in caught:
        warnings.warn(w.message, w.category, stacklevel=2)
    return pa.table(reader)
