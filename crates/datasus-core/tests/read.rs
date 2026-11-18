//! Port of datasusr's `tests/testthat/test-read_dbc.R`: synthetic dBase III
//! files, same expectations.

use datasus_core::{from_bytes, ColType, ColumnData, Container, Encoding, ReadOptions, Table};

struct F(&'static str, u8, u8, u8); // name, type, len, dec

/// Build a minimal dBase III (0x03) file in memory.  Each record must be
/// `1 + sum(len)` bytes; the first byte is the deletion flag.
fn make_dbf(fields: &[F], records: &[&[u8]], n_records: Option<u32>, record_len: Option<u16>) -> Vec<u8> {
    let header_len = 32 + 32 * fields.len() as u16 + 1;
    let record_len =
        record_len.unwrap_or(1 + fields.iter().map(|f| f.2 as u16).sum::<u16>());
    let mut buf = vec![0x03, 24, 1, 1];
    buf.extend(n_records.unwrap_or(records.len() as u32).to_le_bytes());
    buf.extend(header_len.to_le_bytes());
    buf.extend(record_len.to_le_bytes());
    buf.extend([0u8; 20]);
    for f in fields {
        let mut fd = [0u8; 32];
        fd[..f.0.len()].copy_from_slice(f.0.as_bytes());
        fd[11] = f.1;
        fd[16] = f.2;
        fd[17] = f.3;
        buf.extend(fd);
    }
    buf.push(0x0D);
    for r in records {
        buf.extend(*r);
    }
    buf.push(0x1A);
    buf
}

fn read(bytes: Vec<u8>, opts: &ReadOptions) -> Table {
    from_bytes(bytes, opts).unwrap()
}

fn opts() -> ReadOptions {
    ReadOptions::default()
}

fn names(t: &Table) -> Vec<String> {
    t.columns().map(|f| f.name.clone()).collect()
}

fn s(v: &[&str]) -> ColumnData {
    ColumnData::Character(v.iter().map(|x| x.to_string()).collect())
}

fn basic() -> Vec<u8> {
    let fields = [
        F("NAME", b'C', 5, 0),
        F("CNT", b'N', 4, 0),
        F("VAL", b'N', 6, 2),
        F("RATE", b'F', 6, 0),
        F("DT", b'D', 8, 0),
        F("OK", b'L', 1, 0),
    ];
    make_dbf(
        &fields,
        &[
            b" ana    12  3.50   1.520240115T",
            b" bruno  -7 10.25     220231231F",
            b"                              ?",
        ],
        None,
        None,
    )
}

#[test]
fn happy_path() {
    let t = read(basic(), &opts());
    assert_eq!(t.data().container(), Container::PlainDbf);
    assert_eq!(names(&t), ["NAME", "CNT", "VAL", "RATE", "DT", "OK"]);
    assert_eq!(t.n_rows(), 3);
    let c = t.collect();
    assert_eq!(c[0], s(&["ana", "bruno", ""]));
    assert_eq!(c[1], ColumnData::Integer(vec![Some(12), Some(-7), None]));
    assert_eq!(c[2], ColumnData::Double(vec![Some(3.5), Some(10.25), None]));
    assert_eq!(c[3], ColumnData::Double(vec![Some(1.5), Some(2.0), None]));
    assert_eq!(c[4], s(&["20240115", "20231231", ""]));
    assert_eq!(c[5], ColumnData::Logical(vec![Some(true), Some(false), None]));
}

#[test]
fn parse_dates_col_types_n_max() {
    let dates = ColumnData::Date(vec![Some(19737), Some(19722), None]);

    let t = read(basic(), &ReadOptions { parse_dates: true, ..opts() });
    assert_eq!(t.collect()[4], dates);

    let col_types = vec![
        ("cnt".to_string(), ColType::Character),
        ("Val".to_string(), ColType::Integer),
        ("dt".to_string(), ColType::Date),
    ];
    let c = read(basic(), &ReadOptions { col_types, ..opts() }).collect();
    assert_eq!(c[1], s(&["12", "-7", ""]));
    assert_eq!(c[2], ColumnData::Integer(vec![None, None, None]));
    assert_eq!(c[4], dates);

    let t = read(basic(), &ReadOptions { n_max: Some(2), ..opts() });
    assert_eq!(t.n_rows(), 2);
    assert_eq!(t.collect()[0], s(&["ana", "bruno"]));

    let t = read(basic(), &ReadOptions { guess_types: false, ..opts() });
    let types: Vec<_> = t.columns().map(|f| f.col_type.unwrap()).collect();
    assert_eq!(types[1], ColType::Integer);
    assert_eq!(types[2], ColType::Double);
    assert_eq!(types[3], ColType::Integer);
}

#[test]
fn encoding() {
    let p = make_dbf(&[F("TXT", b'C', 4, 0)], &[b" caf\xe9"], None, None);
    assert_eq!(read(p.clone(), &opts()).collect()[0], s(&["caf\u{e9}"]));
    let utf8 = ReadOptions { encoding: Encoding::Utf8, ..opts() };
    assert_eq!(read(p, &utf8).collect()[0], s(&["caf\u{fffd}"]));
    assert_eq!(Encoding::from_name("ISO-8859-1"), Some(Encoding::Latin1));
    assert_eq!(Encoding::from_name("Latin-1"), Some(Encoding::Latin1));
    assert_eq!(Encoding::from_name("utf-8"), Some(Encoding::Utf8));
    assert_eq!(Encoding::from_name("CP1252"), None);
}

#[test]
fn bad_field_lengths() {
    let f = [F("A", b'C', 200, 0), F("B", b'C', 200, 0)];
    let p = make_dbf(&f, &[&[b' '; 11]], None, Some(11));
    let e = from_bytes(p, &opts()).err().unwrap().to_string();
    assert!(e.contains("exceed the declared record length"), "{e}");

    let p = make_dbf(&[F("A", b'C', 0, 0)], &[b" "], None, Some(1));
    let e = from_bytes(p, &opts()).err().unwrap().to_string();
    assert!(e.contains("length 0"), "{e}");
}

#[test]
fn deleted_records() {
    let f = [F("ID", b'N', 3, 0)];
    let recs: [&[u8]; 5] = [b"*  1", b"   2", b"*  3", b"   4", b"   5"];
    let p = make_dbf(&f, &recs, None, None);
    let ints = |v: &[i32]| ColumnData::Integer(v.iter().map(|x| Some(*x)).collect());
    assert_eq!(read(p.clone(), &opts()).collect()[0], ints(&[2, 4, 5]));
    let n = |n| ReadOptions { n_max: Some(n), ..opts() };
    assert_eq!(read(p.clone(), &n(2)).collect()[0], ints(&[2, 4]));
    assert_eq!(read(p, &n(1)).collect()[0], ints(&[2]));

    let t = read(make_dbf(&f, &[b"*  1", b"*  2"], None, None), &opts());
    assert_eq!(t.n_rows(), 0);
    assert_eq!(names(&t), ["ID"]);
    assert_eq!(t.collect()[0], ints(&[]));
}

#[test]
fn integer_overflow() {
    let f = [F("N", b'N', 12, 0)];
    let force_int = ReadOptions { col_types: vec![("N".into(), ColType::Integer)], ..opts() };

    let big = make_dbf(&f, &[b"   2147483648", b"            1"], None, None);
    assert_eq!(
        read(big, &opts()).collect()[0],
        ColumnData::Double(vec![Some(2147483648.0), Some(1.0)])
    );

    let imin = make_dbf(&f, &[b"  -2147483648", b"            5"], None, None);
    assert_eq!(
        read(imin.clone(), &opts()).collect()[0],
        ColumnData::Double(vec![Some(-2147483648.0), Some(5.0)])
    );
    assert_eq!(read(imin, &force_int).collect()[0], ColumnData::Integer(vec![None, Some(5)]));

    let huge = make_dbf(&f, &[b"  99999999999"], None, None);
    assert_eq!(read(huge, &force_int).collect()[0], ColumnData::Integer(vec![None]));

    let imax = make_dbf(&f, &[b"   2147483647"], None, None);
    assert_eq!(read(imax, &opts()).collect()[0], ColumnData::Integer(vec![Some(i32::MAX)]));
}

#[test]
fn trim_ws_only_affects_character() {
    let f = [
        F("C", b'C', 4, 0),
        F("N", b'N', 5, 0),
        F("D", b'N', 6, 1),
        F("L", b'L', 1, 0),
        F("DT", b'D', 10, 0),
    ];
    let p = make_dbf(&f, &[b"  ab 12    1.5  T 20240229 "], None, None);
    let o = ReadOptions { trim_ws: false, parse_dates: true, ..opts() };
    let c = read(p.clone(), &o).collect();
    assert_eq!(c[0], s(&[" ab "]));
    assert_eq!(c[1], ColumnData::Integer(vec![Some(12)]));
    assert_eq!(c[2], ColumnData::Double(vec![Some(1.5)]));
    assert_eq!(c[3], ColumnData::Logical(vec![Some(true)]));
    assert_eq!(c[4], ColumnData::Date(vec![Some(19782)]));
    assert_eq!(read(p, &opts()).collect()[0], s(&["ab"]));
}

#[test]
fn invalid_dates() {
    let recs: [&[u8]; 8] = [
        b" 20240231", b" 20230229", b" 20240229", b" 20000229", b" 19000229", b" 20240431",
        b" 20241231", b" 2024130a",
    ];
    let p = make_dbf(&[F("DT", b'D', 8, 0)], &recs, None, None);
    let c = read(p, &ReadOptions { parse_dates: true, ..opts() }).collect();
    assert_eq!(
        c[0],
        ColumnData::Date(vec![None, None, Some(19782), Some(11016), None, None, Some(20088), None])
    );
}

#[test]
fn missing_records_warn() {
    let p = make_dbf(&[F("ID", b'N', 3, 0)], &[b"   1", b"   2"], Some(5), None);
    let t = read(p, &opts());
    assert_eq!(t.warnings().len(), 1);
    assert!(t.warnings()[0].contains("declares 5 records but only 2 are present"));
    assert_eq!(t.collect()[0], ColumnData::Integer(vec![Some(1), Some(2)]));
}

#[test]
fn select_is_case_insensitive() {
    let f = [F("Abc", b'C', 2, 0), F("XYZ", b'C', 2, 0)];
    let p = make_dbf(&f, &[b" aaxx"], None, None);
    let sel = |v: &[&str]| ReadOptions {
        select: Some(v.iter().map(|x| x.to_string()).collect()),
        ..opts()
    };
    let t = read(p.clone(), &sel(&["abc"]));
    assert_eq!(names(&t), ["Abc"]);
    assert!(t.unmatched_select().is_empty());

    let t = read(p.clone(), &sel(&["xyz", "nope", "Other"]));
    assert_eq!(names(&t), ["XYZ"]);
    assert_eq!(t.unmatched_select(), ["nope", "Other"]);
    assert_eq!(t.collect()[0], s(&["xx"]));

    let o = ReadOptions { col_types: vec![("abc".into(), ColType::Integer)], ..opts() };
    assert_eq!(read(p, &o).columns().next().unwrap().col_type, Some(ColType::Integer));
}

#[test]
fn truncated_plain_dbf() {
    let recs: [&[u8]; 4] = [b"   1", b"   2", b"   3", b"   4"];
    let mut p = make_dbf(&[F("ID", b'N', 3, 0)], &recs, None, None);
    p.truncate(65 + 4 * 2 + 2); // header + 2.5 records, no EOF marker
    let t = read(p, &opts());
    assert_eq!(t.data().container(), Container::TruncatedDbf);
    assert!(t.warnings()[0].contains("declares 4 records but only 2 are present"));
    assert_eq!(t.collect()[0], ColumnData::Integer(vec![Some(1), Some(2)]));
}

#[test]
fn garbage_errors() {
    assert!(from_bytes(vec![0xFF, 0x00, 0x13, 0x37, 0x42], &opts()).is_err());
    assert!(from_bytes(Vec::new(), &opts()).is_err());
}

/// Literal-only PKWare DCL stream: header (0, 4), one raw literal per byte,
/// then the end code (length 519).
fn dcl_literals(data: &[u8]) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();
    let mut push = |v: u32, n: u32| (0..n).for_each(|i| bits.push(((v >> i) & 1) as u8));
    push(0, 8);
    push(4, 8);
    for &b in data {
        push(0, 1);
        push(b as u32, 8);
    }
    push(1, 1);
    push(0, 7);
    push(0xFF, 8);
    bits.chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |a, (i, b)| a | (b << i)))
        .collect()
}

#[test]
fn tiny_dbc_with_and_without_crc() {
    let payload = dcl_literals(b" X");
    assert!(payload.len() > 2);
    let plain = make_dbf(&[F("X", b'C', 1, 0)], &[b" A"], None, None);
    let hdr = &plain[..65];

    for crc in [&[0u8; 4][..], &[][..]] {
        let dbc = [hdr, crc, &payload].concat();
        let t = read(dbc, &opts());
        assert_eq!(t.data().container(), Container::Dbc);
        assert_eq!(t.collect()[0], s(&["X"]));
    }
}

#[test]
fn whole_file_dbc() {
    let plain = make_dbf(&[F("X", b'C', 1, 0)], &[b" A", b" B"], None, None);
    let t = read(dcl_literals(&plain), &opts());
    assert_eq!(t.data().container(), Container::WholeFileDbc);
    assert_eq!(t.collect()[0], s(&["A", "B"]));
}

/// PKWare DCL stream that expands to `3 + 518 * copies` zero bytes: three
/// literals, then maximum-length copies at distance 1.
fn dcl_bomb(copies: usize) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();
    let mut push = |v: u32, n: u32| (0..n).for_each(|i| bits.push(((v >> i) & 1) as u8));
    push(0, 8);
    push(4, 8);
    for _ in 0..3 {
        push(0, 1);
        push(b' ' as u32, 8);
    }
    for _ in 0..copies {
        push(1, 1);
        push(0, 7); // length symbol 15 (base 264, 8 extra bits)
        push(254, 8); // 264 + 254 = 518
        push(0b11, 2); // distance symbol 0
        push(0, 4); // distance 1
    }
    push(1, 1);
    push(0, 7);
    push(0xFF, 8);
    bits.chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |a, (i, b)| a | (b << i)))
        .collect()
}

#[test]
fn decompression_is_bounded() {
    use datasus_core::{decode, decode_with_limit, Error};

    // The bomb helper itself is a valid stream.
    let mut out = Vec::new();
    datasus_core::blast::blast(&dcl_bomb(4), &mut out, usize::MAX).unwrap();
    assert_eq!(out, vec![b' '; 3 + 518 * 4]);

    // DBC: the payload expands far beyond the one declared record, but only
    // the declared record area (+ EOF marker) is decompressed.
    let plain = make_dbf(&[F("X", b'C', 1, 0)], &[b" A"], None, None);
    let dbc = [&plain[..65], &[0u8; 4][..], &dcl_bomb(2000)].concat();
    let data = decode(dbc.clone()).unwrap();
    assert_eq!(data.records().len(), 3);
    assert_eq!(read(dbc, &opts()).collect()[0], s(&[""]));

    // A header declaring more than the limit is refused once the limit is hit.
    let mut big = make_dbf(&[F("X", b'C', 1, 0)], &[b" A"], Some(1_000_000), None);
    big.truncate(65);
    let dbc = [&big[..], &[0u8; 4][..], &dcl_bomb(2000)].concat();
    assert!(matches!(decode_with_limit(dbc, 4096), Err(Error::TooLarge { max_bytes: 4096 })));

    // Whole-file stream: bounded by the limit alone.
    assert!(matches!(decode_with_limit(dcl_bomb(2000), 4096), Err(Error::TooLarge { .. })));
}

#[test]
fn nul_padding_ends_the_value() {
    let f = [F("C", b'C', 4, 0), F("N", b'N', 4, 0)];
    let p = make_dbf(&f, &[b" ab\0\x0012\0\0", b" \0\0\0\0\0\0\0\0"], None, None);
    let c = read(p, &opts()).collect();
    assert_eq!(c[0], s(&["ab", ""]));
    assert_eq!(c[1], ColumnData::Integer(vec![Some(12), None]));
}

#[test]
fn batches() {
    use datasus_core::Cursor;
    let recs: [&[u8]; 5] = [b"*  1", b"   2", b"*  3", b"   4", b"   5"];
    let t = read(make_dbf(&[F("ID", b'N', 3, 0)], &recs, None, None), &opts());

    struct Ints(Vec<(usize, i32)>);
    impl datasus_core::Sink for Ints {
        fn int(&mut self, _: usize, row: usize, v: Option<i32>) {
            self.0.push((row, v.unwrap()));
        }
        fn double(&mut self, _: usize, _: usize, _: Option<f64>) {}
        fn logical(&mut self, _: usize, _: usize, _: Option<bool>) {}
        fn date(&mut self, _: usize, _: usize, _: Option<i32>) {}
        fn text(&mut self, _: usize, _: usize, _: &[u8]) {}
    }

    let mut cur = Cursor::default();
    let mut sink = Ints(Vec::new());
    assert_eq!(t.read_rows(&mut cur, 2, &mut sink), 2);
    assert_eq!(t.read_rows(&mut cur, 2, &mut sink), 1);
    assert_eq!(t.read_rows(&mut cur, 2, &mut sink), 0);
    assert_eq!(sink.0, [(0, 2), (1, 4), (0, 5)]);
}
