//! Decompressor for PKWare Data Compression Library (DCL "implode") streams.
//!
//! This is a Rust port of `blast.c` 1.3 by Mark Adler (zlib `contrib/blast`),
//! distributed under the zlib license:
//!
//! > Copyright (C) 2003, 2012, 2013 Mark Adler
//! >
//! > This software is provided 'as-is', without any express or implied
//! > warranty.  In no event will the author be held liable for any damages
//! > arising from the use of this software.
//! >
//! > Permission is granted to anyone to use this software for any purpose,
//! > including commercial applications, and to alter it and redistribute it
//! > freely, subject to the following restrictions:
//! >
//! > 1. The origin of this software must not be misrepresented; you must not
//! >    claim that you wrote the original software. If you use this software
//! >    in a product, an acknowledgment in the product documentation would be
//! >    appreciated but is not required.
//! > 2. Altered source versions must be plainly marked as such, and must not
//! >    be misrepresented as being the original software.
//! > 3. This notice may not be removed or altered from any source distribution.
//!
//! This file is an altered version: it was translated to Rust, works on
//! in-memory slices instead of callbacks, and uses the output vector itself
//! as the sliding window.

const MAXBITS: usize = 13;

/// Error codes follow `blast.c` so messages stay comparable with the C reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlastError {
    /// Ran out of input before the end code (`blast()` code 2).
    OutOfInput,
    /// First byte (coded-literal flag) is not 0 or 1 (code -1).
    BadLiteralFlag,
    /// Dictionary size is not 4, 5 or 6 (code -2).
    BadDictSize,
    /// Distance points before the start of the output (code -3).
    DistanceTooFar,
    /// Invalid Huffman code (cannot happen with the fixed tables; code -9).
    BadCode,
}

impl BlastError {
    pub fn code(self) -> i32 {
        match self {
            BlastError::OutOfInput => 2,
            BlastError::BadLiteralFlag => -1,
            BlastError::BadDictSize => -2,
            BlastError::DistanceTooFar => -3,
            BlastError::BadCode => -9,
        }
    }
}

struct Huffman {
    count: [u16; MAXBITS + 1],
    symbol: [u16; 256],
}

/// Build decoding tables from the compact (repeat, length) representation.
fn construct(rep: &[u8]) -> Huffman {
    let mut length = [0u8; 256];
    let mut n = 0usize;
    for &r in rep {
        let len = r & 15;
        for _ in 0..(r >> 4) + 1 {
            length[n] = len;
            n += 1;
        }
    }

    let mut h = Huffman { count: [0; MAXBITS + 1], symbol: [0; 256] };
    for &len in &length[..n] {
        h.count[len as usize] += 1;
    }

    let mut offs = [0u16; MAXBITS + 1];
    for len in 1..MAXBITS {
        offs[len + 1] = offs[len] + h.count[len];
    }
    for (symbol, &len) in length[..n].iter().enumerate() {
        if len != 0 {
            h.symbol[offs[len as usize] as usize] = symbol as u16;
            offs[len as usize] += 1;
        }
    }
    h
}

/// Bit lengths of literal codes.
const LITLEN: [u8; 98] = [
    11, 124, 8, 7, 28, 7, 188, 13, 76, 4, 10, 8, 12, 10, 12, 10, 8, 23, 8, 9, 7, 6, 7, 8, 7, 6,
    55, 8, 23, 24, 12, 11, 7, 9, 11, 12, 6, 7, 22, 5, 7, 24, 6, 11, 9, 6, 7, 22, 7, 11, 38, 7, 9,
    8, 25, 11, 8, 11, 9, 12, 8, 12, 5, 38, 5, 38, 5, 11, 7, 5, 6, 21, 6, 10, 53, 8, 7, 24, 10, 27,
    44, 253, 253, 253, 252, 252, 252, 13, 12, 45, 12, 45, 12, 61, 12, 45, 44, 173,
];
/// Bit lengths of length codes 0..15.
const LENLEN: [u8; 6] = [2, 35, 36, 53, 38, 23];
/// Bit lengths of distance codes 0..63.
const DISTLEN: [u8; 7] = [2, 20, 53, 230, 247, 151, 248];
/// Base for length codes.
const BASE: [u32; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
/// Extra bits for length codes.
const EXTRA: [u32; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

struct State<'a> {
    input: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl State<'_> {
    #[inline]
    fn next_byte(&mut self) -> Result<u32, BlastError> {
        let b = *self.input.get(self.pos).ok_or(BlastError::OutOfInput)?;
        self.pos += 1;
        Ok(b as u32)
    }

    /// Return `need` (0..=8) bits from the stream, least significant first.
    #[inline]
    fn bits(&mut self, need: u32) -> Result<u32, BlastError> {
        let mut val = self.bitbuf;
        while self.bitcnt < need {
            val |= self.next_byte()? << self.bitcnt;
            self.bitcnt += 8;
        }
        self.bitbuf = val >> need;
        self.bitcnt -= need;
        Ok(val & ((1 << need) - 1))
    }

    /// Decode one symbol.  Codes are stored bit-reversed and inverted; see
    /// the format notes in `blast.c`.
    #[inline]
    fn decode(&mut self, h: &Huffman) -> Result<usize, BlastError> {
        let mut bitbuf = self.bitbuf;
        let mut left = self.bitcnt;
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        let mut len: u32 = 1;
        loop {
            while left > 0 {
                left -= 1;
                code |= ((bitbuf & 1) ^ 1) as i32;
                bitbuf >>= 1;
                let count = h.count[len as usize] as i32;
                if code < first + count {
                    self.bitbuf = bitbuf;
                    self.bitcnt = self.bitcnt.wrapping_sub(len) & 7;
                    return Ok(h.symbol[(index + (code - first)) as usize] as usize);
                }
                index += count;
                first += count;
                first <<= 1;
                code <<= 1;
                len += 1;
            }
            left = (MAXBITS as u32 + 1) - len;
            if left == 0 {
                break;
            }
            bitbuf = self.next_byte()?;
            if left > 8 {
                left = 8;
            }
        }
        Err(BlastError::BadCode)
    }
}

/// Decompress a PKWare DCL stream from `input`, appending to `out`.
///
/// Input after the end code is ignored.  On error, `out` holds whatever was
/// decompressed so far.
pub fn blast(input: &[u8], out: &mut Vec<u8>) -> Result<(), BlastError> {
    let litcode = construct(&LITLEN);
    let lencode = construct(&LENLEN);
    let distcode = construct(&DISTLEN);

    let start = out.len();
    let mut s = State { input, pos: 0, bitbuf: 0, bitcnt: 0 };

    let lit = s.bits(8)?;
    if lit > 1 {
        return Err(BlastError::BadLiteralFlag);
    }
    let dict = s.bits(8)?;
    if !(4..=6).contains(&dict) {
        return Err(BlastError::BadDictSize);
    }

    loop {
        if s.bits(1)? != 0 {
            let symbol = s.decode(&lencode)?;
            let len = (BASE[symbol] + s.bits(EXTRA[symbol])?) as usize;
            if len == 519 {
                break; // end code
            }

            let shift = if len == 2 { 2 } else { dict };
            let mut dist = s.decode(&distcode)? << shift;
            dist += s.bits(shift)? as usize;
            dist += 1;

            let produced = out.len() - start;
            if dist > produced {
                return Err(BlastError::DistanceTooFar);
            }

            let from = out.len() - dist;
            if dist >= len {
                out.extend_from_within(from..from + len);
            } else {
                // Overlapped copy: repeats the last `dist` bytes.
                out.reserve(len);
                for i in 0..len {
                    let b = out[from + i];
                    out.push(b);
                }
            }
        } else {
            let symbol = if lit != 0 { s.decode(&litcode)? as u32 } else { s.bits(8)? };
            out.push(symbol as u8);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler_example() {
        // From the header comment of blast.c.
        let input = [0x00, 0x04, 0x82, 0x24, 0x25, 0x8f, 0x80, 0x7f];
        let mut out = Vec::new();
        blast(&input, &mut out).unwrap();
        assert_eq!(out, b"AIAIAIAIAIAIA");
    }

    #[test]
    fn errors() {
        let mut out = Vec::new();
        assert_eq!(blast(&[], &mut out), Err(BlastError::OutOfInput));
        assert_eq!(blast(&[2, 4], &mut out), Err(BlastError::BadLiteralFlag));
        assert_eq!(blast(&[0, 7], &mut out), Err(BlastError::BadDictSize));
        assert_eq!(blast(&[0x00, 0x04, 0x82], &mut out), Err(BlastError::OutOfInput));
        assert_eq!(BlastError::OutOfInput.code(), 2);
    }
}
