// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! macOS: VideoToolbox (`VTDecompressionSession`, hardware decoding where the Mac has it).
//! Access units arrive as Annex B; VideoToolbox wants a format description built from the
//! SPS/PPS and the slices with four-byte length prefixes (AVCC). Decoding is synchronous
//! (no asynchronous or temporal-processing flags), so the output handler has run when
//! `VTDecompressionSessionDecodeFrame…` returns; VideoToolbox converts to 32-bit BGRA itself.
//!
//! Unsafe blocks: format description creation, session creation, block buffer creation and
//! fill, sample buffer creation, the decode call, and reading the pixel buffer in the output
//! handler (lock, base address, unlock). Teardown (wait, invalidate) goes through the same
//! safe wrappers' unsafe calls in `Drop`.

use crate::{Error, Frame, annexb, convert};
use block2::RcBlock;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMTime, CMVideoFormatDescription,
    CMVideoFormatDescriptionCreateFromH264ParameterSets, kCMBlockBufferAssureMemoryNowFlag,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA, kCVReturnSuccess,
};
use objc2_video_toolbox::{VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionSession};
use std::{
    collections::VecDeque,
    ptr::{self, NonNull},
    sync::{Arc, Mutex},
};

pub const NAME: &str = "VideoToolbox";

type OSStatus = i32;

/// What the output handler produced since the last look.
#[derive(Default)]
struct Output {
    convert: bool,
    frames: VecDeque<Result<Frame, Error>>,
}

struct Session {
    session: CFRetained<VTDecompressionSession>,
    format: CFRetained<CMFormatDescription>,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: `session` is a live session this struct owns; waiting for (synchronous,
        // so already finished) frames and invalidating it are the documented teardown, and
        // nothing uses it afterwards.
        unsafe {
            self.session.wait_for_asynchronous_frames();
            self.session.invalidate();
        }
    }
}

pub struct Backend {
    session: Option<Session>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    out: Arc<Mutex<Output>>,
    /// Slices seen without parameter sets to start a session.
    orphans: u32,
}

fn describe(status: OSStatus) -> String {
    let what = match status {
        -12906 => "no H.264 decoder for this stream on this Mac",
        -12909 => "bad data",
        -12910 => "unsupported data format",
        -12911 => "decoder malfunction",
        -12903 => "the decoder session was reset",
        -12710..=-12700 => "the parameter sets are invalid",
        _ => "VideoToolbox error",
    };
    format!("{what}, status {status}")
}

impl Backend {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            session: None,
            sps: None,
            pps: None,
            out: Arc::default(),
            orphans: 0,
        })
    }

    pub fn parameters(&mut self, annex_b: &[u8]) -> Result<(), Error> {
        let p = annexb::parts(annex_b);
        if let Some(s) = p.sps {
            self.sps = Some(s.to_vec());
        }
        if let Some(s) = p.pps {
            self.pps = Some(s.to_vec());
        }
        if self.sps.is_none() || self.pps.is_none() {
            return Err(Error::Create(
                "the stream description has no SPS and PPS".into(),
            ));
        }
        self.ensure_session()
    }

    /// (Re)starts the session when the parameter sets are known and differ from the
    /// running session's.
    fn ensure_session(&mut self) -> Result<(), Error> {
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return Ok(());
        };
        if self
            .session
            .as_ref()
            .is_some_and(|s| &s.sps == sps && &s.pps == pps)
        {
            return Ok(());
        }
        self.session = None;
        let format = format_description(sps, pps)?;
        let session = create_session(&format)?;
        self.session = Some(Session {
            session,
            format,
            sps: sps.clone(),
            pps: pps.clone(),
        });
        Ok(())
    }

    pub fn decode(&mut self, au: &[u8], convert: bool) -> Result<Option<Frame>, Error> {
        let parts = annexb::parts(au);
        if let Some(s) = parts.sps {
            self.sps = Some(s.to_vec());
        }
        if let Some(s) = parts.pps {
            self.pps = Some(s.to_vec());
        }
        if parts.slices.is_empty() {
            return Ok(None);
        }
        self.ensure_session()?;
        let Some(session) = &self.session else {
            self.orphans += 1;
            if self.orphans > 300 {
                return Err(Error::Create(
                    "the stream never sent its parameter sets (SPS/PPS)".into(),
                ));
            }
            return Ok(None);
        };
        let mut avcc = Vec::with_capacity(au.len() + 4 * parts.slices.len());
        for nal in &parts.slices {
            avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            avcc.extend_from_slice(nal);
        }
        let sample = sample_buffer(&avcc, &session.format)?;
        self.out.lock().unwrap().convert = convert;
        let out = self.out.clone();
        let handler = RcBlock::new(
            move |status: OSStatus,
                  _flags: VTDecodeInfoFlags,
                  image: *mut CVImageBuffer,
                  _pts: CMTime,
                  _duration: CMTime| {
                let mut o = out.lock().unwrap();
                let result = if status != 0 {
                    Some(Err(Error::Decode(describe(status))))
                } else if let Some(image) = NonNull::new(image) {
                    // SAFETY: VideoToolbox passes a valid image buffer, retained for the
                    // duration of this call, when the status is success.
                    let image = unsafe { image.as_ref() };
                    o.convert.then(|| to_rgba(image))
                } else {
                    None // dropped frame
                };
                if let Some(r) = result {
                    o.frames.push_back(r);
                }
            },
        );
        // SAFETY: `session` and `sample` are live CF objects. The handler is a heap block
        // that VideoToolbox copies (retains) for as long as it needs it; it only captures an
        // `Arc` and checks its raw-pointer argument. `info_flags_out` may be null.
        let status = unsafe {
            session.session.decode_frame_with_output_handler(
                &sample,
                VTDecodeFrameFlags(0),
                ptr::null_mut(),
                RcBlock::as_ptr(&handler),
            )
        };
        if status != 0 {
            if status == -12903 {
                // Invalid session: start a new one with the next access unit.
                self.session = None;
            }
            return Err(Error::Decode(describe(status)));
        }
        self.take()
    }

    /// The newest frame produced, or the first error when there is no frame.
    fn take(&mut self) -> Result<Option<Frame>, Error> {
        let mut o = self.out.lock().unwrap();
        let mut newest = None;
        let mut error = None;
        for r in o.frames.drain(..) {
            match r {
                Ok(f) => newest = Some(f),
                Err(e) => error = error.or(Some(e)),
            }
        }
        match (newest, error) {
            (Some(f), _) => Ok(Some(f)),
            (None, Some(e)) => Err(e),
            (None, None) => Ok(None),
        }
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, Error> {
        if let Some(s) = &self.session {
            self.out.lock().unwrap().convert = true;
            // SAFETY: a live session; both calls only take the session.
            unsafe {
                s.session.finish_delayed_frames();
                s.session.wait_for_asynchronous_frames();
            }
        }
        self.out
            .lock()
            .unwrap()
            .frames
            .drain(..)
            .filter_map(Result::ok)
            .map(Ok)
            .collect()
    }
}

fn format_description(
    sps: &[u8],
    pps: &[u8],
) -> Result<CFRetained<CMVideoFormatDescription>, Error> {
    if sps.len() < 4 || pps.is_empty() {
        return Err(Error::Create("the parameter sets are truncated".into()));
    }
    let pointers = [NonNull::from(&sps[0]), NonNull::from(&pps[0])];
    let mut sizes = [sps.len(), pps.len()];
    let mut out: *const CMFormatDescription = ptr::null();
    // SAFETY: two parameter sets, each pointer valid for its size for the whole call (the
    // function copies them); `out` is a valid place for the result.
    let status = unsafe {
        CMVideoFormatDescriptionCreateFromH264ParameterSets(
            None,
            2,
            NonNull::from(&pointers).cast(),
            NonNull::from(&mut sizes).cast(),
            4,
            NonNull::from(&mut out),
        )
    };
    match NonNull::new(out.cast_mut()) {
        // SAFETY: a "Create" function: the result carries +1 retain count that we own.
        Some(p) if status == 0 => Ok(unsafe { CFRetained::from_raw(p) }),
        _ => Err(Error::Create(describe(status))),
    }
}

fn create_session(
    format: &CMVideoFormatDescription,
) -> Result<CFRetained<VTDecompressionSession>, Error> {
    // SAFETY: an immutable CF constant provided by CoreVideo.
    let key: &CFString = unsafe { kCVPixelBufferPixelFormatTypeKey };
    let value = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
    let attributes = CFDictionary::<CFString, CFNumber>::from_slices(&[key], &[&value]);
    let mut out: *mut VTDecompressionSession = ptr::null_mut();
    // SAFETY: the format description and attribute dictionary (CFString → CFNumber, as
    // VideoToolbox expects) are live; a null callback record is allowed because every
    // frame is decoded with an output handler; `out` is a valid place for the result.
    let status = unsafe {
        VTDecompressionSession::create(
            None,
            format,
            None,
            Some(attributes.as_opaque()),
            ptr::null(),
            NonNull::from(&mut out),
        )
    };
    match NonNull::new(out) {
        // SAFETY: a "Create" function: the result carries +1 retain count that we own.
        Some(p) if status == 0 => Ok(unsafe { CFRetained::from_raw(p) }),
        _ => Err(Error::Create(describe(status))),
    }
}

fn sample_buffer(
    avcc: &[u8],
    format: &CMFormatDescription,
) -> Result<CFRetained<CMSampleBuffer>, Error> {
    let fail = |what: &str, status| Error::Decode(format!("{what} ({status})"));
    let mut block: *mut CMBlockBuffer = ptr::null_mut();
    // SAFETY: a null memory block with the default allocator makes CoreMedia allocate
    // `avcc.len()` bytes itself (allocated now: kCMBlockBufferAssureMemoryNowFlag); no custom
    // block source; `block` is a valid place for the result.
    let status = unsafe {
        CMBlockBuffer::create_with_memory_block(
            None,
            ptr::null_mut(),
            avcc.len(),
            None,
            ptr::null(),
            0,
            avcc.len(),
            kCMBlockBufferAssureMemoryNowFlag,
            NonNull::from(&mut block),
        )
    };
    let block = match NonNull::new(block) {
        // SAFETY: a "Create" function: +1 retain count that we own.
        Some(p) if status == 0 => unsafe { CFRetained::from_raw(p) },
        _ => return Err(fail("cannot allocate a sample", status)),
    };
    // SAFETY: copies `avcc.len()` readable bytes into the block, which holds exactly that
    // many bytes from offset 0.
    let status = unsafe {
        CMBlockBuffer::replace_data_bytes(NonNull::from(avcc).cast(), &block, 0, avcc.len())
    };
    if status != 0 {
        return Err(fail("cannot fill a sample", status));
    }
    let sizes = [avcc.len()];
    let mut sample: *mut CMSampleBuffer = ptr::null_mut();
    // SAFETY: live block buffer and format description; one sample whose size is in
    // `sizes` (valid for the call); no timing entries (null with count 0 is allowed);
    // `sample` is a valid place for the result.
    let status = unsafe {
        CMSampleBuffer::create_ready(
            None,
            Some(&block),
            Some(format),
            1,
            0,
            ptr::null(),
            1,
            sizes.as_ptr(),
            NonNull::from(&mut sample),
        )
    };
    match NonNull::new(sample) {
        // SAFETY: a "Create" function: +1 retain count that we own.
        Some(p) if status == 0 => Ok(unsafe { CFRetained::from_raw(p) }),
        _ => Err(fail("cannot make a sample", status)),
    }
}

/// Copies a decoded BGRA pixel buffer into RGBA.
fn to_rgba(image: &CVImageBuffer) -> Result<Frame, Error> {
    if CVPixelBufferGetPixelFormatType(image) != kCVPixelFormatType_32BGRA {
        return Err(Error::Decode(
            "VideoToolbox returned an unexpected pixel format".into(),
        ));
    }
    let lock = CVPixelBufferLockFlags::ReadOnly;
    // SAFETY: `image` is a live pixel buffer; locking makes its memory readable until the
    // matching unlock below.
    if unsafe { CVPixelBufferLockBaseAddress(image, lock) } != kCVReturnSuccess {
        return Err(Error::Decode("cannot read the decoded picture".into()));
    }
    let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
    let stride = CVPixelBufferGetBytesPerRow(image);
    let base = CVPixelBufferGetBaseAddress(image).cast::<u8>();
    let rgba = if base.is_null() || h == 0 {
        None
    } else {
        // SAFETY: while locked, a non-planar pixel buffer's base address points to `height`
        // rows of `bytes_per_row` bytes; the slice does not outlive the lock.
        let pixels = unsafe { std::slice::from_raw_parts(base, stride * h) };
        convert::bgra_to_rgba(pixels, w, h, stride)
    };
    // SAFETY: unlocks the lock taken above, with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(image, lock) };
    let rgba = rgba.ok_or_else(|| Error::Decode("the decoded picture is incomplete".into()))?;
    Ok(Frame {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}
