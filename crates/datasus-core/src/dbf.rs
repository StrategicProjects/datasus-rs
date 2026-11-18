//! dBase (DBF) parsing: header validation, column typing and record
//! decoding.  Mirrors `dbf_parser.c` from the datasusr R package.
//!
//! Values are pushed into a [`Sink`] so each front end (Arrow builders, R
//! vectors, ...) allocates its own columns and no intermediate table is
//! built.

use crate::container::DbfData;
use crate::Error;

/// Output type of a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColType {
    Character,
    Integer,
    Double,
    Logical,
    Date,
}

impl ColType {
    /// Parse the names used by datasusr's `col_types`.
    pub fn from_name(x: &str) -> Option<ColType> {
        match x {
            "character" => Some(ColType::Character),
            "integer" => Some(ColType::Integer),
            "double" => Some(ColType::Double),
            "logical" => Some(ColType::Logical),
            "date" => Some(ColType::Date),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ColType::Character => "character",
            ColType::Integer => "integer",
            ColType::Double => "double",
            ColType::Logical => "logical",
            ColType::Date => "date",
        }
    }
}

/// Encoding of character fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// ISO-8859-1, the encoding of DATASUS files.
    #[default]
    Latin1,
    Utf8,
}

impl Encoding {
    /// Accepts the same aliases as datasusr (case-insensitive).
    pub fn from_name(x: &str) -> Option<Encoding> {
        match x.to_ascii_lowercase().as_str() {
            "latin1" | "latin-1" | "iso-8859-1" | "iso8859-1" => Some(Encoding::Latin1),
            "utf-8" | "utf8" => Some(Encoding::Utf8),
            _ => None,
        }
    }

    /// Append `raw` to `out` as UTF-8.  Invalid UTF-8 input is replaced
    /// with U+FFFD.
    pub fn decode_into(self, raw: &[u8], out: &mut String) {
        match std::str::from_utf8(raw) {
            Ok(s) if self == Encoding::Utf8 || raw.is_ascii() => out.push_str(s),
            _ => match self {
                Encoding::Latin1 => out.extend(raw.iter().map(|&b| b as char)),
                Encoding::Utf8 => out.push_str(&String::from_utf8_lossy(raw)),
            },
        }
    }

    pub fn decode(self, raw: &[u8]) -> String {
        let mut s = String::with_capacity(raw.len());
        self.decode_into(raw, &mut s);
        s
    }
}

/// Reader options; the defaults match `read_datasus_dbc()`.
#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// Columns to keep (case-insensitive).  `None` or empty keeps all.
    pub select: Option<Vec<String>>,
    /// Maximum number of live (non-deleted) records to read.
    pub n_max: Option<u64>,
    /// Trim whitespace from character fields.  Numeric, date and logical
    /// fields are always trimmed before parsing.
    pub trim_ws: bool,
    pub encoding: Encoding,
    /// Scan numeric fields to tell integer columns from double columns.
    pub guess_types: bool,
    /// Explicit column types, matched case-insensitively by field name.
    pub col_types: Vec<(String, ColType)>,
    /// Return `D` fields as dates instead of text.
    pub parse_dates: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            select: None,
            n_max: None,
            trim_ws: true,
            encoding: Encoding::Latin1,
            guess_types: true,
            col_types: Vec::new(),
            parse_dates: false,
        }
    }
}

/// A field as declared in the DBF header.
#[derive(Debug, Clone)]
pub struct Field {
    /// Field name as stored (decoded as Latin-1).
    pub name: String,
    /// dBase type byte (`C`, `N`, `F`, `D`, `L`, ...).
    pub dbf_type: u8,
    pub len: u8,
    pub dec: u8,
    /// Output type; `None` when the field was not selected.
    pub col_type: Option<ColType>,
    offset: usize,
}

/// Receives decoded values, one call per selected column per row, in row
/// order.  `col` indexes [`Table::columns`]; `row` restarts at 0 on every
/// [`Table::read_rows`] call.
pub trait Sink {
    fn int(&mut self, col: usize, row: usize, value: Option<i32>);
    fn double(&mut self, col: usize, row: usize, value: Option<f64>);
    fn logical(&mut self, col: usize, row: usize, value: Option<bool>);
    /// Days since 1970-01-01.
    fn date(&mut self, col: usize, row: usize, value: Option<i32>);
    /// Raw field bytes in the file encoding (see [`Encoding::decode_into`]),
    /// already trimmed.  Character values are never missing; a blank field
    /// is the empty string.
    fn text(&mut self, col: usize, row: usize, raw: &[u8]);
}

/// Position inside the record area, so rows can be read in batches.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cursor {
    raw: usize,
    row: usize,
}

/// A parsed DBF header plus the record area it describes.
pub struct Table {
    data: DbfData,
    fields: Vec<Field>,
    columns: Vec<usize>,
    record_len: usize,
    n_scan: usize,
    n_rows: usize,
    declared_records: u32,
    trim_ws: bool,
    encoding: Encoding,
    warnings: Vec<String>,
    unmatched_select: Vec<String>,
}

fn le_u16(p: &[u8]) -> u16 {
    u16::from_le_bytes([p[0], p[1]])
}

fn le_u32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

/// C `isspace()` in the "C" locale.
#[inline]
fn is_space(b: u8) -> bool {
    b == b' ' || (0x09..=0x0D).contains(&b)
}

#[inline]
fn trim(src: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = src.len();
    while s < e && is_space(src[s]) {
        s += 1;
    }
    while e > s && is_space(src[e - 1]) {
        e -= 1;
    }
    &src[s..e]
}

/// The C reader copies fields into NUL-terminated strings, so a value ends
/// at the first NUL byte (DATASUS files do pad some fields with NULs).
#[inline]
fn until_nul(src: &[u8]) -> &[u8] {
    match src.iter().position(|&b| b == 0) {
        Some(i) => &src[..i],
        None => src,
    }
}

/// Decimal integer representable as a non-missing R integer: `i32::MIN` is
/// rejected because it is R's `NA_integer_`.
fn parse_int(x: &[u8]) -> Option<i32> {
    let (neg, digits) = match x.first() {
        Some(b'-') => (true, &x[1..]),
        Some(b'+') => (false, &x[1..]),
        _ => (false, x),
    };
    if digits.is_empty() {
        return None;
    }
    let mut val: i64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val * 10 + (b - b'0') as i64;
        if val > i32::MAX as i64 + 1 {
            return None;
        }
    }
    let val = if neg { -val } else { val };
    if val <= i32::MIN as i64 || val > i32::MAX as i64 {
        return None;
    }
    Some(val as i32)
}

/// `strtod()` over the whole string.  Hexadecimal floats and `nan(...)`,
/// which `strtod()` accepts, are not supported.
fn parse_double(x: &[u8]) -> Option<f64> {
    std::str::from_utf8(x).ok()?.parse::<f64>().ok()
}

fn days_in_month(year: i32, month: i32) -> i32 {
    const MDAYS: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        return if leap { 29 } else { 28 };
    }
    MDAYS[(month - 1) as usize]
}

/// `YYYYMMDD` (first 8 bytes of `src`) to days since 1970-01-01.
fn parse_date(src: &[u8]) -> Option<i32> {
    if src.len() < 8 || !src[..8].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let d = |i: usize| (src[i] - b'0') as i32;
    let year = d(0) * 1000 + d(1) * 100 + d(2) * 10 + d(3);
    let month = d(4) * 10 + d(5);
    let day = d(6) * 10 + d(7);
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = year - (month <= 2) as i32;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

impl Table {
    /// Validate the header of `data` and decide which columns to read and
    /// with which types.
    pub fn new(data: DbfData, opts: &ReadOptions) -> Result<Table, Error> {
        let header = data.header();
        let records = data.records();

        if header.len() < 32 {
            return Err(Error::Header("Invalid DBF header: too small.".into()));
        }
        let declared_records = le_u32(&header[4..]);
        let header_len_decl = le_u16(&header[8..]) as usize;
        let record_len = le_u16(&header[10..]) as usize;

        if header_len_decl > header.len() || record_len == 0 {
            return Err(Error::Header("Invalid DBF header.".into()));
        }
        if header_len_decl < 33 || (header_len_decl - 33) % 32 != 0 {
            return Err(Error::Header("Invalid DBF header field descriptor section.".into()));
        }
        let n_fields = (header_len_decl - 33) / 32;
        if n_fields == 0 {
            return Err(Error::Header("No DBF fields found.".into()));
        }

        let mut sum_len = 1usize;
        for i in 0..n_fields {
            let flen = header[32 + i * 32 + 16] as usize;
            if flen == 0 {
                return Err(Error::Header(format!(
                    "Invalid DBF header: field {} has length 0.",
                    i + 1
                )));
            }
            sum_len += flen;
        }
        if sum_len > record_len {
            return Err(Error::Header(format!(
                "Invalid DBF header: field lengths (1 + {} bytes) exceed the declared record length ({} bytes).",
                sum_len - 1,
                record_len
            )));
        }

        let mut warnings = Vec::new();
        let mut n_records = declared_records as usize;
        let available = (records.len() / record_len).min(u32::MAX as usize);
        if available < n_records {
            warnings.push(format!(
                "DBF declares {n_records} records but only {available} are present; reading {available}."
            ));
            n_records = available;
        }

        // Deleted records ('*' flag) are skipped; n_max counts live records.
        let n_max = opts.n_max.unwrap_or(u64::MAX);
        let mut n_scan = 0usize;
        let mut n_rows = 0usize;
        for r in 0..n_records {
            if n_rows as u64 >= n_max {
                break;
            }
            n_scan = r + 1;
            if records[r * record_len] != b'*' {
                n_rows += 1;
            }
        }

        let select: Vec<String> = opts
            .select
            .iter()
            .flatten()
            .map(|s| s.to_uppercase())
            .collect();
        let declared: Vec<(String, ColType)> = opts
            .col_types
            .iter()
            .map(|(n, t)| (n.to_uppercase(), *t))
            .collect();

        let mut fields = Vec::with_capacity(n_fields);
        let mut columns = Vec::new();
        let mut upper_names = Vec::new();
        let mut offset = 1usize;

        for i in 0..n_fields {
            let fd = &header[32 + i * 32..32 + i * 32 + 32];
            let name = Encoding::Latin1.decode(until_nul(&fd[..11]));
            let upper = name.to_ascii_uppercase();
            let dbf_type = fd[11];
            let len = fd[16];
            let dec = fd[17];
            let keep = select.is_empty() || select.contains(&upper);

            let mut field = Field { name, dbf_type, len, dec, col_type: None, offset };
            offset += len as usize;

            if keep {
                let target = match declared.iter().find(|(n, _)| *n == upper) {
                    Some((_, t)) => *t,
                    None => match dbf_type {
                        b'L' => ColType::Logical,
                        b'D' if opts.parse_dates => ColType::Date,
                        b'D' => ColType::Character,
                        b'N' | b'F' if opts.guess_types => {
                            infer_numeric(&field, records, n_scan, record_len)
                        }
                        b'N' | b'F' if dec > 0 => ColType::Double,
                        b'N' | b'F' => ColType::Integer,
                        _ => ColType::Character,
                    },
                };
                field.col_type = Some(target);
                columns.push(i);
                upper_names.push(upper);
            }
            fields.push(field);
        }

        let mut unmatched_select: Vec<String> = Vec::new();
        for s in opts.select.iter().flatten() {
            let up = s.to_uppercase();
            if !upper_names.contains(&up) && !unmatched_select.contains(s) {
                unmatched_select.push(s.clone());
            }
        }

        Ok(Table {
            data,
            fields,
            columns,
            record_len,
            n_scan,
            n_rows,
            declared_records,
            trim_ws: opts.trim_ws,
            encoding: opts.encoding,
            warnings,
            unmatched_select,
        })
    }

    /// All fields declared in the header, selected or not.
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Selected columns, in file order.
    pub fn columns(&self) -> impl ExactSizeIterator<Item = &Field> + '_ {
        self.columns.iter().map(|&i| &self.fields[i])
    }

    pub fn n_cols(&self) -> usize {
        self.columns.len()
    }

    /// Number of rows that reading will produce (live records, capped by
    /// `n_max`).
    pub fn n_rows(&self) -> usize {
        self.n_rows
    }

    /// Record count declared in the header (includes deleted records).
    pub fn declared_records(&self) -> u32 {
        self.declared_records
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Non-fatal problems found in the file (e.g. missing records).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Names in `select` that matched no field.
    pub fn unmatched_select(&self) -> &[String] {
        &self.unmatched_select
    }

    pub fn data(&self) -> &DbfData {
        &self.data
    }

    /// Decode up to `max_rows` rows starting at `cursor`, pushing the values
    /// into `sink`.  Returns the number of rows decoded; 0 means the end.
    pub fn read_rows<S: Sink>(&self, cursor: &mut Cursor, max_rows: usize, sink: &mut S) -> usize {
        let records = self.data.records();
        let mut done = 0usize;

        while cursor.raw < self.n_scan && cursor.row < self.n_rows && done < max_rows {
            let start = cursor.raw * self.record_len;
            let rec = &records[start..start + self.record_len];
            cursor.raw += 1;

            if rec[0] == b'*' {
                continue;
            }

            for (c, &fi) in self.columns.iter().enumerate() {
                let f = &self.fields[fi];
                let raw = &rec[f.offset..f.offset + f.len as usize];

                match f.col_type {
                    Some(ColType::Integer) => sink.int(c, done, parse_int(until_nul(trim(raw)))),
                    Some(ColType::Double) => {
                        sink.double(c, done, parse_double(until_nul(trim(raw))))
                    }
                    Some(ColType::Logical) => {
                        let v = match trim(raw).first() {
                            Some(b'Y' | b'y' | b'T' | b't') => Some(true),
                            Some(b'N' | b'n' | b'F' | b'f') => Some(false),
                            _ => None,
                        };
                        sink.logical(c, done, v);
                    }
                    Some(ColType::Date) => {
                        let v = if f.dbf_type == b'D' {
                            parse_date(trim(raw))
                        } else {
                            let t = until_nul(trim(raw));
                            if t.len() == 8 { parse_date(t) } else { None }
                        };
                        sink.date(c, done, v);
                    }
                    Some(ColType::Character) | None => {
                        let t = if self.trim_ws { trim(raw) } else { raw };
                        sink.text(c, done, until_nul(t));
                    }
                }
            }

            done += 1;
            cursor.row += 1;
        }
        done
    }
}

/// Scan the first `n_scan` records (deleted ones skipped) of a numeric
/// field: integer unless some value needs a double.
fn infer_numeric(field: &Field, records: &[u8], n_scan: usize, record_len: usize) -> ColType {
    if field.dec > 0 {
        return ColType::Double;
    }
    for r in 0..n_scan {
        let rec = &records[r * record_len..];
        if rec[0] == b'*' {
            continue;
        }
        let t = until_nul(trim(&rec[field.offset..field.offset + field.len as usize]));
        if t.is_empty() {
            continue;
        }
        if t.iter().any(|&b| b == b'.' || b == b'e' || b == b'E') || parse_int(t).is_none() {
            return ColType::Double;
        }
    }
    ColType::Integer
}

/// A decoded column, for callers that just want the data in memory.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    Integer(Vec<Option<i32>>),
    Double(Vec<Option<f64>>),
    Logical(Vec<Option<bool>>),
    /// Days since 1970-01-01.
    Date(Vec<Option<i32>>),
    Character(Vec<String>),
}

struct Collect {
    cols: Vec<ColumnData>,
    encoding: Encoding,
}

impl Sink for Collect {
    fn int(&mut self, col: usize, _: usize, value: Option<i32>) {
        if let ColumnData::Integer(v) = &mut self.cols[col] {
            v.push(value);
        }
    }
    fn double(&mut self, col: usize, _: usize, value: Option<f64>) {
        if let ColumnData::Double(v) = &mut self.cols[col] {
            v.push(value);
        }
    }
    fn logical(&mut self, col: usize, _: usize, value: Option<bool>) {
        if let ColumnData::Logical(v) = &mut self.cols[col] {
            v.push(value);
        }
    }
    fn date(&mut self, col: usize, _: usize, value: Option<i32>) {
        if let ColumnData::Date(v) = &mut self.cols[col] {
            v.push(value);
        }
    }
    fn text(&mut self, col: usize, _: usize, raw: &[u8]) {
        if let ColumnData::Character(v) = &mut self.cols[col] {
            v.push(self.encoding.decode(raw));
        }
    }
}

impl Table {
    /// Read every row into owned columns (text decoded to UTF-8).
    pub fn collect(&self) -> Vec<ColumnData> {
        let n = self.n_rows;
        let cols = self
            .columns()
            .map(|f| match f.col_type {
                Some(ColType::Integer) => ColumnData::Integer(Vec::with_capacity(n)),
                Some(ColType::Double) => ColumnData::Double(Vec::with_capacity(n)),
                Some(ColType::Logical) => ColumnData::Logical(Vec::with_capacity(n)),
                Some(ColType::Date) => ColumnData::Date(Vec::with_capacity(n)),
                _ => ColumnData::Character(Vec::with_capacity(n)),
            })
            .collect();
        let mut sink = Collect { cols, encoding: self.encoding };
        self.read_rows(&mut Cursor::default(), usize::MAX, &mut sink);
        sink.cols
    }
}

/// Make column names unique the way tibble's `"unique"` repair does: empty
/// and duplicated names get a `...<position>` suffix.  With `clean`, names
/// are lowercased afterwards (datasusr's `clean_names`).
pub fn repair_names<'a>(names: impl Iterator<Item = &'a str>, clean: bool) -> Vec<String> {
    let names: Vec<&str> = names.collect();
    names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let dup = n.is_empty() || names.iter().filter(|m| m == &n).count() > 1;
            let out = if dup { format!("{n}...{}", i + 1) } else { n.to_string() };
            if clean { out.to_lowercase() } else { out }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints() {
        assert_eq!(parse_int(b"12"), Some(12));
        assert_eq!(parse_int(b"-7"), Some(-7));
        assert_eq!(parse_int(b"+7"), Some(7));
        assert_eq!(parse_int(b"2147483647"), Some(i32::MAX));
        assert_eq!(parse_int(b"2147483648"), None);
        assert_eq!(parse_int(b"-2147483647"), Some(-2147483647));
        assert_eq!(parse_int(b"-2147483648"), None);
        assert_eq!(parse_int(b"99999999999999999999999999"), None);
        assert_eq!(parse_int(b""), None);
        assert_eq!(parse_int(b"-"), None);
        assert_eq!(parse_int(b"1 2"), None);
        assert_eq!(parse_int(b"1.0"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date(b"19700101"), Some(0));
        assert_eq!(parse_date(b"20240115"), Some(19737));
        assert_eq!(parse_date(b"20240229"), Some(19782));
        assert_eq!(parse_date(b"20230229"), None);
        assert_eq!(parse_date(b"19000229"), None);
        assert_eq!(parse_date(b"2024130a"), None);
        assert_eq!(parse_date(b"2024"), None);
    }

    #[test]
    fn trimming() {
        assert_eq!(trim(b"  ab \t"), b"ab");
        assert_eq!(trim(b"    "), b"");
        assert_eq!(until_nul(b"ab\0\0"), b"ab");
        assert_eq!(until_nul(trim(b"\0\0\0")), b"");
    }

    #[test]
    fn names() {
        let r = repair_names(["A", "B", "A", ""].into_iter(), true);
        assert_eq!(r, ["a...1", "b", "a...3", "...4"]);
    }
}
