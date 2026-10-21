// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! H.264 decoding with the decoder the operating system provides, so Mica itself ships no
//! H.264 decoder code: VideoToolbox on macOS (hardware), the Media Foundation H.264 decoder
//! on Windows, GStreamer (whatever H.264 decoder plugin the distribution installs) on Linux
//! and the other Unix systems. The codec licence comes with the operating system or the
//! distribution's plugins.
//!
//! The development-only cargo feature `openh264` swaps in OpenH264 compiled from source
//! instead (never in packaged builds).
//!
//! This is the one crate of the workspace that may use `unsafe` (the macOS and Windows
//! backends call the system frameworks). Each unsafe block says why it is sound; everything
//! outside the backend modules is safe code, and so is the API.

#![deny(unsafe_code)]

mod annexb;
mod convert;

#[cfg(feature = "openh264")]
mod openh264_backend;
#[cfg(feature = "openh264")]
use openh264_backend as backend;

#[cfg(all(not(feature = "openh264"), target_os = "macos"))]
#[allow(unsafe_code)]
mod apple;
#[cfg(all(not(feature = "openh264"), target_os = "macos"))]
use apple as backend;

#[cfg(all(not(feature = "openh264"), windows))]
#[allow(unsafe_code)]
mod mediafoundation;
#[cfg(all(not(feature = "openh264"), windows))]
use mediafoundation as backend;

#[cfg(all(not(feature = "openh264"), unix, not(target_os = "macos")))]
mod gst;
#[cfg(all(not(feature = "openh264"), unix, not(target_os = "macos")))]
use gst as backend;

pub use annexb::{access_units, nal_units};

/// Largest picture accepted in either direction (decoder memory).
pub const MAX_SIDE: u32 = 8192;

/// The video codec of a stream. Only H.264 for now (what printer cameras send over RTSP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
}

/// A decoded picture: `width * height` RGBA pixels, rows top to bottom, no padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Why decoding failed. `Display` gives a sentence for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The operating system has no usable H.264 decoder (e.g. GStreamer without an H.264
    /// plugin, a Windows "N" edition without the Media Feature Pack).
    Unavailable(String),
    /// The decoder refused to start for this stream (unsupported profile or size, broken
    /// parameter sets).
    Create(String),
    /// One access unit could not be decoded (lost packets, corrupt data). The decoder stays
    /// usable; the next key frame normally recovers.
    Decode(String),
    /// The picture is larger than [`MAX_SIDE`] or empty.
    UnsupportedSize { width: u32, height: u32 },
}

impl Error {
    /// The decoder cannot work at all for this stream: retrying frame by frame is pointless.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Error::Unavailable(_) | Error::Create(_))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unavailable(why) => write!(f, "{why}"),
            Error::Create(why) => write!(f, "cannot start the H.264 decoder: {why}"),
            Error::Decode(why) => write!(f, "the camera sent H.264 Mica cannot decode ({why})"),
            Error::UnsupportedSize { width, height } => {
                write!(
                    f,
                    "the camera's video has an unsupported size ({width} × {height})"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// Name of the decoder in use, for logs and the About box.
pub const BACKEND: &str = backend::NAME;

/// An H.264 decoder. Feed it Annex B access units (start codes, one picture each, parameter
/// sets in band or given up front). Use it on the thread that made it.
pub struct Decoder {
    inner: backend::Backend,
    /// Not `Send`: Media Foundation objects belong to the thread that initialised COM.
    _thread: std::marker::PhantomData<*const ()>,
}

impl Decoder {
    /// A decoder for `codec`. `params`: SPS and PPS as Annex B (start codes), when the
    /// stream description has them; `None` when they arrive in band with the key frames.
    /// With parameters the operating system decoder starts right away, so a stream it
    /// cannot handle fails here.
    pub fn new(codec: Codec, params: Option<&[u8]>) -> Result<Self, Error> {
        let Codec::H264 = codec;
        let mut inner = backend::Backend::new()?;
        if let Some(p) = params {
            inner.parameters(p)?;
        }
        Ok(Self {
            inner,
            _thread: std::marker::PhantomData,
        })
    }

    /// Decodes one access unit and returns the newest picture it produced, as RGBA.
    /// `Ok(None)`: no picture yet (parameter sets only, waiting for a key frame, or a
    /// decoder that holds frames back).
    pub fn decode(&mut self, access_unit: &[u8]) -> Result<Option<Frame>, Error> {
        let f = self.inner.decode(access_unit, true)?;
        f.map(checked).transpose()
    }

    /// Decodes one access unit without converting the picture (H.264 frames depend on each
    /// other, so every one must be decoded even when nobody looks at it).
    pub fn decode_only(&mut self, access_unit: &[u8]) -> Result<(), Error> {
        self.inner.decode(access_unit, false).map(|_| ())
    }

    /// Ends the stream: returns the pictures the decoder still held back, oldest first.
    pub fn flush(&mut self) -> Result<Vec<Frame>, Error> {
        self.inner.flush()?.into_iter().map(checked).collect()
    }
}

fn checked(f: Frame) -> Result<Frame, Error> {
    let (w, h) = (f.width, f.height);
    if w == 0
        || h == 0
        || w > MAX_SIDE
        || h > MAX_SIDE
        || f.rgba.len() != w as usize * h as usize * 4
    {
        return Err(Error::UnsupportedSize {
            width: w,
            height: h,
        });
    }
    Ok(f)
}

#[cfg(test)]
mod tests;
