// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: libbgcode@d4da907361 src/LibBGCode/binarize/meatpack.cpp :: MPBinarizer, unbinarize
// Changes: Local lookup tables, checked decoding, byte buffers instead of C++ strings.
// Adaptation of MeatPack G-Code Compression by Scott Mudge (BSD-3-Clause):
// https://github.com/scottmudge/OctoPrint-MeatPack
// https://github.com/scottmudge/Prusa-Firmware-MeatPack
// Copyright (c) 2020, Scott Mudge. All rights reserved.
// The full BSD-3-Clause notice is retained in THIRD_PARTY.md.

//! Prusa's MeatPack dialect. This is intentionally a text normalization codec:
//! blank lines and inline comments disappear; G-line spaces/case are normalized.
//! Full comment lines survive only in comments mode. Decode restores G parameters'
//! spaces and collapses consecutive newlines, as libbgcode does.

use super::Error;

const TABLE: &[u8; 15] = b"0123456789. \nGX";

fn command(out: &mut Vec<u8>, cmd: u8) {
    out.extend_from_slice(&[255, 255, cmd]);
}

/// Encode a block, resetting the command state for each call. Both binary G-code
/// modes omit G-line spaces; `keep_comments` selects MeatPackComments.
pub fn encode(text: &str, keep_comments: bool) -> Vec<u8> {
    let mut out = Vec::new();
    command(&mut out, 251);
    command(&mut out, 247);
    let mut packing = true;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if keep_comments && trimmed.starts_with(';') {
            if packing {
                command(&mut out, 250);
                packing = false;
            }
            out.extend_from_slice(line.as_bytes());
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with([';', '\n', '\r']) || line.len() < 2 {
            continue;
        }
        let mut modified = line
            .split(';')
            .next()
            .unwrap()
            .trim_matches([' ', '\t'])
            .as_bytes()
            .to_vec();
        if modified.is_empty() {
            continue;
        }
        if let Some(g) = modified.iter().position(|&c| c == b'G')
            && modified.get(g + 1).is_some_and(u8::is_ascii_digit)
        {
            modified.retain(|&c| c != b' ');
            for c in &mut modified {
                if matches!(*c, b'e' | b'x' | b'g') {
                    c.make_ascii_uppercase();
                }
            }
            // Preserve upstream's unusual checksum rewrite: remove '*' but keep
            // the original checksum digits, then XOR the entire remaining line.
            if modified.contains(&b'*') {
                modified.retain(|&c| c != b'*');
                let checksum = modified.iter().fold(0u8, |a, &b| a ^ b);
                modified.extend_from_slice(format!("*{checksum}").as_bytes());
            }
            modified.push(b'\n');
        }
        if modified.last() != Some(&b'\n') {
            modified.push(b'\n');
        }
        if !packing {
            command(&mut out, 251);
            packing = true;
        }
        let nibble = |c| match c {
            b'E' => 11,
            b' ' => 15,
            _ => TABLE.iter().position(|&v| v == c).map_or(15, |n| n as u8),
        };
        for pair in modified.chunks(2) {
            let a = pair[0];
            let b = *pair.get(1).unwrap_or(&b'\n');
            let low = nibble(a);
            let high = nibble(b);
            out.push(low | (high << 4));
            if low == 15 {
                out.push(a);
            }
            if high == 15 {
                out.push(b);
            }
        }
    }
    if !keep_comments {
        command(&mut out, 249);
    }
    out
}

/// Decode packed and unpacked sections, command switches, and either space mode.
pub fn decode(input: &[u8]) -> Result<String, Error> {
    let mut output = Vec::new();
    let mut packing = false;
    let mut no_spaces = false;
    let mut gline = false;
    let mut i = 0;
    let mut emit = |c| {
        let new_gline = c == b'G' && output.last().is_none_or(|&last| last == b'\n');
        if new_gline {
            gline = true;
        }
        if c == b'\n' {
            gline = false;
        }
        if !new_gline && gline && b"XYZEFIJRSGPWHCA".contains(&c) && output.last() != Some(&b' ') {
            output.push(b' ');
        }
        if c != b'\n' || output.last() != Some(&b'\n') {
            output.push(c);
        }
    };
    while i < input.len() {
        if input[i..].starts_with(&[255, 255]) {
            match input
                .get(i + 2)
                .ok_or(Error::Invalid("truncated MeatPack command"))?
            {
                251 => packing = true,
                250 | 249 => packing = false,
                247 => no_spaces = true,
                246 => no_spaces = false,
                248 => {}
                _ => return Err(Error::Invalid("unknown MeatPack command")),
            }
            i += 3;
        } else if !packing {
            emit(input[i]);
            i += 1;
        } else {
            let packed = input[i];
            i += 1;
            for (index, nibble) in [packed & 15, packed >> 4].into_iter().enumerate() {
                let c = if nibble == 15 {
                    let c = *input
                        .get(i)
                        .ok_or(Error::Invalid("truncated MeatPack literal"))?;
                    i += 1;
                    c
                } else if nibble == 11 && no_spaces {
                    b'E'
                } else {
                    TABLE[nibble as usize]
                };
                emit(c);
                // A packed newline in the low nibble terminates the pair. A
                // full-width newline does not: this distinction is in upstream.
                if index == 0 && nibble == 12 {
                    break;
                }
            }
        }
    }
    String::from_utf8(output).map_err(|_| Error::Invalid("MeatPack text is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_normalization() {
        let text = "; layer café\n\nG1 X1.2 Y-3 E0.5 ; inline\nM104 S200\n; end\n";
        let expected = "; layer café\nG1 X1.2 Y-3 E0.5\nM104 S200\n; end\n";
        assert_eq!(decode(&encode(text, true)).unwrap(), expected);
        assert_eq!(
            decode(&encode(text, false)).unwrap(),
            "G1 X1.2 Y-3 E0.5\nM104 S200\n"
        );
        assert_eq!(decode(&encode("", true)).unwrap(), "");
        assert_eq!(decode(&encode("G1 X1\n", true)).unwrap(), "G1 X1\n");
        assert_eq!(decode(&encode("G1 X1", true)).unwrap(), "G1 X1\n");
    }

    #[test]
    fn wire_vectors_and_commands() {
        assert_eq!(
            encode("G1 X1\n", true),
            [255, 255, 251, 255, 255, 247, 0x1d, 0x1e, 0xcc]
        );
        assert_eq!(
            decode(&[255, 255, 251, 0x1d, 0xeb, 0xc1]).unwrap(),
            "G1 X1\n"
        );
        assert_eq!(
            decode(&[
                255, 255, 251, 255, 255, 247, 0x1d, 0x1b, 0xcc, 255, 255, 246, 0x1d, 0xeb, 0xc1
            ])
            .unwrap(),
            "G1 E1\nG1 X1\n"
        );
        assert!(decode(&[255, 255]).is_err());
        assert!(decode(&[255, 255, 251, 0xff]).is_err());
        assert!(decode(&[255, 255, 251, 0x1f]).is_err());
    }

    #[test]
    fn upstream_inline_comment_padding_is_not_invertible() {
        let with_comment = encode("G90 ; absolute coordinates\n", true);
        let plain = encode("G90\n", true);
        assert_eq!(&with_comment[6..], &[0x9d, 0xc0]);
        assert_eq!(&plain[6..], &[0x9d, 0xc0, 0xcc]);
        assert_eq!(decode(&with_comment).unwrap(), "G90\n");
        assert_eq!(decode(&plain).unwrap(), "G90\n");
    }
}
