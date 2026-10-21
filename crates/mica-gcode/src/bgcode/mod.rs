// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: libbgcode@d4da907361 src/LibBGCode/core/core.cpp :: FileHeader, BlockHeader
// Source: libbgcode@d4da907361 src/LibBGCode/core/core.hpp :: wire enumerations
// Source: libbgcode@d4da907361 src/LibBGCode/binarize/binarize.hpp :: BinarizerConfig, BinaryData
// Source: libbgcode@d4da907361 src/LibBGCode/binarize/binarize.cpp :: Binarizer, encode_metadata, compress
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: s_binarizer_config
// Changes: Safe Rust streaming writer and checked in-memory reader; explicit errors and decode limits.
// Changes: Preserve ordered metadata, including JSON; cap encoded as well as source G-code blocks at 64 KiB.

//! Prusa binary G-code version 1, including all six block types and four compression modes.
//!
//! [`encode`] and [`decode`] are convenience functions; [`Encoder`] writes incrementally
//! to any [`Write`]. Supply metadata and thumbnail bytes separately from G-code text.
//! No metadata is inferred or removed from the input. Defaults match PrusaSlicer 2.9.6.
//!
//! MeatPack is **not lossless text storage**: it removes inline comments and blank
//! lines and normalizes G-line spacing/case. See [`meatpack`]. Select
//! [`GcodeEncoding::None`] when exact arbitrary input text preservation is required.
//! INI order, duplicate keys, empty values and embedded '=' values are preserved.
//! JSON metadata is retained as opaque UTF-8, without interpreting its schema.
//!
//! ```
//! use mica_gcode::bgcode::{encode, decode, BgcodeMetadata, BinarizerConfig, Metadata};
//! let metadata = BgcodeMetadata {
//!     printer: Metadata::Ini(vec![("printer_model".into(), "COREONE".into())]),
//!     print: Metadata::Ini(vec![("layer_height".into(), "0.2".into())]),
//!     slicer: vec![Metadata::Ini(vec![("Producer".into(), "Mica Slicer".into())])],
//!     ..BgcodeMetadata::default()
//! };
//! let bytes = encode("G1 X10 Y20\n", &metadata, &BinarizerConfig::default())?;
//! let decoded = decode(&bytes)?;
//! assert_eq!(decoded.gcode, "G1 X10 Y20\n");
//! assert_eq!(decoded.metadata, metadata);
//! # Ok::<(), mica_gcode::bgcode::Error>(())
//! ```

pub mod crc32;
pub mod heatshrink;
pub mod meatpack;
pub mod stream;

use std::fmt;
use std::io::{self, Write};
use std::ops::Range;

/// Maximum source text and encoded (pre-compression) G-code bytes per written block.
pub const MAX_GCODE_BLOCK_SIZE: usize = 65536;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Invalid(&'static str),
    Unsupported {
        field: &'static str,
        value: u32,
    },
    Checksum {
        offset: usize,
        expected: u32,
        actual: u32,
    },
    LimitExceeded,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "binary G-code I/O: {e}"),
            Self::Invalid(reason) => write!(f, "invalid binary G-code: {reason}"),
            Self::Unsupported { field, value } => write!(f, "unsupported {field}: {value}"),
            Self::Checksum {
                offset,
                expected,
                actual,
            } => write!(
                f,
                "CRC32 mismatch at block offset {offset}: stored {expected:08x}, calculated {actual:08x}"
            ),
            Self::LimitExceeded => f.write_str("binary G-code decode size limit exceeded"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

macro_rules! wire_enum {
    ($name:ident, $field:literal, {$($variant:ident = $value:literal),+ $(,)?}) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u16)]
        pub enum $name { $($variant = $value),+ }
        impl TryFrom<u16> for $name {
            type Error = Error;
            fn try_from(value: u16) -> Result<Self, Error> {
                match value { $($value => Ok(Self::$variant),)+
                    _ => Err(Error::Unsupported { field: $field, value: u32::from(value) }) }
            }
        }
    };
}

wire_enum!(BlockType, "block type", { FileMetadata = 0, Gcode = 1, SlicerMetadata = 2, PrinterMetadata = 3, PrintMetadata = 4, Thumbnail = 5 });
wire_enum!(Compression, "compression", { None = 0, Deflate = 1, Heatshrink11_4 = 2, Heatshrink12_4 = 3 });
wire_enum!(Checksum, "checksum", { None = 0, Crc32 = 1 });
wire_enum!(GcodeEncoding, "G-code encoding", { None = 0, MeatPack = 1, MeatPackComments = 2 });
wire_enum!(ThumbnailFormat, "thumbnail format", { Png = 0, Jpg = 1, Qoi = 2 });

/// Ordered INI key/value pairs, or the code-ahead-of-spec JSON metadata encoding (1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Metadata {
    Ini(Vec<(String, String)>),
    Json(String),
}

impl Default for Metadata {
    fn default() -> Self {
        Self::Ini(Vec::new())
    }
}

impl Metadata {
    fn encode(&self) -> Result<(u16, Vec<u8>), Error> {
        match self {
            Self::Json(text) => Ok((1, text.as_bytes().to_vec())),
            Self::Ini(entries) => {
                let mut data = Vec::new();
                for (key, value) in entries {
                    if key.contains(['=', '\n']) || value.contains('\n') {
                        return Err(Error::Invalid(
                            "INI key/value contains a delimiter; escape newlines first",
                        ));
                    }
                    data.extend_from_slice(key.as_bytes());
                    data.push(b'=');
                    data.extend_from_slice(value.as_bytes());
                    data.push(b'\n');
                }
                Ok((0, data))
            }
        }
    }

    fn decode(encoding: u16, data: Vec<u8>) -> Result<Self, Error> {
        let text = String::from_utf8(data).map_err(|_| Error::Invalid("metadata is not UTF-8"))?;
        match encoding {
            0 => {
                let entries = text
                    .split('\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        let (key, value) = line
                            .split_once('=')
                            .ok_or(Error::Invalid("INI entry has no '='"))?;
                        Ok((key.to_owned(), value.to_owned()))
                    })
                    .collect::<Result<_, Error>>()?;
                Ok(Self::Ini(entries))
            }
            1 => Ok(Self::Json(text)),
            value => Err(Error::Unsupported {
                field: "metadata encoding",
                value: value.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub format: ThumbnailFormat,
    pub width: u16,
    pub height: u16,
    /// Original PNG/JPG/QOI bytes; the codec does not render or transcode images.
    pub data: Vec<u8>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BgcodeMetadata {
    pub file: Option<Metadata>,
    pub printer: Metadata,
    pub thumbnails: Vec<Thumbnail>,
    pub print: Metadata,
    /// One INI block, one JSON block, or INI followed by JSON (newer libbgcode).
    pub slicer: Vec<Metadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinarizerConfig {
    pub file_compression: Compression,
    pub printer_compression: Compression,
    pub print_compression: Compression,
    pub slicer_compression: Compression,
    pub gcode_compression: Compression,
    pub gcode_encoding: GcodeEncoding,
    pub checksum: Checksum,
    /// Whole-line source text cache limit, in 1..=65536 bytes.
    pub gcode_block_size: usize,
}

impl Default for BinarizerConfig {
    fn default() -> Self {
        Self {
            file_compression: Compression::None,
            printer_compression: Compression::None,
            print_compression: Compression::Deflate,
            slicer_compression: Compression::Deflate,
            gcode_compression: Compression::Heatshrink12_4,
            gcode_encoding: GcodeEncoding::MeatPackComments,
            checksum: Checksum::Crc32,
            gcode_block_size: MAX_GCODE_BLOCK_SIZE,
        }
    }
}

fn compress(data: &[u8], compression: Compression) -> Result<Vec<u8>, Error> {
    // libbgcode writes zero bytes, even with a compression flag, for empty blocks.
    if data.is_empty() {
        return Ok(Vec::new());
    }
    match compression {
        Compression::None => Ok(data.to_vec()),
        Compression::Deflate => {
            // The format's "Deflate" is zlib-wrapped, not raw DEFLATE.
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data)?;
            Ok(encoder.finish()?)
        }
        Compression::Heatshrink11_4 => Ok(heatshrink::encode(data, heatshrink::Window::Bits11)),
        Compression::Heatshrink12_4 => Ok(heatshrink::encode(data, heatshrink::Window::Bits12)),
    }
}

fn decompress(data: &[u8], compression: Compression, size: usize) -> Result<Vec<u8>, Error> {
    if data.is_empty() && size == 0 {
        return Ok(Vec::new());
    }
    match compression {
        Compression::None => Ok(data.to_vec()),
        Compression::Deflate => {
            let mut decoder = flate2::Decompress::new(true);
            let mut out = Vec::with_capacity(size.checked_add(1).ok_or(Error::LimitExceeded)?);
            let status = decoder
                .decompress_vec(data, &mut out, flate2::FlushDecompress::Finish)
                .map_err(|_| Error::Invalid("invalid zlib stream"))?;
            if status != flate2::Status::StreamEnd
                || decoder.total_in() != data.len() as u64
                || out.len() != size
            {
                return Err(Error::Invalid(
                    "zlib length mismatch, truncation or trailing data",
                ));
            }
            Ok(out)
        }
        Compression::Heatshrink11_4 => heatshrink::decode(data, heatshrink::Window::Bits11, size),
        Compression::Heatshrink12_4 => heatshrink::decode(data, heatshrink::Window::Bits12, size),
    }
}

fn encode_gcode(text: &str, encoding: GcodeEncoding) -> Vec<u8> {
    match encoding {
        GcodeEncoding::None => text.as_bytes().to_vec(),
        GcodeEncoding::MeatPack => meatpack::encode(text, false),
        GcodeEncoding::MeatPackComments => meatpack::encode(text, true),
    }
}

/// Streaming writer. `append_gcode` accepts complete newline-terminated lines;
/// call `finish` to write the last block. After any I/O error, discard the writer.
pub struct Encoder<W: Write> {
    writer: W,
    config: BinarizerConfig,
    cache: String,
    in_config_block: bool,
}

impl<W: Write> Encoder<W> {
    pub fn new(
        writer: W,
        metadata: &BgcodeMetadata,
        config: &BinarizerConfig,
    ) -> Result<Self, Error> {
        if !(1..=MAX_GCODE_BLOCK_SIZE).contains(&config.gcode_block_size) {
            return Err(Error::Invalid("G-code block size must be in 1..=65536"));
        }
        if metadata.slicer.is_empty()
            || metadata.slicer.len() > 2
            || (metadata.slicer.len() == 2
                && !matches!(
                    (&metadata.slicer[0], &metadata.slicer[1]),
                    (Metadata::Ini(_), Metadata::Json(_))
                ))
        {
            return Err(Error::Invalid("expected INI and/or JSON slicer metadata"));
        }
        let mut this = Self {
            writer,
            config: config.clone(),
            cache: String::new(),
            in_config_block: false,
        };
        this.writer.write_all(b"GCDE")?;
        this.writer.write_all(&1u32.to_le_bytes())?;
        this.writer
            .write_all(&(config.checksum as u16).to_le_bytes())?;
        if let Some(file) = &metadata.file {
            this.metadata(BlockType::FileMetadata, file, config.file_compression)?;
        }
        this.metadata(
            BlockType::PrinterMetadata,
            &metadata.printer,
            config.printer_compression,
        )?;
        for thumbnail in &metadata.thumbnails {
            if thumbnail.width == 0 || thumbnail.height == 0 || thumbnail.data.is_empty() {
                return Err(Error::Invalid("empty thumbnail or zero dimensions"));
            }
            this.block(
                BlockType::Thumbnail,
                Compression::None,
                &[thumbnail.format as u16, thumbnail.width, thumbnail.height],
                &thumbnail.data,
            )?;
        }
        this.metadata(
            BlockType::PrintMetadata,
            &metadata.print,
            config.print_compression,
        )?;
        for slicer in &metadata.slicer {
            this.metadata(BlockType::SlicerMetadata, slicer, config.slicer_compression)?;
        }
        Ok(this)
    }

    fn metadata(
        &mut self,
        kind: BlockType,
        metadata: &Metadata,
        compression: Compression,
    ) -> Result<(), Error> {
        let (encoding, data) = metadata.encode()?;
        self.block(kind, compression, &[encoding], &data)
    }

    fn block(
        &mut self,
        kind: BlockType,
        compression: Compression,
        params: &[u16],
        data: &[u8],
    ) -> Result<(), Error> {
        let payload = compress(data, compression)?;
        let size =
            u32::try_from(data.len()).map_err(|_| Error::Invalid("block exceeds u32 size"))?;
        let compressed_size = u32::try_from(payload.len())
            .map_err(|_| Error::Invalid("compressed block exceeds u32 size"))?;
        let mut header = Vec::with_capacity(18);
        header.extend_from_slice(&(kind as u16).to_le_bytes());
        header.extend_from_slice(&(compression as u16).to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        if compression != Compression::None {
            header.extend_from_slice(&compressed_size.to_le_bytes());
        }
        for param in params {
            header.extend_from_slice(&param.to_le_bytes());
        }
        self.writer.write_all(&header)?;
        self.writer.write_all(&payload)?;
        if self.config.checksum == Checksum::Crc32 {
            let crc = crc32::update(crc32::update(0, &header), &payload);
            self.writer.write_all(&crc.to_le_bytes())?;
        }
        Ok(())
    }

    pub fn append_gcode(&mut self, text: &str) -> Result<(), Error> {
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(Error::Invalid("G-code must end with a newline"));
        }
        for line in text.split_inclusive('\n') {
            // Full settings already live in slicer metadata, never executable blocks.
            // Keep this in the shared encoder so GUI and CLI packing agree.
            let marker = line.trim_end();
            if matches!(
                marker,
                "; prusaslicer_config = begin" | "; CONFIG_BLOCK_START"
            ) {
                self.in_config_block = true;
                continue;
            }
            if matches!(marker, "; prusaslicer_config = end" | "; CONFIG_BLOCK_END") {
                self.in_config_block = false;
                continue;
            }
            if self.in_config_block {
                continue;
            }

            if line.len() > self.config.gcode_block_size {
                return Err(Error::Invalid("G-code line exceeds block size"));
            }
            if self.cache.len() + line.len() > self.config.gcode_block_size {
                self.flush_cache()?;
            }
            self.cache.push_str(line);
        }
        Ok(())
    }

    fn flush_cache(&mut self) -> Result<(), Error> {
        if self.cache.is_empty() {
            return Ok(());
        }
        let text = std::mem::take(&mut self.cache);
        self.gcode_block(&text)
    }

    fn gcode_block(&mut self, text: &str) -> Result<(), Error> {
        let data = encode_gcode(text, self.config.gcode_encoding);
        if data.len() > MAX_GCODE_BLOCK_SIZE {
            // Non-G lines with mostly unpackable characters can expand in MeatPack.
            // Unlike upstream, enforce the wire-size limit as well as the text cap.
            let split = text.as_bytes()[..text.len() / 2]
                .iter()
                .rposition(|&c| c == b'\n')
                .or_else(|| text.find('\n').filter(|&i| i + 1 < text.len()))
                .ok_or(Error::Invalid("encoded G-code line exceeds 64 KiB"))?
                + 1;
            self.gcode_block(&text[..split])?;
            return self.gcode_block(&text[split..]);
        }
        self.block(
            BlockType::Gcode,
            self.config.gcode_compression,
            &[self.config.gcode_encoding as u16],
            &data,
        )
    }

    /// Flush the final G-code block and return the underlying writer.
    pub fn finish(mut self) -> Result<W, Error> {
        self.flush_cache()?;
        Ok(self.writer)
    }
}

/// Encode complete newline-terminated G-code, with separately supplied metadata.
pub fn encode(
    gcode: &str,
    metadata: &BgcodeMetadata,
    config: &BinarizerConfig,
) -> Result<Vec<u8>, Error> {
    let mut encoder = Encoder::new(Vec::new(), metadata, config)?;
    encoder.append_gcode(gcode)?;
    encoder.finish()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    pub kind: BlockType,
    pub compression: Compression,
    /// Encoded size before compression (MeatPack bytes for packed G-code).
    pub uncompressed_size: u32,
    /// None when the header has no compressed-size field.
    pub compressed_size: Option<u32>,
    /// Metadata/G-code encoding, or thumbnail format.
    pub encoding: u16,
    pub offset: usize,
    /// Original file's compressed payload, excluding parameters and checksum.
    pub payload_range: Range<usize>,
    /// Range in [`DecodedBgcode::gcode`] for G-code blocks.
    pub gcode_range: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBgcode {
    pub checksum: Checksum,
    pub metadata: BgcodeMetadata,
    pub gcode: String,
    pub blocks: Vec<BlockInfo>,
}

/// Resource limits for the in-memory decoder. Increase for unusually large files.
#[derive(Debug, Clone, Copy)]
pub struct DecodeLimits {
    pub max_block_bytes: usize,
    /// Budget for decompressed payloads plus decoded G-code bytes.
    pub max_total_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_block_bytes: 64 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024,
        }
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(Error::Invalid("block size overflow"))?;
        let data = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error::Invalid("truncated file"))?;
        self.pos = end;
        Ok(data)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// Verify all block CRCs and decode metadata, thumbnails and G-code. The file
/// header itself has no checksum in this format. Checksum-free files are supported
/// and explicitly identified by the returned [`Checksum::None`].
pub fn decode(bytes: &[u8]) -> Result<DecodedBgcode, Error> {
    decode_with_limits(bytes, DecodeLimits::default())
}

pub fn decode_with_limits(bytes: &[u8], limits: DecodeLimits) -> Result<DecodedBgcode, Error> {
    let mut cursor = Cursor { bytes, pos: 0 };
    if cursor.take(4)? != b"GCDE" {
        return Err(Error::Invalid("wrong magic"));
    }
    let version = cursor.u32()?;
    if version != 1 {
        return Err(Error::Unsupported {
            field: "version",
            value: version,
        });
    }
    let checksum = Checksum::try_from(cursor.u16()?)?;
    let mut result = DecodedBgcode {
        checksum,
        metadata: BgcodeMetadata::default(),
        gcode: String::new(),
        blocks: Vec::new(),
    };
    let mut stage = 0;
    let mut budget = limits.max_total_bytes;
    while cursor.pos < bytes.len() {
        let offset = cursor.pos;
        let kind = BlockType::try_from(cursor.u16()?)?;
        let compression = Compression::try_from(cursor.u16()?)?;
        let uncompressed_size = cursor.u32()?;
        let compressed_size = if compression == Compression::None {
            None
        } else {
            Some(cursor.u32()?)
        };
        let encoding = cursor.u16()?;
        let dimensions = if kind == BlockType::Thumbnail {
            Some((cursor.u16()?, cursor.u16()?))
        } else {
            None
        };
        let start = cursor.pos;
        let payload = cursor.take(compressed_size.unwrap_or(uncompressed_size) as usize)?;
        let end = cursor.pos;
        if checksum == Checksum::Crc32 {
            let expected = cursor.u32()?;
            let actual = crc32::update(0, &bytes[offset..end]);
            if expected != actual {
                return Err(Error::Checksum {
                    offset,
                    expected,
                    actual,
                });
            }
        }
        let size = uncompressed_size as usize;
        if size > limits.max_block_bytes {
            return Err(Error::LimitExceeded);
        }
        budget = budget.checked_sub(size).ok_or(Error::LimitExceeded)?;
        let data = decompress(payload, compression, size)?;
        let mut gcode_range = None;
        match kind {
            BlockType::FileMetadata if stage == 0 => {
                result.metadata.file = Some(Metadata::decode(encoding, data)?);
                stage = 1;
            }
            BlockType::PrinterMetadata if stage <= 1 => {
                result.metadata.printer = Metadata::decode(encoding, data)?;
                stage = 2;
            }
            BlockType::Thumbnail if stage == 2 => {
                let (width, height) = dimensions.unwrap();
                if width == 0 || height == 0 || data.is_empty() {
                    return Err(Error::Invalid("empty thumbnail or zero dimensions"));
                }
                result.metadata.thumbnails.push(Thumbnail {
                    format: ThumbnailFormat::try_from(encoding)?,
                    width,
                    height,
                    data,
                });
            }
            BlockType::PrintMetadata if stage == 2 => {
                result.metadata.print = Metadata::decode(encoding, data)?;
                stage = 3;
            }
            BlockType::SlicerMetadata if stage == 3 || stage == 4 => {
                let metadata = Metadata::decode(encoding, data)?;
                if stage == 4
                    && !(result.metadata.slicer.len() == 1
                        && matches!(
                            (&result.metadata.slicer[0], &metadata),
                            (Metadata::Ini(_), Metadata::Json(_))
                        ))
                {
                    return Err(Error::Invalid("duplicate or misordered slicer metadata"));
                }
                result.metadata.slicer.push(metadata);
                stage = 4;
            }
            BlockType::Gcode if stage >= 4 => {
                let text = match GcodeEncoding::try_from(encoding)? {
                    GcodeEncoding::None => String::from_utf8(data)
                        .map_err(|_| Error::Invalid("G-code is not UTF-8"))?,
                    GcodeEncoding::MeatPack | GcodeEncoding::MeatPackComments => {
                        meatpack::decode(&data)?
                    }
                };
                budget = budget.checked_sub(text.len()).ok_or(Error::LimitExceeded)?;
                let start = result.gcode.len();
                result.gcode.push_str(&text);
                gcode_range = Some(start..result.gcode.len());
                stage = 5;
            }
            _ => return Err(Error::Invalid("invalid block order or missing metadata")),
        }
        result.blocks.push(BlockInfo {
            kind,
            compression,
            uncompressed_size,
            compressed_size,
            encoding,
            offset,
            payload_range: start..end,
            gcode_range,
        });
    }
    if stage < 4 {
        return Err(Error::Invalid("missing required metadata blocks"));
    }
    Ok(result)
}
