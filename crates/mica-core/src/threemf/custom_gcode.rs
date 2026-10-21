// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer::_extract_custom_gcode_per_print_z_from_archive, _3MF_Exporter::_add_custom_gcode_per_print_z_file_to_archive
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_extract_custom_gcode_per_print_z_from_archive, _BBS_3MF_Exporter::_add_custom_gcode_per_print_z_file_to_archive
// Changes: Streaming quick-xml reader for both files (PrusaSlicer's several bed roots, Orca's plates); items are sorted by
// Changes: height; the writers emit the same elements and attributes, including the legacy `gcode` attribute.

//! Per-layer custom G-code in 3MF projects: PrusaSlicer's
//! `Metadata/Prusa_Slicer_custom_gcode_per_print_z.xml` (one `custom_gcodes_per_print_z` element
//! per bed) and Orca's `Metadata/custom_gcode_per_layer.xml` (one `plate` per plate). Both are
//! keyed here by zero-based plate (bed) index.

use super::{Settings, ThreeMfError, invalid};
use crate::custom_gcode::{Info, Item, Kind, Mode};
use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

pub(super) const PRUSA_FILE: &str = "Metadata/Prusa_Slicer_custom_gcode_per_print_z.xml";
pub(super) const ORCA_FILE: &str = "Metadata/custom_gcode_per_layer.xml";

fn attributes(
    e: &quick_xml::events::BytesStart<'_>,
    reader: &Reader<impl BufRead>,
) -> Result<Settings, ThreeMfError> {
    let mut attrs = Settings::new();
    for a in e.attributes() {
        let a = a.map_err(|e| invalid(e.to_string()))?;
        attrs.insert(
            String::from_utf8_lossy(a.key.as_ref()).into_owned(),
            a.decode_and_unescape_value(reader.decoder())
                .map_err(|e| invalid(e.to_string()))?
                .into_owned(),
        );
    }
    Ok(attrs)
}

/// One `code` (Prusa) or `layer` (Orca) element; `z_key` names the height attribute.
fn item(attrs: &Settings, z_key: &str) -> Result<Item, ThreeMfError> {
    let get = |k: &str| attrs.get(k).map(String::as_str);
    let print_z: f64 = get(z_key)
        .ok_or_else(|| invalid(format!("custom G-code without {z_key}")))?
        .trim()
        .parse()
        .map_err(|_| invalid("invalid custom G-code height"))?;
    if !print_z.is_finite() {
        return Err(invalid("non-finite custom G-code height"));
    }
    let extruder = get("extruder")
        .unwrap_or("1")
        .trim()
        .parse::<i64>()
        .map_err(|_| invalid("invalid custom G-code extruder"))?
        .max(0) as usize;
    let color = get("color").unwrap_or("").to_string();
    let (kind, extra) = match get("type") {
        // Projects of PrusaSlicer 2.2.0 and older stored the G-code instead of the type.
        None => {
            let gcode = get("gcode").unwrap_or("");
            let kind = Kind::from_legacy_gcode(gcode);
            let extra = match kind {
                Kind::PausePrint => color.clone(),
                Kind::Custom => gcode.to_string(),
                _ => String::new(),
            };
            (kind, extra)
        }
        Some(t) => (
            t.trim()
                .parse::<i64>()
                .ok()
                .and_then(Kind::from_index)
                .ok_or_else(|| invalid(format!("unknown custom G-code type {t:?}")))?,
            get("extra").unwrap_or("").to_string(),
        ),
    };
    Ok(Item {
        print_z,
        kind,
        extruder,
        color,
        extra,
    })
}

fn finish(mut out: BTreeMap<u32, Info>) -> BTreeMap<u32, Info> {
    for info in out.values_mut() {
        info.sort();
    }
    out
}

/// PrusaSlicer: `custom_gcodes_per_print_z` roots (several, one per bed, `bed_idx` defaulting to
/// 0) holding `code` items and a `mode`.
pub(super) fn parse_prusa(reader: impl BufRead) -> Result<BTreeMap<u32, Info>, ThreeMfError> {
    let mut reader = Reader::from_reader(reader);
    let mut buf = vec![];
    let mut out: BTreeMap<u32, Info> = BTreeMap::new();
    let mut bed: Option<u32> = None;
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name().as_ref().to_vec();
                let attrs = attributes(&e, &reader)?;
                match name.as_slice() {
                    b"custom_gcodes_per_print_z" => {
                        let index = attrs
                            .get("bed_idx")
                            .map(|v| v.trim().parse::<u32>())
                            .transpose()
                            .map_err(|_| invalid("invalid custom G-code bed index"))?
                            .unwrap_or(0);
                        out.entry(index).or_default();
                        bed = Some(index);
                    }
                    b"code" => {
                        if let Some(b) = bed {
                            let item = item(&attrs, "print_z")?;
                            out.entry(b).or_default().items.push(item);
                        }
                    }
                    b"mode" => {
                        if let (Some(b), Some(v)) = (bed, attrs.get("value")) {
                            out.entry(b).or_default().mode = Mode::from_name(v);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"custom_gcodes_per_print_z" => bed = None,
            Event::DocType(_) => return Err(invalid("XML DTDs are unsupported")),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(finish(out))
}

/// Orca: a `custom_gcodes_per_layer` root with `plate` elements (`plate_info id` one based,
/// `layer` items, `mode`); files without plates hold the items of plate 1 directly.
pub(super) fn parse_orca(reader: impl BufRead) -> Result<BTreeMap<u32, Info>, ThreeMfError> {
    let mut reader = Reader::from_reader(reader);
    let mut buf = vec![];
    let mut out: BTreeMap<u32, Info> = BTreeMap::new();
    // Items of the plate being read, until its id is known.
    let mut current: Option<(Option<u32>, Info)> = None;
    let mut root = Info::default();
    let mut seen_root = false;
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name().as_ref().to_vec();
                let attrs = attributes(&e, &reader)?;
                match name.as_slice() {
                    b"custom_gcodes_per_layer" => seen_root = true,
                    b"plate" => current = Some((None, Info::default())),
                    b"plate_info" => {
                        if let Some((id, _)) = &mut current {
                            *id = attrs.get("id").and_then(|v| v.trim().parse::<u32>().ok());
                        }
                    }
                    b"layer" => {
                        let item = item(&attrs, "top_z")?;
                        match &mut current {
                            Some((_, info)) => info.items.push(item),
                            None => root.items.push(item),
                        }
                    }
                    b"mode" => {
                        if let Some(v) = attrs.get("value") {
                            match &mut current {
                                Some((_, info)) => info.mode = Mode::from_name(v),
                                None => root.mode = Mode::from_name(v),
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"plate" => {
                if let Some((Some(id), info)) = current.take()
                    && id >= 1
                {
                    out.insert(id - 1, info);
                }
            }
            Event::DocType(_) => return Err(invalid("XML DTDs are unsupported")),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !seen_root {
        return Ok(BTreeMap::new());
    }
    if out.is_empty() && (!root.items.is_empty() || root.mode != Mode::Undef) {
        out.insert(0, root);
    }
    Ok(finish(out))
}

fn attr(s: &str) -> String {
    quick_xml::escape::escape(s)
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
        .replace('\t', "&#9;")
}

/// The pre-2.2 `gcode` attribute both slicers still write next to `type`.
fn legacy_gcode(item: &Item, templates: &Templates) -> String {
    match item.kind {
        Kind::ColorChange => templates.color_change.clone(),
        Kind::PausePrint => templates.pause.clone(),
        Kind::Template => templates.template.clone(),
        Kind::ToolChange => "tool_change".into(),
        Kind::Custom => item.extra.clone(),
    }
}

/// The printer templates written into the legacy `gcode` attribute.
#[derive(Clone, Debug)]
pub struct Templates {
    pub color_change: String,
    pub pause: String,
    pub template: String,
}
impl Default for Templates {
    fn default() -> Self {
        Self {
            color_change: "M600".into(),
            pause: "M601".into(),
            template: String::new(),
        }
    }
}

fn line(
    w: &mut impl Write,
    element: &str,
    z_key: &str,
    item: &Item,
    t: &Templates,
) -> std::io::Result<()> {
    writeln!(
        w,
        "<{element} {z_key}=\"{}\" type=\"{}\" extruder=\"{}\" color=\"{}\" extra=\"{}\" gcode=\"{}\"/>",
        item.print_z,
        item.kind.index(),
        item.extruder,
        attr(&item.color),
        attr(&item.extra),
        attr(&legacy_gcode(item, t)),
    )
}

/// PrusaSlicer's file: bed 0 always (so older slicers know it has no changes), other beds when
/// they have items.
pub(super) fn write_prusa(
    w: &mut impl Write,
    plates: &BTreeMap<u32, Info>,
    t: &Templates,
) -> std::io::Result<()> {
    writeln!(w, "<?xml version=\"1.0\" encoding=\"utf-8\"?>")?;
    let empty = Info::default();
    let first = plates.get(&0).unwrap_or(&empty);
    for (bed, info) in std::iter::once((0, first)).chain(
        plates
            .iter()
            .filter(|(b, i)| **b != 0 && !i.items.is_empty())
            .map(|(b, i)| (*b, i)),
    ) {
        writeln!(w, "<custom_gcodes_per_print_z bed_idx=\"{bed}\">")?;
        for item in &info.items {
            line(w, "code", "print_z", item, t)?;
        }
        writeln!(w, "<mode value=\"{}\"/>", info.mode.name())?;
        writeln!(w, "</custom_gcodes_per_print_z>")?;
    }
    Ok(())
}

/// Orca's file: one `plate` per plate with items.
pub(super) fn write_orca(
    w: &mut impl Write,
    plates: &BTreeMap<u32, Info>,
    t: &Templates,
) -> std::io::Result<()> {
    writeln!(w, "<?xml version=\"1.0\" encoding=\"utf-8\"?>")?;
    writeln!(w, "<custom_gcodes_per_layer>")?;
    for (plate, info) in plates.iter().filter(|(_, i)| !i.items.is_empty()) {
        writeln!(w, "<plate>")?;
        writeln!(w, "<plate_info id=\"{}\"/>", plate + 1)?;
        for item in &info.items {
            line(w, "layer", "top_z", item, t)?;
        }
        writeln!(w, "<mode value=\"{}\"/>", info.mode.name())?;
        writeln!(w, "</plate>")?;
    }
    writeln!(w, "</custom_gcodes_per_layer>")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BTreeMap<u32, Info> {
        let mut a = Info {
            mode: Mode::SingleExtruder,
            ..Info::default()
        };
        a.put(Item {
            color: "#E67E22".into(),
            ..Item::new(5.0, Kind::ColorChange, 1)
        });
        a.put(Item {
            extra: "Insert magnets & \"nuts\"".into(),
            ..Item::new(10.2, Kind::PausePrint, 1)
        });
        a.put(Item {
            extra: "M117 hello\nG4 S1".into(),
            ..Item::new(12.4, Kind::Custom, 1)
        });
        let mut b = Info {
            mode: Mode::MultiAsSingle,
            ..Info::default()
        };
        b.put(Item::new(2.0, Kind::ToolChange, 3));
        BTreeMap::from([(0, a), (2, b)])
    }

    #[test]
    fn prusa_file_round_trips() {
        let mut text = Vec::new();
        write_prusa(&mut text, &sample(), &Templates::default()).unwrap();
        let back = parse_prusa(&text[..]).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn orca_file_round_trips() {
        let mut text = Vec::new();
        write_orca(&mut text, &sample(), &Templates::default()).unwrap();
        let back = parse_orca(&text[..]).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn upstream_files_and_legacy_items_parse() {
        // As PrusaSlicer 2.9 writes it (boost property tree), two beds.
        let prusa = br##"<?xml version="1.0" encoding="utf-8"?>
<custom_gcodes_per_print_z bed_idx="0">
<code print_z="5" type="0" extruder="1" color="#E67E22" extra="" gcode="M600"/>
<code print_z="10" type="1" extruder="1" color="" extra="Magnets" gcode="M601"/>
<mode value="SingleExtruder"/>
</custom_gcodes_per_print_z>
<custom_gcodes_per_print_z bed_idx="1">
<code print_z="0.6" gcode="M600" extruder="1" color="#000000"/>
<mode value="SingleExtruder"/>
</custom_gcodes_per_print_z>"##;
        let p = parse_prusa(&prusa[..]).unwrap();
        assert_eq!(p[&0].items.len(), 2);
        assert_eq!(p[&0].items[1].extra, "Magnets");
        assert_eq!(p[&1].items[0].kind, Kind::ColorChange);
        let orca = br##"<?xml version="1.0" encoding="utf-8"?>
<custom_gcodes_per_layer>
<plate>
<plate_info id="2"/>
<layer top_z="1.4" type="2" extruder="2" color="#FF0000" extra="" gcode="tool_change"/>
<mode value="MultiAsSingle"/>
</plate>
</custom_gcodes_per_layer>"##;
        let o = parse_orca(&orca[..]).unwrap();
        assert_eq!(o.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(o[&1].mode, Mode::MultiAsSingle);
        assert_eq!(o[&1].items[0].extruder, 2);
    }
}
