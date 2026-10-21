// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: libbgcode@d4da907361 src/LibBGCode/core/core.cpp :: FileHeader, BlockHeader
// Source: libbgcode@d4da907361 src/LibBGCode/binarize/binarize.cpp :: unbinarize
// Changes: Bounded streaming block visitor using the existing Rust codecs; validates order and CRC before visiting.

use super::*;
use std::io::Read;

/// One verified, decompressed block. At most one block is retained by the reader.
pub enum Block {
    Metadata(BlockType, Metadata),
    Thumbnail(Thumbnail),
    Gcode(String),
}

/// Decode in file order, checking checksums and metadata order. Offsets in errors are byte offsets.
/// `limit` bounds both compressed and uncompressed block sizes, including decoded MeatPack.
pub fn visit<R: Read>(
    mut reader: R,
    limit: usize,
    mut visitor: impl FnMut(Block) -> Result<(), Error>,
) -> Result<Checksum, Error> {
    let mut header = [0; 10];
    reader.read_exact(&mut header)?;
    if &header[..4] != b"GCDE" {
        return Err(Error::Invalid("wrong magic"));
    }
    let mut cursor = Cursor {
        bytes: &header[4..],
        pos: 0,
    };
    let version = cursor.u32()?;
    if version != 1 {
        return Err(Error::Unsupported {
            field: "version",
            value: version,
        });
    }
    let checksum = Checksum::try_from(cursor.u16()?)?;
    let mut offset = 10;
    let mut stage = 0;
    let mut slicer_ini = false;
    loop {
        let mut base = [0; 8];
        if reader.read(&mut base[..1])? == 0 {
            break;
        }
        reader.read_exact(&mut base[1..])?;
        let mut c = Cursor {
            bytes: &base,
            pos: 0,
        };
        let kind = BlockType::try_from(c.u16()?)?;
        let compression = Compression::try_from(c.u16()?)?;
        let size = c.u32()? as usize;
        let mut raw = base.to_vec();
        let compressed = if compression != Compression::None {
            let mut bytes = [0; 4];
            reader.read_exact(&mut bytes)?;
            raw.extend(bytes);
            u32::from_le_bytes(bytes) as usize
        } else {
            size
        };
        if size > limit || compressed > limit {
            return Err(Error::LimitExceeded);
        }
        let mut params = vec![0; if kind == BlockType::Thumbnail { 6 } else { 2 }];
        reader.read_exact(&mut params)?;
        raw.extend(&params);
        let mut c = Cursor {
            bytes: &params,
            pos: 0,
        };
        let encoding = c.u16()?;
        let dimensions = if kind == BlockType::Thumbnail {
            (c.u16()?, c.u16()?)
        } else {
            (0, 0)
        };
        let mut payload = vec![0; compressed];
        reader.read_exact(&mut payload)?;
        let bytes_read =
            raw.len() + payload.len() + if checksum == Checksum::Crc32 { 4 } else { 0 };
        if checksum == Checksum::Crc32 {
            let mut bytes = [0; 4];
            reader.read_exact(&mut bytes)?;
            let expected = u32::from_le_bytes(bytes);
            let actual = crc32::update(crc32::update(0, &raw), &payload);
            if actual != expected {
                return Err(Error::Checksum {
                    offset,
                    expected,
                    actual,
                });
            }
        }
        let data = decompress(&payload, compression, size)?;
        let block = match kind {
            BlockType::FileMetadata if stage == 0 => {
                stage = 1;
                Block::Metadata(kind, Metadata::decode(encoding, data)?)
            }
            BlockType::PrinterMetadata if stage <= 1 => {
                stage = 2;
                Block::Metadata(kind, Metadata::decode(encoding, data)?)
            }
            BlockType::Thumbnail if stage == 2 => {
                if dimensions.0 == 0 || dimensions.1 == 0 || data.is_empty() {
                    return Err(Error::Invalid("empty thumbnail"));
                }
                Block::Thumbnail(Thumbnail {
                    format: ThumbnailFormat::try_from(encoding)?,
                    width: dimensions.0,
                    height: dimensions.1,
                    data,
                })
            }
            BlockType::PrintMetadata if stage == 2 => {
                stage = 3;
                Block::Metadata(kind, Metadata::decode(encoding, data)?)
            }
            BlockType::SlicerMetadata
                if stage == 3 || (stage == 4 && slicer_ini && encoding == 1) =>
            {
                slicer_ini = stage == 3 && encoding == 0;
                stage = 4;
                Block::Metadata(kind, Metadata::decode(encoding, data)?)
            }
            BlockType::Gcode if stage >= 4 => {
                stage = 5;
                let text = match GcodeEncoding::try_from(encoding)? {
                    GcodeEncoding::None => String::from_utf8(data)
                        .map_err(|_| Error::Invalid("G-code is not UTF-8"))?,
                    _ => meatpack::decode(&data)?,
                };
                if text.len() > limit {
                    return Err(Error::LimitExceeded);
                }
                Block::Gcode(text)
            }
            _ => return Err(Error::Invalid("invalid block order or missing metadata")),
        };
        visitor(block)?;
        offset += bytes_read;
    }
    if stage < 4 {
        return Err(Error::Invalid("missing required metadata blocks"));
    }
    Ok(checksum)
}
