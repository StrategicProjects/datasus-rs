//! Container detection: turn the bytes of a `.dbc` (or `.dbf`) file into a
//! DBF header plus an uncompressed record area.
//!
//! Mirrors `dbc_reader.c` from the datasusr R package, including the order
//! in which the three layouts are tried.

use std::ops::Range;

use crate::blast::blast;
use crate::Error;

/// Bytes between the DBF header and the compressed payload in a DBC file.
const CRC_OFFSET: usize = 4;

/// How the input was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// Uncompressed DBF with all declared records present.
    PlainDbf,
    /// Uncompressed DBF with fewer bytes than the header declares.
    TruncatedDbf,
    /// DATASUS DBC: plain DBF header followed by a compressed record area.
    Dbc,
    /// A whole DBF file compressed as a single PKWare DCL stream.
    WholeFileDbc,
}

enum Records {
    InBuf(Range<usize>),
    Owned(Vec<u8>),
}

/// An uncompressed DBF: header bytes and record area.
pub struct DbfData {
    buf: Vec<u8>,
    header_len: usize,
    records: Records,
    container: Container,
}

impl DbfData {
    pub fn header(&self) -> &[u8] {
        &self.buf[..self.header_len]
    }

    pub fn records(&self) -> &[u8] {
        match &self.records {
            Records::InBuf(r) => &self.buf[r.clone()],
            Records::Owned(v) => v,
        }
    }

    pub fn container(&self) -> Container {
        self.container
    }
}

fn le_u16(p: &[u8]) -> u16 {
    u16::from_le_bytes([p[0], p[1]])
}

fn le_u32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

fn looks_like_dbf_header(buf: &[u8]) -> bool {
    if buf.len() < 32 {
        return false;
    }
    let version_ok = matches!(
        buf[0],
        0x02 | 0x03 | 0x04 | 0x05 | 0x30 | 0x31 | 0x32 | 0x43 | 0x63 | 0x7B | 0x83 | 0x8B | 0x8E
            | 0xCB | 0xF5
    );
    if !version_ok {
        return false;
    }
    let header_len = le_u16(&buf[8..]) as usize;
    let record_len = le_u16(&buf[10..]) as usize;
    if header_len < 33 || record_len < 1 || header_len >= buf.len() {
        return false;
    }
    matches!(buf[header_len - 1], 0x0D | 0x00)
}

fn looks_like_complete_dbf(buf: &[u8]) -> bool {
    if !looks_like_dbf_header(buf) {
        return false;
    }
    let n_records = le_u32(&buf[4..]) as u64;
    let header_len = le_u16(&buf[8..]) as u64;
    let record_len = le_u16(&buf[10..]) as u64;
    buf.len() as u64 >= header_len + n_records * record_len
}

/// Every record of a plain DBF starts with a deletion flag (`' '` live,
/// `'*'` deleted), optionally followed by the 0x1A end-of-file marker.
fn looks_like_plain_dbf_records(buf: &[u8], header_len: usize, record_len: usize) -> bool {
    if header_len > buf.len() || record_len == 0 {
        return false;
    }
    let mut off = header_len;
    while off < buf.len() {
        if off == buf.len() - 1 && buf[off] == 0x1A {
            break;
        }
        if buf[off] != b' ' && buf[off] != b'*' {
            return false;
        }
        off += record_len;
    }
    true
}

/// Capacity hint for the decompressed record area, bounded so a hostile
/// header cannot trigger a huge allocation up front.
fn capacity_hint(header: &[u8], compressed_len: usize) -> usize {
    let declared = le_u32(&header[4..]) as u64 * le_u16(&header[10..]) as u64 + 1;
    declared.min(compressed_len as u64 * 64).min(1 << 31) as usize
}

/// Recognise the layout of `buf` and decompress the record area if needed.
pub fn decode(buf: Vec<u8>) -> Result<DbfData, Error> {
    // A plain DBF must not only be long enough for the declared records:
    // every record must also start with a deletion flag.  A small DBC whose
    // compressed payload is larger than the uncompressed record area passes
    // the size test alone.
    if looks_like_complete_dbf(&buf) {
        let header_len = le_u16(&buf[8..]) as usize;
        let record_len = le_u16(&buf[10..]) as usize;
        if looks_like_plain_dbf_records(&buf, header_len, record_len) {
            let records = Records::InBuf(header_len..buf.len());
            return Ok(DbfData { buf, header_len, records, container: Container::PlainDbf });
        }
    }

    if looks_like_dbf_header(&buf) {
        let header_len = le_u16(&buf[8..]) as usize;
        let record_len = le_u16(&buf[10..]) as usize;

        let mut out = Vec::new();
        let mut result = Err(crate::blast::BlastError::OutOfInput);
        let mut code = -1;

        if header_len + CRC_OFFSET < buf.len() {
            let payload = &buf[header_len + CRC_OFFSET..];
            out.reserve(capacity_hint(&buf, payload.len()));
            result = blast(payload, &mut out);
            if let Err(e) = result {
                code = e.code();
            }
        }

        // Some DBC files have no CRC bytes between header and payload.
        if result.is_err() {
            out.clear();
            result = blast(&buf[header_len..], &mut out);
            if let Err(e) = result {
                code = e.code();
            }
        }

        if result.is_err() {
            // Not a DBC: a truncated plain DBF.  Read what is there.
            if looks_like_plain_dbf_records(&buf, header_len, record_len) {
                let records = Records::InBuf(header_len..buf.len());
                return Ok(DbfData { buf, header_len, records, container: Container::TruncatedDbf });
            }
            return Err(Error::Blast { code, record_area: true });
        }

        return Ok(DbfData {
            buf,
            header_len,
            records: Records::Owned(out),
            container: Container::Dbc,
        });
    }

    let mut out = Vec::new();
    if let Err(e) = blast(&buf, &mut out) {
        return Err(Error::Blast { code: e.code(), record_area: false });
    }
    if !looks_like_complete_dbf(&out) {
        return Err(Error::NotDbf);
    }
    let header_len = le_u16(&out[8..]) as usize;
    let records = Records::InBuf(header_len..out.len());
    Ok(DbfData { buf: out, header_len, records, container: Container::WholeFileDbc })
}
