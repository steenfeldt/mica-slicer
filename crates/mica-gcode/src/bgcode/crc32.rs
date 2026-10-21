// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: libbgcode@d4da907361 src/LibBGCode/core/core_impl.hpp :: crc32_sw
// Changes: Safe Rust byte slices; incremental CRC exposed as a small function.

/// Update an IEEE CRC32 (initial value zero, reflected polynomial 0xedb88320).
pub fn update(crc: u32, bytes: &[u8]) -> u32 {
    let mut value = !crc;
    for &byte in bytes {
        value ^= u32::from(byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(value & 1)));
        }
    }
    !value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors_and_incremental() {
        assert_eq!(update(0, b""), 0);
        assert_eq!(update(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(update(update(0, b"1234"), b"56789"), 0xcbf4_3926);
        assert_eq!(
            update(0, b"The quick brown fox jumps over the lazy dog"),
            0x414f_a339
        );
    }
}
