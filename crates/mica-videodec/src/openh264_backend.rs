// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Development only (feature `openh264`): OpenH264 compiled from source. Never in packaged
//! builds: Cisco's H.264 patent licence covers only Cisco's own binaries.

use crate::{Error, Frame};
use openh264::{OpenH264API, decoder::Decoder, formats::YUVSource};

pub const NAME: &str = "OpenH264 (development build)";

pub struct Backend {
    decoder: Decoder,
}

impl Backend {
    pub fn new() -> Result<Self, Error> {
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), Default::default())
            .map_err(|e| Error::Unavailable(format!("cannot start OpenH264: {e}")))?;
        Ok(Self { decoder })
    }

    pub fn parameters(&mut self, annex_b: &[u8]) -> Result<(), Error> {
        self.decode(annex_b, false).map(|_| ())
    }

    pub fn decode(&mut self, au: &[u8], convert: bool) -> Result<Option<Frame>, Error> {
        let yuv = self
            .decoder
            .decode(au)
            .map_err(|e| Error::Decode(e.to_string()))?;
        let Some(yuv) = yuv else { return Ok(None) };
        if !convert {
            return Ok(None);
        }
        let (w, h) = yuv.dimensions();
        if w == 0 || h == 0 || w > crate::MAX_SIDE as usize || h > crate::MAX_SIDE as usize {
            return Err(Error::UnsupportedSize {
                width: w as u32,
                height: h as u32,
            });
        }
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        Ok(Some(Frame {
            width: w as u32,
            height: h as u32,
            rgba,
        }))
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, Error> {
        Ok(vec![])
    }
}
