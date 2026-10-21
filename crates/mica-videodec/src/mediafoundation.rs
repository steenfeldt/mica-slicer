// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Windows: the Media Foundation H.264 decoder (a synchronous MFT; Microsoft's own
//! software/DXVA decoder or a GPU vendor's, whichever Windows ranks first). Annex B access
//! units go in as samples; NV12 pictures come out and are converted to RGBA here.
//!
//! Every Media Foundation / COM call is `unsafe` in the `windows` crate (raw COM). They all
//! take live interface pointers owned by this module and plain values; the only raw memory
//! touched is the locked buffer of a sample, inside a Lock/Unlock pair.

use crate::{Error, Frame, convert};
use std::mem::ManuallyDrop;
use windows::Win32::{
    Foundation::RPC_E_CHANGED_MODE,
    Media::MediaFoundation::*,
    System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
};

pub const NAME: &str = "Media Foundation";

const UNAVAILABLE: &str = "H.264 camera streams need Windows' H.264 video decoder (on Windows \"N\" editions, install the Media Feature Pack)";

/// COM and Media Foundation, started for this thread for the decoder's lifetime.
struct Runtime {
    uninit_com: bool,
}

impl Runtime {
    fn start() -> Result<Self, Error> {
        // SAFETY: plain COM initialisation of the calling thread; a thread already in
        // another apartment answers RPC_E_CHANGED_MODE, which is fine (COM is usable) and
        // must then not be balanced with CoUninitialize.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let uninit_com = hr.is_ok();
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(Error::Unavailable(format!("{UNAVAILABLE} (COM: {hr})")));
        }
        // SAFETY: MFStartup with the SDK version it was built for; balanced in Drop.
        if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) } {
            if uninit_com {
                // SAFETY: balances the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            return Err(Error::Unavailable(format!("{UNAVAILABLE} ({e})")));
        }
        Ok(Self { uninit_com })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: balances MFStartup and (when it succeeded) CoInitializeEx on this thread
        // (the decoder is not `Send`), after the transform was released.
        unsafe {
            let _ = MFShutdown();
            if self.uninit_com {
                CoUninitialize();
            }
        }
    }
}

pub struct Backend {
    // Field order matters: the transform is released before Media Foundation shuts down.
    transform: IMFTransform,
    /// Output picture size and row stride (bytes) of the current output type.
    size: Option<(u32, u32, u32)>,
    provides_samples: bool,
    out_size: u32,
    time: i64,
    convert: bool,
    /// Pictures taken out while making room for input.
    pending: Vec<Frame>,
    _runtime: Runtime,
}

fn mf(what: &str) -> impl Fn(windows::core::Error) -> Error + '_ {
    move |e| Error::Decode(format!("{what}: {e}"))
}

fn create(what: &str) -> impl Fn(windows::core::Error) -> Error + '_ {
    move |e| Error::Create(format!("{what}: {e}"))
}

/// The best-ranked synchronous H.264 decoder transform.
fn find_decoder() -> Result<IMFTransform, Error> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: `input` lives for the call; `list` / `count` receive a CoTaskMem array of
    // `count` activation objects that we own (each released, then the array freed, below).
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            None,
            &mut list,
            &mut count,
        )
    }
    .map_err(|e| Error::Unavailable(format!("{UNAVAILABLE} ({e})")))?;
    let mut activates = Vec::with_capacity(count as usize);
    if !list.is_null() {
        for i in 0..count as usize {
            // SAFETY: `i < count` elements of the array; reading moves each reference out
            // (the array is freed without releasing them again).
            activates.push(unsafe { list.add(i).read() });
        }
        // SAFETY: the array came from MFTEnumEx (CoTaskMemAlloc) and is not used again.
        unsafe { CoTaskMemFree(Some(list.cast_const().cast())) };
    }
    let mut last = None;
    for a in activates.into_iter().flatten() {
        // SAFETY: a live activation object; asks it for the transform it creates.
        match unsafe { a.ActivateObject::<IMFTransform>() } {
            Ok(t) => return Ok(t),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Unavailable(match last {
        Some(e) => format!("{UNAVAILABLE} ({e})"),
        None => UNAVAILABLE.into(),
    }))
}

impl Backend {
    pub fn new() -> Result<Self, Error> {
        let runtime = Runtime::start()?;
        let transform = find_decoder()?;
        // SAFETY: calls on the live transform and on media types it creates or we create;
        // the GUID keys are constants that outlive each call.
        unsafe {
            if let Ok(attrs) = transform.GetAttributes() {
                // Fewer frames held back (cameras want latency low); not every decoder knows it.
                let _ = attrs.SetUINT32(&CODECAPI_AVLowLatencyMode, 1);
            }
            let input = MFCreateMediaType().map_err(create("media type"))?;
            input
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(create("media type"))?;
            input
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(create("media type"))?;
            transform
                .SetInputType(0, &input, 0)
                .map_err(create("the decoder refused H.264 input"))?;
        }
        let mut b = Self {
            transform,
            size: None,
            provides_samples: false,
            out_size: 0,
            time: 0,
            convert: true,
            pending: vec![],
            _runtime: runtime,
        };
        b.choose_output()?;
        // SAFETY: streaming notifications on the live transform; no parameters.
        unsafe {
            b.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(create("cannot start streaming"))?;
            b.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(create("cannot start streaming"))?;
        }
        Ok(b)
    }

    /// Picks NV12 output (every H.264 decoder offers it) and reads its size.
    fn choose_output(&mut self) -> Result<(), Error> {
        // SAFETY: calls on the live transform and the media types it returns; the GUID keys
        // are constants.
        unsafe {
            let mut i = 0;
            loop {
                let t = match self.transform.GetOutputAvailableType(0, i) {
                    Ok(t) => t,
                    Err(e) => return Err(Error::Create(format!("no NV12 output: {e}"))),
                };
                if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                    self.transform
                        .SetOutputType(0, &t, 0)
                        .map_err(create("cannot set NV12 output"))?;
                    self.size = t.GetUINT64(&MF_MT_FRAME_SIZE).ok().map(|s| {
                        let (w, h) = ((s >> 32) as u32, s as u32);
                        // A negative stride (bottom-up) does not happen for NV12.
                        let stride = t
                            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                            .ok()
                            .filter(|&s| (s as i32) > 0)
                            .unwrap_or(w);
                        (w, h, stride)
                    });
                    break;
                }
                i += 1;
            }
            let info = self
                .transform
                .GetOutputStreamInfo(0)
                .map_err(create("output stream"))?;
            self.provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            self.out_size = info.cbSize;
        }
        Ok(())
    }

    pub fn parameters(&mut self, annex_b: &[u8]) -> Result<(), Error> {
        // Parameter sets go in band, with the first access unit.
        self.input(annex_b)
    }

    fn input(&mut self, au: &[u8]) -> Result<(), Error> {
        let len =
            u32::try_from(au.len()).map_err(|_| Error::Decode("access unit too large".into()))?;
        // SAFETY: a new memory buffer of `len` bytes is locked, filled with exactly `len`
        // bytes from `au` and unlocked before it is attached to a new sample; all objects are
        // live and owned here.
        let sample = unsafe {
            let buffer = MFCreateMemoryBuffer(len.max(1)).map_err(mf("buffer"))?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None).map_err(mf("buffer"))?;
            std::ptr::copy_nonoverlapping(au.as_ptr(), data, au.len());
            buffer.Unlock().map_err(mf("buffer"))?;
            buffer.SetCurrentLength(len).map_err(mf("buffer"))?;
            let sample = MFCreateSample().map_err(mf("sample"))?;
            sample.AddBuffer(&buffer).map_err(mf("sample"))?;
            sample.SetSampleTime(self.time).map_err(mf("sample"))?;
            sample
        };
        self.time += 333_333;
        loop {
            // SAFETY: a live sample on the live transform's input stream 0.
            match unsafe { self.transform.ProcessInput(0, &sample, 0) } {
                Ok(()) => return Ok(()),
                // Full: take the pictures out first, then retry.
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    let mut drained = std::mem::take(&mut self.pending);
                    let n = self.output(&mut drained)?;
                    self.pending = drained;
                    if n == 0 {
                        return Err(Error::Decode("the decoder does not take more input".into()));
                    }
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            }
        }
    }

    /// Takes every finished picture out of the transform (converted only when wanted).
    /// Returns how many came out.
    fn output(&mut self, frames: &mut Vec<Frame>) -> Result<usize, Error> {
        let mut n = 0;
        loop {
            let provided = if self.provides_samples {
                None
            } else {
                // SAFETY: creates a sample with a buffer of the size the transform asked for.
                Some(unsafe {
                    let s = MFCreateSample().map_err(mf("sample"))?;
                    let b = MFCreateMemoryBuffer(self.out_size.max(1)).map_err(mf("buffer"))?;
                    s.AddBuffer(&b).map_err(mf("sample"))?;
                    s
                })
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(provided),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            // SAFETY: one output buffer description for stream 0 with our sample (or none
            // when the transform provides them); `status` is a valid place for the result.
            let r = unsafe { self.transform.ProcessOutput(0, &mut buffers, &mut status) };
            // We own whatever sample / events the structure now holds.
            let sample = ManuallyDrop::into_inner(std::mem::replace(
                &mut buffers[0].pSample,
                ManuallyDrop::new(None),
            ));
            drop(ManuallyDrop::into_inner(std::mem::replace(
                &mut buffers[0].pEvents,
                ManuallyDrop::new(None),
            )));
            match r {
                Ok(()) => {
                    if let Some(s) = sample {
                        n += 1;
                        if self.convert {
                            frames.push(self.picture(&s)?);
                        }
                    }
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(n),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.choose_output()?,
                Err(e) => return Err(Error::Decode(e.to_string())),
            }
        }
    }

    fn picture(&self, sample: &IMFSample) -> Result<Frame, Error> {
        let (w, h, stride) = self
            .size
            .ok_or_else(|| Error::Decode("the decoder did not say the picture size".into()))?;
        // SAFETY: the sample's single contiguous buffer is locked; its first
        // `current length` bytes are readable until the Unlock below, and the slice does not
        // outlive the lock.
        unsafe {
            let buffer = sample.ConvertToContiguousBuffer().map_err(mf("picture"))?;
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut data, None, Some(&mut len))
                .map_err(mf("picture"))?;
            let rgba = if data.is_null() {
                None
            } else {
                let bytes = std::slice::from_raw_parts(data, len as usize);
                nv12(bytes, w as usize, h as usize, stride as usize)
            };
            let _ = buffer.Unlock();
            let rgba =
                rgba.ok_or_else(|| Error::Decode("the decoded picture is incomplete".into()))?;
            Ok(Frame {
                width: w,
                height: h,
                rgba,
            })
        }
    }

    pub fn decode(&mut self, au: &[u8], convert: bool) -> Result<Option<Frame>, Error> {
        self.convert = convert;
        self.input(au)?;
        let mut frames = std::mem::take(&mut self.pending);
        self.output(&mut frames)?;
        Ok(frames.pop())
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, Error> {
        self.convert = true;
        // SAFETY: end-of-stream and drain notifications on the live transform.
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                .map_err(mf("drain"))?;
        }
        let mut frames = std::mem::take(&mut self.pending);
        self.output(&mut frames)?;
        Ok(frames)
    }
}

/// An NV12 buffer whose luma plane is `stride` wide and padded to some height (decoders
/// pad to 16 rows): the chroma plane starts after the padded luma plane.
fn nv12(bytes: &[u8], w: usize, h: usize, stride: usize) -> Option<Vec<u8>> {
    if stride == 0 {
        return None;
    }
    // len = stride * padded * 3 / 2
    let padded = (bytes.len() * 2 / 3 / stride).max(h);
    let uv_start = stride.checked_mul(padded)?;
    convert::nv12_to_rgba(bytes, stride, bytes.get(uv_start..)?, stride, w, h)
}
