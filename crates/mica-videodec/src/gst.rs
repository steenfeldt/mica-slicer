// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Linux and the other Unix systems: GStreamer, through its safe Rust bindings (no unsafe
//! here). `appsrc ! h264parse ! decodebin ! videoconvert ! RGBA appsink`: decodebin picks
//! the best H.264 decoder the distribution installed (VA-API, libav, OpenH264 plugin, …),
//! so the codec licence is the distribution's. Decoding runs on GStreamer's threads;
//! `decode` returns the newest picture ready so far (one or two frames behind), `flush`
//! waits for the rest.

use crate::{Error, Frame, annexb};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;
use std::time::{Duration, Instant};

pub const NAME: &str = "GStreamer";

const UNAVAILABLE: &str =
    "H.264 camera streams need GStreamer with an H.264 decoder (gst-plugins-bad/libav)";

pub struct Backend {
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
    sink: gst_app::AppSink,
    /// Parameter sets given up front, sent with the next access unit.
    params: Vec<u8>,
    frame: u64,
    /// Access units with a picture pushed, pictures taken out.
    pushed: u64,
    pulled: u64,
}

/// How many pictures decoding may run behind the input, and how long `decode` waits when
/// it is further behind (the first pictures, while decodebin picks the decoder).
const MAX_LAG: u64 = 2;
const LAG_WAIT: Duration = Duration::from_millis(500);

fn unavailable(detail: impl std::fmt::Display) -> Error {
    Error::Unavailable(format!("{UNAVAILABLE}; {detail}"))
}

fn h264_caps() -> gst::Caps {
    gst::Caps::builder("video/x-h264")
        .field("stream-format", "byte-stream")
        .field("alignment", "au")
        .build()
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Backend {
    pub fn new() -> Result<Self, Error> {
        gst::init().map_err(|e| unavailable(format!("GStreamer did not start: {e}")))?;
        for element in [
            "appsrc",
            "appsink",
            "h264parse",
            "decodebin",
            "videoconvert",
        ] {
            if gst::ElementFactory::find(element).is_none() {
                return Err(unavailable(format!("the element \"{element}\" is missing")));
            }
        }
        let parsed = gst::Caps::builder("video/x-h264").build();
        let has_decoder = gst::ElementFactory::factories_with_type(
            gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
            gst::Rank::MARGINAL,
        )
        .iter()
        .any(|f| f.can_sink_any_caps(&parsed));
        if !has_decoder {
            return Err(Error::Unavailable(UNAVAILABLE.into()));
        }
        // A live source: decoders then avoid frame-threading delay (gst-libav switches to
        // slice threads), so pictures come out right after their access unit.
        let pipeline = gst::parse::launch(
            "appsrc name=src format=time is-live=true ! h264parse ! decodebin ! videoconvert \
             ! video/x-raw,format=RGBA ! appsink name=sink sync=false",
        )
        .map_err(|e| Error::Create(e.to_string()))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| Error::Create("not a pipeline".into()))?;
        let by_name = |name: &str| {
            pipeline
                .by_name(name)
                .ok_or_else(|| Error::Create(format!("no {name} in the pipeline")))
        };
        let src = by_name("src")?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| Error::Create("appsrc".into()))?;
        let sink = by_name("sink")?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| Error::Create("appsink".into()))?;
        src.set_caps(Some(&h264_caps()));
        src.set_max_bytes(0);
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| Error::Create(format!("GStreamer would not play: {e}")))?;
        Ok(Self {
            pipeline,
            src,
            sink,
            params: vec![],
            frame: 0,
            pushed: 0,
            pulled: 0,
        })
    }

    pub fn parameters(&mut self, annex_b: &[u8]) -> Result<(), Error> {
        self.params = annex_b.to_vec();
        Ok(())
    }

    /// A GStreamer error posted on the bus, as ours.
    fn bus_error(&self) -> Result<(), Error> {
        let Some(bus) = self.pipeline.bus() else {
            return Ok(());
        };
        while let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(err) = msg.view() {
                let e = err.error();
                let text = e.to_string();
                if e.matches(gst::CoreError::MissingPlugin)
                    || e.matches(gst::StreamError::CodecNotFound)
                {
                    return Err(unavailable(text));
                }
                return Err(Error::Decode(text));
            }
        }
        Ok(())
    }

    fn rgba(sample: &gst::Sample) -> Result<Frame, Error> {
        let bad = |what: &str| Error::Decode(format!("GStreamer: {what}"));
        let caps = sample.caps().ok_or_else(|| bad("picture without caps"))?;
        let info = gst_video::VideoInfo::from_caps(caps).map_err(|_| bad("unknown picture"))?;
        let buffer = sample.buffer_owned().ok_or_else(|| bad("empty picture"))?;
        let frame = gst_video::VideoFrame::from_buffer_readable(buffer, &info)
            .map_err(|_| bad("unreadable picture"))?;
        let (w, h) = (info.width() as usize, info.height() as usize);
        let stride = frame.plane_stride()[0] as usize;
        let data = frame.plane_data(0).map_err(|_| bad("unreadable picture"))?;
        if stride < w * 4 || h == 0 || data.len() < stride * (h - 1) + w * 4 {
            return Err(bad("short picture"));
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            rgba.extend_from_slice(&data[y * stride..y * stride + w * 4]);
        }
        Ok(Frame {
            width: w as u32,
            height: h as u32,
            rgba,
        })
    }

    /// Pictures already decoded (waiting up to `wait` for the first one, or for the end
    /// of the stream after `flush`); converted only when wanted.
    fn ready(
        &mut self,
        convert: bool,
        wait: Option<Duration>,
        all: bool,
    ) -> Result<Vec<Frame>, Error> {
        let mut out = vec![];
        let end = wait.map(|w| Instant::now() + w);
        loop {
            let timeout = end.map_or(gst::ClockTime::ZERO, |e| {
                gst::ClockTime::from_nseconds(
                    e.saturating_duration_since(Instant::now()).as_nanos() as u64,
                )
            });
            let Some(sample) = self.sink.try_pull_sample(Some(timeout)) else {
                break;
            };
            self.pulled += 1;
            if convert {
                out.push(Self::rgba(&sample)?);
            }
            if !all || self.sink.is_eos() {
                // Anything else already there, without waiting.
                return self.ready(convert, None, false).map(|mut more| {
                    out.append(&mut more);
                    out
                });
            }
        }
        Ok(out)
    }

    pub fn decode(&mut self, au: &[u8], convert: bool) -> Result<Option<Frame>, Error> {
        let pictures = annexb::nal_units(au).iter().any(|n| annexb::is_vcl(n));
        let mut data = std::mem::take(&mut self.params);
        data.extend_from_slice(au);
        let mut buffer = gst::Buffer::from_mut_slice(data);
        {
            let b = buffer.get_mut().expect("a new buffer is writable");
            b.set_pts(gst::ClockTime::from_mseconds(self.frame * 40));
        }
        self.frame += 1;
        self.pushed += u64::from(pictures);
        self.src
            .push_buffer(buffer)
            .map_err(|e| Error::Decode(format!("GStreamer: {e:?}")))?;
        self.bus_error()?;
        // Decoding runs on GStreamer's threads. Keep it at most MAX_LAG pictures behind,
        // so the newest picture is never more than that old.
        let wait = (self.pushed.saturating_sub(self.pulled) > MAX_LAG).then_some(LAG_WAIT);
        let mut frames = self.ready(convert, wait, false)?;
        if self.pushed.saturating_sub(self.pulled) > MAX_LAG {
            // Frames the decoder dropped (corrupt data): stop waiting for them.
            self.pulled = self.pushed - MAX_LAG;
        }
        self.bus_error()?;
        Ok(frames.pop())
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, Error> {
        let _ = self.src.end_of_stream();
        let frames = self.ready(true, Some(Duration::from_secs(5)), true);
        (self.pushed, self.pulled) = (0, 0);
        self.bus_error()?;
        // Ready for a new stream (which starts with a key frame).
        let _ = self.pipeline.set_state(gst::State::Null);
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| Error::Create(format!("GStreamer would not play: {e}")))?;
        frames
    }
}
