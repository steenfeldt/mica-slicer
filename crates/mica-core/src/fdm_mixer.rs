// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 bundled_deps/prusa_fdm_mixer/prusa_fdm_mixer.cpp :: predict_lab, mix, mix_rgb, hex_to_rgb, rgb_to_hex, rgb_to_lab, lab_to_rgb, delta_e_2000
// Source: prusaslicer@b028299c77 bundled_deps/prusa_fdm_mixer/prusa_fdm_mixer.hpp :: Part, RGB, LAB
// Changes: Rust port of the prusa-fdm-mixer colour model (identical to github.com/prusa3d/prusa-fdm-mixer@0124582 cpp/), errors as `Result` instead of exceptions; its 33 C++ checks (cpp/test_prusa_fdm_mixer.cpp) are the unit tests below.
// prusa-fdm-mixer: Copyright (c) 2026 Ondrej Bartas (Prusa Research s.r.o.) and contributors (MIT; see THIRD_PARTY.md).

//! Predicts the colour a layer-alternated filament mix shows (Yule–Nielsen in linear RGB with
//! fitted lightness, chroma and cyan-hue corrections). Used for the predicted swatch of a
//! mixed colour; it never affects G-code.

/// One filament of a mix: its colour and its share (shares sum to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub rgb: [u8; 3],
    pub ratio: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

fn srgb_to_linear(c: f64) -> f64 {
    let c = c / 255.;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0., 1.);
    let v = if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    };
    v * 255.
}

/// C++ `std::lround` then clamp to a byte.
fn clamp_round(v: f64) -> u8 {
    v.round().clamp(0., 255.) as u8
}

fn rgb_to_xyz(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|c| srgb_to_linear(f64::from(c)));
    [
        (r * 0.4124564 + g * 0.3575761 + b * 0.1804375) * 100.,
        (r * 0.2126729 + g * 0.7151522 + b * 0.0721750) * 100.,
        (r * 0.0193339 + g * 0.1191920 + b * 0.9503041) * 100.,
    ]
}

fn xyz_to_rgb([x, y, z]: [f64; 3]) -> [u8; 3] {
    let (x, y, z) = (x / 100., y / 100., z / 100.);
    [
        x * 3.2404542 + y * -1.5371385 + z * -0.4985314,
        x * -0.9692660 + y * 1.8760108 + z * 0.0415560,
        x * 0.0556434 + y * -0.2040259 + z * 1.0572252,
    ]
    .map(|c| clamp_round(linear_to_srgb(c)))
}

const XN: f64 = 95.047;
const YN: f64 = 100.0;
const ZN: f64 = 108.883;

fn lab_f(t: f64) -> f64 {
    if t > 0.008856 {
        t.cbrt()
    } else {
        7.787 * t + 16. / 116.
    }
}

fn lab_f_inv(t: f64) -> f64 {
    if t > 0.206893 {
        t * t * t
    } else {
        (t - 16. / 116.) / 7.787
    }
}

pub fn rgb_to_lab(rgb: [u8; 3]) -> Lab {
    let [x, y, z] = rgb_to_xyz(rgb);
    let (fx, fy, fz) = (lab_f(x / XN), lab_f(y / YN), lab_f(z / ZN));
    Lab {
        l: 116. * fy - 16.,
        a: 500. * (fx - fy),
        b: 200. * (fy - fz),
    }
}

pub fn lab_to_rgb(lab: Lab) -> [u8; 3] {
    let fy = (lab.l + 16.) / 116.;
    let fx = lab.a / 500. + fy;
    let fz = fy - lab.b / 200.;
    xyz_to_rgb([XN * lab_f_inv(fx), YN * lab_f_inv(fy), ZN * lab_f_inv(fz)])
}

/// `#rrggbb` or `rrggbb`, either case (also `#rrggbbaa`, alpha dropped: `crate::color`).
pub fn hex_to_rgb(hex: &str) -> Result<[u8; 3], String> {
    crate::color::parse_hex(hex).ok_or_else(|| format!("expected 6 hex digits, got: {hex}"))
}

/// Lowercase `#rrggbb`.
pub fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

const YN_EXPONENT: f64 = 3.0;
const L_SLOPE: f64 = -0.0477;
const L_INTERCEPT: f64 = -2.112;
const L_KINK_THRESHOLD: f64 = 15.0;
const L_KINK_SLOPE: f64 = -0.060;
const C_SLOPE: f64 = 0.2780;
const C_INTERCEPT: f64 = -15.580;
const PEAK_STRENGTH: f64 = 1.375;
const HUE_PEAK_DEG: f64 = 10.38;
const HUE_CENTER_DEG: f64 = 210.0;
const HUE_HALF_WIDTH_DEG: f64 = 30.0;

fn predict_lab(parts: &[Part]) -> Option<Lab> {
    if parts.is_empty() {
        return None;
    }
    // Gradient safety: a (nearly) pure component is returned unchanged.
    if let Some(p) = parts.iter().find(|p| p.ratio >= 0.9999) {
        return Some(rgb_to_lab(p.rgb));
    }
    let inv_n = 1. / YN_EXPONENT;
    let mut acc = [0f64; 3];
    let mut component_ls = Vec::with_capacity(parts.len());
    for p in parts {
        for (a, c) in acc.iter_mut().zip(p.rgb) {
            *a += srgb_to_linear(f64::from(c)).powf(inv_n) * p.ratio;
        }
        component_ls.push(rgb_to_lab(p.rgb).l);
    }
    let yn_rgb = acc.map(|a| clamp_round(linear_to_srgb(a.max(0.).powf(YN_EXPONENT))));
    let base = rgb_to_lab(yn_rgb);

    // Bell weight N^N · ∏ratios: 1 for equal shares, 0 at any endpoint.
    let n = parts.len() as f64;
    let w = (parts.iter().map(|p| p.ratio).product::<f64>() * n.powf(n)).clamp(0., 1.);
    let cs = w * PEAK_STRENGTH;

    let l_min = component_ls.iter().copied().fold(f64::INFINITY, f64::min);
    let l_max = component_ls
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let l_gap = l_max - l_min;
    let mut l_corr = L_SLOPE * l_gap + L_INTERCEPT;
    if l_gap > L_KINK_THRESHOLD {
        l_corr += L_KINK_SLOPE * (l_gap - L_KINK_THRESHOLD);
    }
    let l_new = base.l + l_corr * cs;

    let (mut a_out, mut b_out) = (base.a, base.b);
    let pred_c = base.a.hypot(base.b);
    if pred_c >= 0.01 {
        let target_dc = (C_SLOPE * l_new + C_INTERCEPT) * cs;
        let scale = (pred_c + target_dc).max(0.) / pred_c;
        a_out = base.a * scale;
        b_out = base.b * scale;
    }

    let c_final = a_out.hypot(b_out);
    if c_final >= 1. {
        let mut h = b_out.atan2(a_out).to_degrees();
        if h < 0. {
            h += 360.;
        }
        let mut h_corr = 0.;
        if (HUE_CENTER_DEG - HUE_HALF_WIDTH_DEG..HUE_CENTER_DEG + HUE_HALF_WIDTH_DEG).contains(&h) {
            let falloff = (1. - (h - HUE_CENTER_DEG).abs() / HUE_HALF_WIDTH_DEG).max(0.);
            h_corr = HUE_PEAK_DEG * falloff * w;
        }
        if h_corr != 0. {
            let mut new_h = (h + h_corr) % 360.;
            if new_h < 0. {
                new_h += 360.;
            }
            let r = new_h.to_radians();
            a_out = c_final * r.cos();
            b_out = c_final * r.sin();
        }
    }
    Some(Lab {
        l: l_new,
        a: a_out,
        b: b_out,
    })
}

/// The predicted colour of `parts` (shares should sum to 1); `None` without parts.
pub fn mix_rgb(parts: &[Part]) -> Option<[u8; 3]> {
    predict_lab(parts).map(lab_to_rgb)
}

/// [`mix_rgb`] over hex colours, as upstream's `mix`.
pub fn mix(parts: &[(&str, f64)]) -> Result<String, String> {
    let parts = parts
        .iter()
        .map(|(hex, ratio)| {
            Ok(Part {
                rgb: hex_to_rgb(hex)?,
                ratio: *ratio,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    mix_rgb(&parts)
        .map(rgb_to_hex)
        .ok_or_else(|| "empty parts list".to_string())
}

/// CIEDE2000 colour difference.
pub fn delta_e_2000(lab1: Lab, lab2: Lab) -> f64 {
    let (l1, a1, b1) = (lab1.l, lab1.a, lab1.b);
    let (l2, a2, b2) = (lab2.l, lab2.a, lab2.b);
    let avg_l = (l1 + l2) / 2.;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let avg_c = (c1 + c2) / 2.;
    let avg_c7 = avg_c.powi(7);
    let pow25_7 = 25f64.powi(7);
    let g = 0.5 * (1. - (avg_c7 / (avg_c7 + pow25_7)).sqrt());
    let a1p = a1 * (1. + g);
    let a2p = a2 * (1. + g);
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let avg_cp = (c1p + c2p) / 2.;
    let deg = |x: f64| {
        let d = x.to_degrees();
        if d < 0. { d + 360. } else { d }
    };
    let h1p = deg(b1.atan2(a1p));
    let h2p = deg(b2.atan2(a2p));
    let diff = h2p - h1p;
    let dhp = if c1p * c2p == 0. {
        0.
    } else if diff.abs() <= 180. {
        diff
    } else if diff > 180. {
        diff - 360.
    } else {
        diff + 360.
    };
    let avg_hp = if c1p * c2p == 0. {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180. {
        (h1p + h2p) / 2.
    } else if h1p + h2p < 360. {
        (h1p + h2p + 360.) / 2.
    } else {
        (h1p + h2p - 360.) / 2.
    };
    let t = 1. - 0.17 * (avg_hp - 30.).to_radians().cos()
        + 0.24 * (2. * avg_hp).to_radians().cos()
        + 0.32 * (3. * avg_hp + 6.).to_radians().cos()
        - 0.20 * (4. * avg_hp - 63.).to_radians().cos();
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp_final = 2. * (c1p * c2p).sqrt() * (dhp * std::f64::consts::PI / 360.).sin();
    let sl = 1. + (0.015 * (avg_l - 50.).powi(2)) / (20. + (avg_l - 50.).powi(2)).sqrt();
    let sc = 1. + 0.045 * avg_cp;
    let sh = 1. + 0.015 * avg_cp * t;
    let d_theta = 30. * (-((avg_hp - 275.) / 25.).powi(2)).exp();
    let avg_cp7 = avg_cp.powi(7);
    let rc = 2. * (avg_cp7 / (avg_cp7 + pow25_7)).sqrt();
    let rt = -rc * (2. * d_theta).to_radians().sin();
    ((dlp / sl).powi(2)
        + (dcp / sc).powi(2)
        + (dhp_final / sh).powi(2)
        + rt * (dcp / sc) * (dhp_final / sh))
        .sqrt()
}

#[cfg(test)]
mod tests {
    //! The 33 checks of upstream's `cpp/test_prusa_fdm_mixer.cpp`, one assertion each.
    use super::*;

    fn lab(hex: &str) -> Lab {
        rgb_to_lab(hex_to_rgb(hex).unwrap())
    }
    fn de(a: &str, b: &str) -> f64 {
        delta_e_2000(lab(a), lab(b))
    }
    const CASES: [&str; 8] = [
        "#000000", "#ffffff", "#ff0000", "#00ff00", "#0000ff", "#aabbcc", "#123456", "#7f7f7f",
    ];

    #[test]
    fn hex_round_trips() {
        for h in CASES {
            assert_eq!(rgb_to_hex(hex_to_rgb(h).unwrap()), h);
        }
    }

    #[test]
    fn lab_round_trips() {
        for h in CASES {
            let l = lab(h);
            let back = rgb_to_lab(lab_to_rgb(l));
            assert!(delta_e_2000(l, back) < 1., "{h}");
        }
    }

    #[test]
    fn delta_e_identical_is_zero() {
        let a = Lab {
            l: 50.,
            a: 0.,
            b: 0.,
        };
        assert!(delta_e_2000(a, a) < 0.001);
    }

    #[test]
    fn delta_e_sharma_pair() {
        let a = Lab {
            l: 50.,
            a: 2.6772,
            b: -79.7751,
        };
        let b = Lab {
            l: 50.,
            a: 0.,
            b: -82.7485,
        };
        assert!((delta_e_2000(a, b) - 2.0425).abs() < 0.01);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(hex_to_rgb("aabbcc").unwrap(), [0xaa, 0xbb, 0xcc]);
        assert_eq!(hex_to_rgb("#AABBCC").unwrap(), [0xaa, 0xbb, 0xcc]);
        assert!(hex_to_rgb("nope").is_err());
    }

    #[test]
    fn endpoints_are_the_pure_colours() {
        assert_eq!(mix(&[("#007a9d", 1.)]).unwrap(), "#007a9d");
        assert!(
            de(
                &mix(&[("#ff6338", 1.), ("#252e2e", 0.)]).unwrap(),
                "#ff6338"
            ) < 1.
        );
        assert!(
            de(
                &mix(&[("#ff6338", 0.), ("#252e2e", 1.)]).unwrap(),
                "#252e2e"
            ) < 1.
        );
    }

    #[test]
    fn reference_predictions() {
        for (a, ra, b, rb, expected) in [
            ("#009bc3", 0.5, "#f6b921", 0.5, "#519e5f"),
            ("#c9378c", 0.5, "#f6b921", 0.5, "#cc6545"),
            ("#009bc3", 0.5, "#c9378c", 0.5, "#4a5e94"),
            ("#252e2e", 0.5, "#e4e4e5", 0.5, "#636363"),
            ("#007a9d", 0.5, "#b32326", 0.5, "#434446"),
            ("#007a9d", 0.75, "#f6b921", 0.25, "#1e7c6f"),
            ("#347644", 0.75, "#e2deda", 0.25, "#4f7e5a"),
        ] {
            let got = mix(&[(a, ra), (b, rb)]).unwrap();
            assert!(de(&got, expected) < 1., "{a}+{b}: {got} vs {expected}");
        }
    }

    #[test]
    fn three_colours() {
        let third = 1. / 3.;
        let got = mix(&[("#009bc3", third), ("#c9378c", third), ("#f6b921", third)]).unwrap();
        let sum: u32 = hex_to_rgb(&got)
            .unwrap()
            .iter()
            .map(|c| u32::from(*c))
            .sum();
        assert!(sum > 200 && sum < 550, "{got}");
        let end = mix(&[("#009bc3", 1.), ("#c9378c", 0.), ("#f6b921", 0.)]).unwrap();
        assert!(de(&end, "#009bc3") < 1.);
    }
}
