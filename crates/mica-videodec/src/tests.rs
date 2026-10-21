// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;

/// 20 frames of the mock camera's test picture, 320 × 240 (see testdata/test-pattern.py).
const CLIP: &[u8] = include_bytes!("../testdata/test-pattern-320x240.h264");

fn px(f: &Frame, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * f.width + x) * 4) as usize;
    [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 24)
}

/// The four quadrant colours (red, green, blue, white) where the sweeping bar is not.
fn assert_test_picture(f: &Frame) {
    assert_eq!((f.width, f.height), (320, 240));
    for (x, y, c) in [
        (40, 30, [220, 40, 40]),
        (280, 30, [40, 200, 60]),
        (40, 210, [50, 70, 220]),
        (280, 210, [235, 235, 235]),
    ] {
        // The bar may cover one sample point; then the one next to it must be right.
        let ok = near(px(f, x, y), c) || near(px(f, x + 20, y), c);
        assert!(
            ok,
            "pixel at {x},{y} is {:?}, expected about {c:?}",
            px(f, x, y)
        );
    }
    assert!(f.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    let bright = f
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 100 || p[1] > 100 || p[2] > 100);
    assert!(
        bright.count() > 320 * 240 / 2,
        "the picture is mostly black"
    );
}

#[test]
fn the_clip_splits_into_twenty_access_units() {
    assert!(CLIP.len() < 50 * 1024);
    let aus = access_units(CLIP);
    assert_eq!(aus.len(), 20);
    let kinds: Vec<u8> = nal_units(&aus[0]).iter().map(|n| n[0] & 0x1F).collect();
    assert_eq!(kinds, [9, 7, 8, 5]);
    assert!(aus[1..].iter().all(|au| {
        let k: Vec<u8> = nal_units(au).iter().map(|n| n[0] & 0x1F).collect();
        k == [9, 1]
    }));
}

#[test]
fn the_system_decoder_decodes_the_test_clip() {
    let mut d = match Decoder::new(Codec::H264, None) {
        Ok(d) => d,
        Err(e) => panic!("{BACKEND}: {e}"),
    };
    let mut frames = vec![];
    for (i, au) in access_units(CLIP).iter().enumerate() {
        if i % 3 == 1 {
            // What a slow thumbnail does: decode, do not convert.
            d.decode_only(au).unwrap();
        } else if let Some(f) = d.decode(au).unwrap() {
            frames.push(f);
        }
    }
    frames.extend(d.flush().unwrap());
    assert!(frames.len() >= 5, "{BACKEND}: {} frames", frames.len());
    for f in &frames {
        assert_test_picture(f);
    }
    // The bar moved: the last picture is not the first.
    assert_ne!(frames.first(), frames.last());
}

#[test]
fn parameters_up_front_start_the_decoder_at_once() {
    let aus = access_units(CLIP);
    let nals = nal_units(&aus[0]);
    let mut params = vec![];
    for n in &nals[1..3] {
        params.extend_from_slice(&[0, 0, 0, 1]);
        params.extend_from_slice(n);
    }
    let mut d = Decoder::new(Codec::H264, Some(&params)).unwrap();
    // The key frame without its parameter sets.
    let mut idr = vec![0, 0, 0, 1];
    idr.extend_from_slice(nals[3]);
    let mut got = d.decode(&idr).unwrap().into_iter().collect::<Vec<_>>();
    got.extend(d.flush().unwrap());
    assert_test_picture(got.last().expect("a picture from the key frame"));
}

// VideoToolbox checks parameter sets when the decoder starts; Media Foundation and
// GStreamer take them in band and report problems per frame.
#[cfg(all(target_os = "macos", not(feature = "openh264")))]
#[test]
fn broken_parameter_sets_are_a_typed_creation_error() {
    // A "SPS" that is no SPS: the system decoder must refuse to start, with a reason.
    let bogus = [0, 0, 0, 1, 0x67, 0xff, 0x00, 0x13, 0, 0, 0, 1, 0x68, 0x00];
    let e = Decoder::new(Codec::H264, Some(&bogus))
        .err()
        .expect("an error");
    assert!(e.is_fatal(), "{e:?}");
    assert!(e.to_string().contains("H.264"), "{e}");
}

#[test]
fn garbage_is_an_error_or_nothing_never_a_panic() {
    let mut d = Decoder::new(Codec::H264, None).unwrap();
    let _ = d.decode(&[0, 0, 0, 1, 0x65, 0xff, 0x00, 0x13]);
    let _ = d.decode(&[0, 0, 1]);
    let _ = d.decode(&[]);
    // Still usable for a real stream afterwards.
    let mut n = 0;
    for au in access_units(CLIP) {
        n += usize::from(d.decode(&au).ok().flatten().is_some());
    }
    n += d.flush().map_or(0, |f| f.len());
    assert!(n >= 5, "{n}");
}

#[test]
fn errors_read_as_sentences() {
    let e = Error::Unavailable("H.264 camera streams need GStreamer".into());
    assert!(e.is_fatal());
    assert!(!Error::Decode("bad data".into()).is_fatal());
    assert_eq!(
        Error::UnsupportedSize {
            width: 0,
            height: 3
        }
        .to_string(),
        "the camera's video has an unsupported size (0 × 3)"
    );
}

/// Run with `MICA_VIDEODEC_EXPECT_UNAVAILABLE=1` on a system without an H.264 decoder
/// (e.g. GStreamer without gst-libav / gst-plugins-bad): the decoder must say so.
#[test]
#[ignore = "needs a system without an H.264 decoder: MICA_VIDEODEC_EXPECT_UNAVAILABLE=1 cargo test -- --ignored"]
fn a_missing_system_decoder_is_reported() {
    assert!(
        std::env::var_os("MICA_VIDEODEC_EXPECT_UNAVAILABLE").is_some(),
        "set MICA_VIDEODEC_EXPECT_UNAVAILABLE=1 on a system without an H.264 decoder"
    );
    let e = Decoder::new(Codec::H264, None)
        .err()
        .expect("no decoder expected");
    assert!(e.is_fatal(), "{e:?}");
    assert!(e.to_string().contains("H.264 camera streams need"), "{e}");
}
