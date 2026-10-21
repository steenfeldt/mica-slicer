// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer::_extract_layer_heights_profile_config_from_archive, _3MF_Exporter::_add_layer_height_profile_file_to_archive
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_extract_layer_heights_profile_config_from_archive, _BBS_3MF_Importer::_extract_brim_ear_points_from_archive, _BBS_3MF_Exporter::_add_layer_height_profile_file_to_archive, _BBS_3MF_Exporter::_add_brim_ear_points_file_to_archive
// Changes: Rust port; malformed lines are skipped instead of reported, as upstream continues past them.
// Upstream attribution (3mf.cpp): Copyright (c) Prusa Research 2018 - 2023
// Oleksandra Iushchenko, David Kocík, Enrico Turri, Lukáš Matěna, Lukáš Hejl,
// Filip Sykala, Vojtěch Bubník, Tomáš Mészáros; Copyright (c) 2020 Henner Zeller.
// PrusaSlicer is released under the terms of the AGPLv3 or higher.

//! The text sidecars for variable layer height profiles and brim ears.
//!
//! Profiles: one line per object, `object_id=N|z;h;z;h;…` (PrusaSlicer's
//! `Metadata/Slic3r_PE_layer_heights_profile.txt`, Orca's `Metadata/layer_heights_profile.txt`).
//! Brim ears (Orca's `Metadata/brim_ear_points.txt`): a `brim_points_format_version=0` header,
//! then `object_id=N|x y z r x y z r …`. `N` is the one-based model object index.

/// The file a dialect stores profiles in.
pub const PRUSA_PROFILE: &str = "Metadata/Slic3r_PE_layer_heights_profile.txt";
pub const ORCA_PROFILE: &str = "Metadata/layer_heights_profile.txt";
pub const BRIM_POINTS: &str = "Metadata/brim_ear_points.txt";

fn object_line(line: &str) -> Option<(usize, &str)> {
    let (id, data) = line.split_once('|')?;
    if data.contains('|') {
        return None;
    }
    let (key, value) = id.split_once('=')?;
    if value.contains('=') || key.is_empty() {
        return None;
    }
    // atoi: leading digits, 0 is invalid.
    let digits: String = value
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let id = digits.parse::<usize>().ok().filter(|&n| n > 0)?;
    Some((id, data))
}

/// atof: the leading number, 0 when there is none.
fn atof(s: &str) -> f64 {
    let s = s.trim_start();
    (1..=s.len())
        .rev()
        .filter(|&i| s.is_char_boundary(i))
        .find_map(|i| s[..i].parse::<f64>().ok())
        .unwrap_or(0.)
}

/// Profiles per one-based object index. Upstream rejects lists of four or fewer values and odd
/// lengths, and the first profile of an index wins.
pub fn parse_profiles(text: &str) -> Vec<(usize, Vec<f64>)> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut out: Vec<(usize, Vec<f64>)> = Vec::new();
    for line in text.split('\n') {
        let Some((id, data)) = object_line(line) else {
            continue;
        };
        if out.iter().any(|(i, _)| *i == id) {
            continue;
        }
        let values: Vec<&str> = data.split(';').collect();
        if values.len() <= 4 || !values.len().is_multiple_of(2) {
            continue;
        }
        out.push((id, values.iter().map(|v| atof(v)).collect()));
    }
    out
}

/// Brim ears per one-based object index, as `float`s (upstream parses them with `atof` into
/// `float`).
pub fn parse_brim_points(text: &str) -> Vec<(usize, Vec<[f32; 4]>)> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut lines: Vec<&str> = text.split('\n').collect();
    let key = "brim_points_format_version=";
    let mut version = 0;
    if let Some(first) = lines.first()
        && let Some(pos) = first.find(key)
    {
        version = atof(&first[pos + key.len()..]) as i64;
        lines.remove(0);
    }
    let mut out: Vec<(usize, Vec<[f32; 4]>)> = Vec::new();
    if version != 0 {
        return out;
    }
    for line in lines {
        let Some((id, data)) = object_line(line) else {
            continue;
        };
        if out.iter().any(|(i, _)| *i == id) {
            continue;
        }
        let values: Vec<f32> = data.split(' ').map(|v| atof(v) as f32).collect();
        let points: Vec<[f32; 4]> = values.as_chunks::<4>().0.to_vec();
        if !points.is_empty() {
            out.push((id, points));
        }
    }
    out
}

/// C's `%f`.
fn f(v: f64) -> String {
    format!("{v:.6}")
}

/// The profile file for `objects` (one-based index, profile); `None` when nothing is written.
/// Upstream writes even lists of at least four values.
pub fn write_profiles<'a>(objects: impl IntoIterator<Item = (usize, &'a [f64])>) -> Option<String> {
    let mut out = String::new();
    for (id, profile) in objects {
        if profile.len() >= 4 && profile.len().is_multiple_of(2) {
            out.push_str(&format!("object_id={id}|"));
            let values: Vec<String> = profile.iter().map(|v| f(*v)).collect();
            out.push_str(&values.join(";"));
            out.push('\n');
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The brim ear file (Orca), with its version header; `None` without ears.
pub fn write_brim_points<'a>(
    objects: impl IntoIterator<Item = (usize, &'a [[f32; 4]])>,
) -> Option<String> {
    let mut out = String::new();
    for (id, points) in objects {
        if points.is_empty() {
            continue;
        }
        out.push_str(&format!("object_id={id}|"));
        let values: Vec<String> = points
            .iter()
            .flat_map(|p| p.iter().map(|v| f(f64::from(*v))))
            .collect();
        out.push_str(&values.join(" "));
        out.push('\n');
    }
    (!out.is_empty()).then(|| format!("brim_points_format_version=0\n{out}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_round_trip_and_upstream_rules() {
        let text = write_profiles([
            (1, &[0., 0.2, 0.2, 0.2, 10., 0.1][..]),
            (2, &[0., 0.2][..]),
            (3, &[0., 0.2, 10., 0.2][..]),
        ])
        .unwrap();
        assert_eq!(
            text,
            "object_id=1|0.000000;0.200000;0.200000;0.200000;10.000000;0.100000\n\
             object_id=3|0.000000;0.200000;10.000000;0.200000\n"
        );
        // A four-value profile is written but not read back (upstream's `<= 4`).
        let parsed = parse_profiles(&text);
        assert_eq!(parsed, vec![(1, vec![0., 0.2, 0.2, 0.2, 10., 0.1])]);
        assert!(parse_profiles("object_id=0|0;0.2;0.2;0.2;1;0.2\nbad\n").is_empty());
        assert!(write_profiles([(1, &[][..])]).is_none());
    }

    #[test]
    fn brim_points_round_trip() {
        let text = write_brim_points([(2, &[[1.5, -2., -0.0001, 4.]][..]), (3, &[][..])]).unwrap();
        assert_eq!(
            text,
            "brim_points_format_version=0\nobject_id=2|1.500000 -2.000000 -0.000100 4.000000\n"
        );
        assert_eq!(
            parse_brim_points(&text),
            vec![(2, vec![[1.5, -2., -0.0001, 4.]])]
        );
        assert!(
            parse_brim_points("brim_points_format_version=1\nobject_id=1|0 0 0 1\n").is_empty()
        );
    }
}
