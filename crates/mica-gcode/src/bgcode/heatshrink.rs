// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: libbgcode@d4da907361 src/LibBGCode/binarize/binarize.cpp :: compress, uncompress
// Changes: Written from the published heatshrink algorithm (ISC), not a vendored C source.
// Changes: Whole-block Rust codec, nearest-match indexing and strict size/padding validation.
// Heatshrink copyright (c) 2013-2015, Scott Vokes <vokes.s@gmail.com>.
// The ISC permission and warranty notice is retained in THIRD_PARTY.md.

//! Heatshrink v0.4.1 bitstream, with the two window sizes used by binary G-code.

use super::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Bits11,
    Bits12,
}

impl Window {
    fn bits(self) -> usize {
        match self {
            Self::Bits11 => 11,
            Self::Bits12 => 12,
        }
    }
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: usize,
}

impl Bits {
    fn push(&mut self, value: usize, count: usize) {
        for shift in (0..count).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            *self.bytes.last_mut().unwrap() |=
                (((value >> shift) & 1) as u8) << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

/// Encode with a 16-byte lookahead, zero-filled initial window, and MSB-first bits.
pub fn encode(input: &[u8], window: Window) -> Vec<u8> {
    let window_bits = window.bits();
    let window_size = 1 << window_bits;
    // The C encoder indexes positions by byte, visiting newest candidates first.
    // Equal-length ties therefore select the shortest distance. Include the initial
    // zero-filled history, and permit overlapping references (e.g. runs of one byte).
    let mut data = vec![0; window_size];
    data.extend_from_slice(input);
    let mut previous = vec![usize::MAX; data.len()];
    let mut heads = [usize::MAX; 256];
    for (i, &byte) in data.iter().enumerate() {
        previous[i] = heads[byte as usize];
        heads[byte as usize] = i;
    }
    let mut output = Bits::default();
    let mut pos = window_size;
    while pos < data.len() {
        let max_len = 16.min(data.len() - pos);
        let mut candidate = previous[pos];
        let mut best_len = 0;
        let mut best_distance = 0;
        while candidate != usize::MAX && candidate >= pos - window_size {
            let mut len = 1;
            while len < max_len && data[candidate + len] == data[pos + len] {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                best_distance = pos - candidate;
                if len == max_len {
                    break;
                }
            }
            candidate = previous[candidate];
        }
        // heatshrink's break-even test counts whole bytes, not literal tag bits.
        if best_len > (1 + window_bits + 4) / 8 {
            output.push(0, 1);
            output.push(best_distance - 1, window_bits);
            output.push(best_len - 1, 4);
            pos += best_len;
        } else {
            output.push(1, 1);
            output.push(data[pos] as usize, 8);
            pos += 1;
        }
    }
    output.bytes
}

/// Decode exactly `expected_size` bytes. Reject truncated tokens, size mismatches,
/// and anything beyond the final byte's zero padding.
pub fn decode(input: &[u8], window: Window, expected_size: usize) -> Result<Vec<u8>, Error> {
    let mut bit = 0;
    let mut read = |count: usize| -> Result<usize, Error> {
        if count > input.len().saturating_mul(8).saturating_sub(bit) {
            return Err(Error::Invalid("truncated Heatshrink token"));
        }
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | usize::from((input[bit / 8] >> (7 - bit % 8)) & 1);
            bit += 1;
        }
        Ok(value)
    };
    // Do not allocate from an untrusted size field before decoding actual tokens.
    let mut output = Vec::new();
    while output.len() < expected_size {
        if read(1)? == 1 {
            output.push(read(8)? as u8);
        } else {
            let distance = read(window.bits())? + 1;
            let len = read(4)? + 1;
            if len > expected_size - output.len() {
                return Err(Error::Invalid("Heatshrink output exceeds declared size"));
            }
            for _ in 0..len {
                let byte = if distance > output.len() {
                    0
                } else {
                    output[output.len() - distance]
                };
                output.push(byte);
            }
        }
    }
    if input.len().saturating_mul(8) - bit > 7
        || (bit..input.len() * 8).any(|i| (input[i / 8] >> (7 - i % 8)) & 1 != 0)
    {
        return Err(Error::Invalid(
            "invalid Heatshrink padding or trailing data",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_and_round_trips() {
        assert_eq!(encode(b"A", Window::Bits12), [0xa0, 0x80]);
        assert_eq!(encode(&[0; 16], Window::Bits12), [0, 7, 0x80]);
        let mut random = 123456789u32;
        let noise: Vec<u8> = (0..10000)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                random as u8
            })
            .collect();
        for window in [Window::Bits11, Window::Bits12] {
            for input in [
                b"".as_slice(),
                b"a",
                b"abcabcabcabcabcabc",
                &[0; 10000],
                &[b'a'; 10000],
                &noise,
            ] {
                let packed = encode(input, window);
                assert_eq!(decode(&packed, window, input.len()).unwrap(), input);
            }
            assert!(decode(&[0x80], window, 1).is_err());
            assert!(decode(&[0xa0, 0x81], window, 1).is_err());
            assert!(decode(&[0xa0, 0x80, 0], window, 1).is_err());
        }
    }
}
