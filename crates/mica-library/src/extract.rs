// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Reading metadata out of a file's content. Nothing here writes anywhere: the caller gets
//! the facts and the thumbnail bytes and stores them in the app's data folder.
//!
//! The 3MF and G-code keys read here (`printer_settings_id`, `filament_colour`,
//! `; thumbnail begin`, `slice_info.config`, …) are file-format knowledge shared by
//! PrusaSlicer, OrcaSlicer and Mica's own output; no slicer code is involved.

use crate::image::{base64_decode, normalise};
use crate::model::{Info, Kind};
use mica_core::mesh::TriangleMesh;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Larger G-code is only read at its start and end (thumbnails and metadata live there).
const GCODE_WINDOW: u64 = 1 << 20;

/// A thumbnail to cache: bytes and file extension.
pub type Thumb = (Vec<u8>, &'static str);

pub struct Extracted {
    pub info: Info,
    pub thumbnail: Option<Thumb>,
}

/// Reads one file. `data` is the whole content when it was small enough to hold in memory
/// (it was hashed from it); otherwise the file is read again in parts. Models without an
/// embedded thumbnail get one rendered later, in the background (`crate::render`).
pub fn extract(kind: Kind, path: &Path, data: Option<&[u8]>) -> Extracted {
    let result = match kind {
        Kind::Stl => data
            .ok_or_else(|| "too large to preview in the library".to_string())
            .and_then(|d| TriangleMesh::from_stl_bytes(d).map_err(|e| e.to_string()))
            .map(|m| from_mesh(&m)),
        Kind::Obj => data
            .ok_or_else(|| "too large to preview in the library".to_string())
            .and_then(parse_obj)
            .map(|m| from_mesh(&m)),
        Kind::ThreeMf => data
            .ok_or_else(|| "too large to preview in the library".to_string())
            .and_then(three_mf),
        Kind::Gcode => gcode_text(path, data),
        Kind::Bgcode => bgcode(path, data),
        Kind::Step => Ok(Extracted {
            info: Info {
                readable: false,
                error: Some("Mica can't read STEP files yet; listed by name".into()),
                ..Default::default()
            },
            thumbnail: None,
        }),
    };
    result.unwrap_or_else(|e| Extracted {
        info: Info {
            readable: false,
            error: Some(e),
            ..Default::default()
        },
        thumbnail: None,
    })
}

fn size_of(mesh: &TriangleMesh) -> Option<[f32; 3]> {
    if mesh.vertices.is_empty() {
        return None;
    }
    let (lo, hi) = mesh.bounds();
    let s = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    s.iter().all(|v| v.is_finite()).then_some(s)
}

fn from_mesh(mesh: &TriangleMesh) -> Extracted {
    Extracted {
        info: Info {
            readable: true,
            size_mm: size_of(mesh),
            triangles: mesh.faces.len() as u64,
            objects: 1,
            plates: 0,
            ..Default::default()
        },
        thumbnail: None,
    }
}

/// Wavefront OBJ: `v` and `f` records (polygons fanned into triangles, negative indices
/// relative to the end). Everything else is ignored.
pub fn parse_obj(data: &[u8]) -> Result<TriangleMesh, String> {
    let text = String::from_utf8_lossy(data);
    let mut mesh = TriangleMesh::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let mut v = [0f32; 3];
                for c in &mut v {
                    *c = it
                        .next()
                        .and_then(|s| s.parse().ok())
                        .ok_or("OBJ: a vertex needs three numbers")?;
                }
                mesh.vertices.push(v);
            }
            Some("f") => {
                let n = mesh.vertices.len() as i64;
                let idx: Vec<u32> = it
                    .map(|t| {
                        let i: i64 = t
                            .split('/')
                            .next()
                            .and_then(|s| s.parse().ok())
                            .ok_or("OBJ: bad face index")?;
                        let i = if i < 0 { n + i } else { i - 1 };
                        if (0..n).contains(&i) {
                            Ok(i as u32)
                        } else {
                            Err("OBJ: face index out of range")
                        }
                    })
                    .collect::<Result<_, _>>()?;
                for k in 1..idx.len().saturating_sub(1) {
                    mesh.faces.push([idx[0], idx[k], idx[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if mesh.faces.is_empty() {
        return Err("OBJ: no faces".into());
    }
    Ok(mesh)
}

// ----- 3MF -------------------------------------------------------------------------------

/// Reads `key = value` settings from a PrusaSlicer INI (`; key = value` lines too) or the
/// values of an Orca JSON config; list values are joined with `;`.
fn config_values(path: &str, text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if path.ends_with(".json") || text.trim_start().starts_with('{') {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text) {
            for (k, v) in map {
                let s = match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Array(a) => a
                        .iter()
                        .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string))
                        .collect::<Vec<_>>()
                        .join(";"),
                    other => other.to_string(),
                };
                out.insert(k, s);
            }
        }
    } else {
        for line in text.lines() {
            let line = line.trim_start_matches(';').trim();
            if let Some((k, v)) = line.split_once(" = ") {
                out.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    out
}

fn list(v: &str) -> Vec<String> {
    v.split(';')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn colour(s: &str) -> Option<String> {
    let s = s.trim().trim_matches('"');
    let hex = s.strip_prefix('#')?;
    (hex.len() >= 6 && hex[..6].chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex[..6].to_ascii_uppercase()))
}

/// The tools' colours: per-extruder colours where set, else the filament colours.
fn colours(values: &HashMap<String, String>) -> Vec<String> {
    let filament = values
        .get("filament_colour")
        .map(|v| list(v))
        .unwrap_or_default();
    let extruder = values
        .get("extruder_colour")
        .map(|v| list(v))
        .unwrap_or_default();
    let n = filament.len().max(extruder.len());
    (0..n)
        .filter_map(|i| {
            extruder
                .get(i)
                .and_then(|c| colour(c))
                .or_else(|| filament.get(i).and_then(|c| colour(c)))
        })
        .collect()
}

fn apply_profiles(info: &mut Info, values: &HashMap<String, String>) {
    let get = |k: &str| {
        values
            .get(k)
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    };
    info.printer = get("printer_settings_id").or_else(|| get("printer_model"));
    info.print_profile = get("print_settings_id");
    info.filament_profiles = values
        .get("filament_settings_id")
        .map(|v| list(v))
        .unwrap_or_default();
}

fn zip_entry(
    zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    name: &str,
    cap: u64,
) -> Option<Vec<u8>> {
    let f = zip.by_name(name).ok()?;
    if f.size() > cap {
        return None;
    }
    let mut out = Vec::with_capacity(f.size() as usize);
    f.take(cap).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Values of `<metadata key="K" value="V"/>` and attribute values in Orca's
/// `slice_info.config`, summed over its plates.
fn slice_info(text: &str, info: &mut Info) {
    let mut time = 0.;
    let mut weight = 0.;
    let mut metres = 0.;
    let attr = |tag: &str, name: &str| -> Option<String> {
        let pat = format!("{name}=\"");
        let i = tag.find(&pat)? + pat.len();
        let j = tag[i..].find('"')? + i;
        Some(tag[i..j].to_string())
    };
    for tag in text.split('<').skip(1) {
        if tag.starts_with("metadata") {
            let key = attr(tag, "key");
            let val = attr(tag, "value").and_then(|v| v.parse::<f64>().ok());
            match (key.as_deref(), val) {
                (Some("prediction"), Some(v)) => time += v,
                (Some("weight"), Some(v)) => weight += v,
                _ => {}
            }
        } else if tag.starts_with("filament ")
            && let Some(m) = attr(tag, "used_m").and_then(|v| v.parse::<f64>().ok())
        {
            metres += m;
        }
    }
    if time > 0. {
        info.print_time_s = Some(time);
    }
    if weight > 0. {
        info.filament_g = Some(weight);
    }
    if metres > 0. {
        info.filament_m = Some(metres);
    }
}

/// The assembled first plate of a project (every printable model part of the objects on it,
/// in bed coordinates), or everything without plate data.
pub fn first_plate_mesh(project: &mica_core::threemf::Project) -> TriangleMesh {
    let first: Option<BTreeSet<(usize, usize)>> = project
        .plates
        .first()
        .map(|p| p.instances.iter().map(|i| (i.object, i.instance)).collect());
    let mut mesh = TriangleMesh::default();
    for (oi, o) in project.objects.iter().enumerate() {
        for (ii, inst) in o.instances.iter().enumerate() {
            if !inst.printable || first.as_ref().is_some_and(|f| !f.contains(&(oi, ii))) {
                continue;
            }
            for part in o
                .parts
                .iter()
                .filter(|p| p.kind == mica_core::threemf::PartKind::Model)
            {
                let base = mesh.vertices.len() as u32;
                mesh.vertices
                    .extend(part.mesh.vertices.iter().map(|&v| inst.transform.apply(v)));
                let mirrored = inst.transform.determinant() < 0.;
                mesh.faces.extend(part.mesh.faces.iter().map(|f| {
                    let f = f.map(|i| i + base);
                    if mirrored { [f[0], f[2], f[1]] } else { f }
                }));
            }
        }
    }
    mesh
}

fn three_mf(data: &[u8]) -> Result<Extracted, String> {
    let project =
        mica_core::threemf::read_3mf(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut info = Info {
        readable: true,
        objects: project.objects.len() as u32,
        plates: project.plates.len().max(1) as u32,
        ..Default::default()
    };
    let mut used_tools = BTreeSet::new();
    let mut painted = false;
    for o in &project.objects {
        let tool = |s: &mica_core::threemf::Settings| {
            s.get("extruder")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|&t| t > 0)
        };
        used_tools.insert(tool(&o.settings).unwrap_or(1));
        for part in &o.parts {
            if let Some(t) = tool(&part.settings) {
                used_tools.insert(t);
            }
            painted |= !part.painting.material.is_empty();
        }
        info.triangles += o
            .parts
            .iter()
            .filter(|p| p.kind == mica_core::threemf::PartKind::Model)
            .map(|p| p.mesh.faces.len() as u64)
            .sum::<u64>()
            * o.instances.iter().filter(|i| i.printable).count().max(1) as u64;
    }
    let mesh = first_plate_mesh(&project);
    info.size_mm = size_of(&mesh);
    if let Some(cfg) = &project.config {
        let values = config_values(&cfg.path, &cfg.text);
        apply_profiles(&mut info, &values);
        let all = colours(&values);
        info.colours = if painted {
            all
        } else {
            used_tools
                .iter()
                .filter_map(|t| all.get(t - 1).cloned())
                .collect()
        };
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    info.sliced = names
        .iter()
        .any(|n| n.starts_with("Metadata/plate_") && n.ends_with(".gcode"));
    if info.sliced
        && let Some(text) = zip_entry(&mut zip, "Metadata/slice_info.config", 16 << 20)
    {
        slice_info(&String::from_utf8_lossy(&text), &mut info);
    }
    // Thumbnails: Orca's plate picture, Prusa's thumbnail, then anything that looks like one.
    let mut candidates: Vec<&String> = names
        .iter()
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            (l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg"))
                && (l.contains("thumbnail") || l.contains("plate_") || l.contains("cover"))
                && !l.contains("_small")
                && !l.contains("no_light")
                && !l.contains("top_")
                && !l.contains("pick_")
        })
        .collect();
    let rank = |n: &str| {
        let l = n.to_ascii_lowercase();
        if l == "metadata/plate_1.png" {
            0
        } else if l == "metadata/thumbnail.png" {
            1
        } else if l.starts_with("metadata/") {
            2
        } else {
            3
        }
    };
    candidates.sort_by_key(|n| (rank(n), n.len()));
    let mut thumbnail = None;
    for name in candidates {
        if let Some(bytes) = zip_entry(&mut zip, name, 16 << 20)
            && let Some(t) = normalise(bytes)
        {
            thumbnail = Some(t);
            break;
        }
    }
    Ok(Extracted { info, thumbnail })
}

// ----- G-code ----------------------------------------------------------------------------

/// `1d 2h 3m 4s` (any subset) in seconds.
pub fn parse_duration(s: &str) -> Option<f64> {
    let mut total = 0.;
    let mut any = false;
    for tok in s.split_whitespace() {
        let (num, unit) = tok.split_at(tok.find(|c: char| c.is_ascii_alphabetic())?);
        let n: f64 = num.parse().ok()?;
        total += n * match unit {
            "d" => 86400.,
            "h" => 3600.,
            "m" => 60.,
            "s" => 1.,
            _ => return None,
        };
        any = true;
    }
    any.then_some(total)
}

fn sum_list(v: &str) -> Option<(f64, Vec<bool>)> {
    let parts: Vec<f64> = v
        .split([',', ';'])
        .map(|s| s.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    Some((parts.iter().sum(), parts.iter().map(|&x| x > 0.).collect()))
}

/// Turns G-code metadata key/values into the library's facts.
fn gcode_info(values: &HashMap<String, String>, thumbs: Vec<Vec<u8>>) -> Extracted {
    let mut info = Info {
        readable: true,
        sliced: true,
        ..Default::default()
    };
    apply_profiles(&mut info, values);
    let get = |k: &str| values.get(k).map(|s| s.trim());
    info.print_time_s = get("estimated printing time (normal mode)")
        .or(get("total estimated time"))
        .or(get("model printing time"))
        .and_then(parse_duration);
    let grams = get("total filament used [g]")
        .or(get("total filament weight [g]"))
        .or(get("filament used [g]"))
        .and_then(sum_list);
    info.filament_g = grams.as_ref().map(|g| g.0).filter(|g| *g > 0.);
    info.filament_m = get("filament used [mm]")
        .or(get("total filament length [mm]"))
        .and_then(sum_list)
        .map(|m| m.0 / 1000.)
        .filter(|m| *m > 0.);
    let all = colours(values);
    let used = get("filament used [g]")
        .or(get("filament used [mm]"))
        .and_then(sum_list)
        .map(|s| s.1);
    info.colours = match used {
        Some(used) if used.len() > 1 => all
            .into_iter()
            .zip(used)
            .filter_map(|(c, u)| u.then_some(c))
            .collect(),
        _ => all.into_iter().take(1).collect(),
    };
    // The largest embedded picture.
    let thumbnail = thumbs
        .into_iter()
        .filter_map(normalise)
        .max_by_key(|(b, _)| b.len());
    Extracted { info, thumbnail }
}

fn read_window(path: &Path, data: Option<&[u8]>) -> Result<(Vec<u8>, Vec<u8>), String> {
    if let Some(d) = data {
        let n = GCODE_WINDOW as usize;
        let head = d[..d.len().min(n)].to_vec();
        let tail = if d.len() > n {
            d[d.len().saturating_sub(n).max(n)..].to_vec()
        } else {
            vec![]
        };
        return Ok((head, tail));
    }
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut head = vec![];
    (&mut f)
        .take(GCODE_WINDOW)
        .read_to_end(&mut head)
        .map_err(|e| e.to_string())?;
    let mut tail = vec![];
    if len > GCODE_WINDOW {
        f.seek(SeekFrom::Start(
            len.saturating_sub(GCODE_WINDOW).max(GCODE_WINDOW),
        ))
        .map_err(|e| e.to_string())?;
        f.read_to_end(&mut tail).map_err(|e| e.to_string())?;
    }
    Ok((head, tail))
}

fn gcode_text(path: &Path, data: Option<&[u8]>) -> Result<Extracted, String> {
    let (head, tail) = read_window(path, data)?;
    let mut values = HashMap::new();
    let mut thumbs = vec![];
    for chunk in [&head, &tail] {
        let text = String::from_utf8_lossy(chunk);
        let mut in_thumb: Option<String> = None;
        for line in text.lines() {
            let Some(c) = line.strip_prefix(';') else {
                continue;
            };
            let c = c.trim();
            if let Some(b64) = &mut in_thumb {
                if c.starts_with("thumbnail") && c.ends_with(" end") {
                    if let Some(bytes) = base64_decode(b64) {
                        thumbs.push(bytes);
                    }
                    in_thumb = None;
                } else {
                    b64.push_str(c);
                }
                continue;
            }
            if c.starts_with("thumbnail") && c.contains(" begin") {
                in_thumb = Some(String::new());
                continue;
            }
            // `key = value`, Orca's `key: value` and its `a: x; b: y` header lines.
            for part in c.split("; ") {
                if let Some((k, v)) = part.split_once(" = ").or_else(|| part.split_once(": ")) {
                    values
                        .entry(k.trim().to_string())
                        .or_insert_with(|| v.trim().to_string());
                }
            }
        }
    }
    if values.is_empty() && thumbs.is_empty() {
        let mut info = Info {
            readable: true,
            sliced: true,
            ..Default::default()
        };
        info.error = Some("no slicer metadata found".into());
        return Ok(Extracted {
            info,
            thumbnail: None,
        });
    }
    Ok(gcode_info(&values, thumbs))
}

fn bgcode(path: &Path, data: Option<&[u8]>) -> Result<Extracted, String> {
    use mica_gcode::bgcode::{Metadata, stream::Block};
    let mut values = HashMap::new();
    let mut thumbs = vec![];
    let mut visit = |b: Block| {
        match b {
            Block::Metadata(_, Metadata::Ini(entries)) => {
                for (k, v) in entries {
                    values.entry(k).or_insert(v);
                }
            }
            Block::Metadata(_, Metadata::Json(_)) => {}
            Block::Thumbnail(t) => thumbs.push(t.data),
            // The metadata and thumbnails come first; stop at the first G-code block.
            Block::Gcode(_) => return Err(mica_gcode::bgcode::Error::Invalid("stop")),
        }
        Ok(())
    };
    let limit = 64 << 20;
    let r = match data {
        Some(d) => mica_gcode::bgcode::stream::visit(d, limit, &mut visit),
        None => mica_gcode::bgcode::stream::visit(
            std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?),
            limit,
            &mut visit,
        ),
    };
    match r {
        Ok(_) | Err(mica_gcode::bgcode::Error::Invalid("stop")) => {}
        // A damaged block after the metadata: keep what was read.
        Err(_) if !values.is_empty() => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(gcode_info(&values, thumbs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1h 2m 3s"), Some(3723.));
        assert_eq!(parse_duration("1d 0h 0m 10s"), Some(86410.));
        assert_eq!(parse_duration("45s"), Some(45.));
        assert_eq!(parse_duration("soon"), None);
    }

    #[test]
    fn obj_fans_polygons() {
        let m =
            parse_obj(b"v 0 0 0\nv 10 0 0\nv 10 5 0\nv 0 5 2\nf 1 2 3 4\nf -1 -2 -3\n").unwrap();
        assert_eq!(m.faces.len(), 3);
        assert_eq!(size_of(&m), Some([10., 5., 2.]));
        assert!(parse_obj(b"v 0 0 0\nf 1 2 3\n").is_err());
    }

    #[test]
    fn text_gcode_metadata_and_thumbnail() {
        let g = crate::testkit::sample_gcode(
            "Prusa CORE One HF0.4 nozzle",
            &["#FF0000", "#00FF00"],
            &[3.0, 1.5],
        );
        let e = extract(Kind::Gcode, Path::new("x.gcode"), Some(&g));
        let i = &e.info;
        assert!(i.readable && i.sliced, "{i:?}");
        assert_eq!(i.printer.as_deref(), Some("Prusa CORE One HF0.4 nozzle"));
        assert_eq!(i.print_time_s, Some(3723.));
        assert_eq!(i.filament_g, Some(4.5));
        assert_eq!(i.colours, ["#FF0000", "#00FF00"]);
        assert!(i.multi_colour());
        assert_eq!(e.thumbnail.unwrap().1, "png");
    }

    #[test]
    fn binary_gcode_metadata_and_thumbnail() {
        let g = crate::testkit::sample_bgcode("Original Prusa XL", 754.);
        let e = extract(Kind::Bgcode, Path::new("x.bgcode"), Some(&g));
        assert_eq!(
            e.info.printer.as_deref(),
            Some("Original Prusa XL"),
            "{:?}",
            e.info
        );
        assert_eq!(e.info.print_time_s, Some(754.));
        assert!(e.thumbnail.is_some());
    }

    #[test]
    fn step_is_listed_by_name_only() {
        let e = extract(Kind::Step, Path::new("x.step"), Some(b"ISO-10303-21;"));
        assert!(!e.info.readable && e.thumbnail.is_none());
    }

    #[test]
    fn three_mf_projects() {
        let root = crate::testkit::repo_root();
        let orca = std::fs::read(root.join("tests/models/plates-orca.3mf")).unwrap();
        let e = extract(Kind::ThreeMf, Path::new("p.3mf"), Some(&orca));
        assert!(e.info.readable, "{:?}", e.info);
        assert!(e.info.plates >= 2, "{:?}", e.info);
        assert!(e.info.objects >= 1);
        assert!(e.info.size_mm.is_some());
        // No embedded picture: the background renderer draws the first plate later.
        assert!(e.thumbnail.is_none() && !e.info.thumbnail_rendered);
        assert!(crate::render::needs_render(Kind::ThreeMf, &e.info));
        let plate = crate::render::load_mesh(Kind::ThreeMf, &orca).unwrap();
        let (lo, hi) = plate.bounds();
        let size = e.info.size_mm.unwrap();
        assert!((0..3).all(|i| (hi[i] - lo[i] - size[i]).abs() < 1e-3));
        let painted = std::fs::read(root.join("tests/models/painted-cube.3mf")).unwrap();
        let e = extract(Kind::ThreeMf, Path::new("c.3mf"), Some(&painted));
        assert!(e.info.readable, "{:?}", e.info);
        assert!(
            crate::render::thumbnail_png(
                &crate::render::load_mesh(Kind::ThreeMf, &painted).unwrap()
            )
            .is_ok()
        );
    }
}
