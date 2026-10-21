// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Annex B framing (ITU-T H.264 Annex B): NAL units behind `00 00 01` / `00 00 00 01`
//! start codes, and the split of a byte stream into access units (section 7.4.1.2.3).

// SPS/PPS and `parts` serve the backends that need the parameter sets out of band
// (VideoToolbox); the others take Annex B as it comes.

/// NAL unit types the backends care about.
#[allow(dead_code)]
pub(crate) const SPS: u8 = 7;
#[allow(dead_code)]
pub(crate) const PPS: u8 = 8;
pub(crate) const AUD: u8 = 9;

pub(crate) fn kind(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1F)
}

/// Coded slice NAL units (non-IDR 1..=4, IDR 5).
pub(crate) fn is_vcl(nal: &[u8]) -> bool {
    (1..=5).contains(&kind(nal))
}

/// NAL units of Annex B data, without start codes (trailing zero bytes dropped).
pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let mut end = starts.get(k + 1).map_or(data.len(), |n| n - 3);
            // A four-byte start code (or trailing_zero_8bits) leaves zeros behind.
            while end > s && data[end - 1] == 0 {
                end -= 1;
            }
            &data[s..end]
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// Splits an Annex B byte stream into access units (each again Annex B, four-byte start
/// codes). A new unit starts at an access unit delimiter, at SPS/PPS/SEI after a slice, or
/// at a slice whose `first_mb_in_slice` is 0 after a slice.
pub fn access_units(stream: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = vec![];
    let mut cur = vec![];
    let mut has_vcl = false;
    for nal in nal_units(stream) {
        let k = kind(nal);
        let first_slice = is_vcl(nal) && nal.get(1).is_some_and(|b| b & 0x80 != 0);
        let starts = k == AUD || (has_vcl && (matches!(k, 6..=8) || first_slice));
        if starts && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            has_vcl = false;
        }
        has_vcl |= is_vcl(nal);
        cur.extend_from_slice(&[0, 0, 0, 1]);
        cur.extend_from_slice(nal);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Parameter sets and slices of one access unit, as the backends need them.
#[allow(dead_code)]
#[derive(Default)]
pub(crate) struct Parts<'a> {
    pub sps: Option<&'a [u8]>,
    pub pps: Option<&'a [u8]>,
    /// Coded slices, in order.
    pub slices: Vec<&'a [u8]>,
}

#[allow(dead_code)]
pub(crate) fn parts(au: &[u8]) -> Parts<'_> {
    let mut p = Parts::default();
    for nal in nal_units(au) {
        match kind(nal) {
            SPS => p.sps = Some(nal),
            PPS => p.pps = Some(nal),
            1..=5 => p.slices.push(nal),
            _ => {}
        }
    }
    p
}
