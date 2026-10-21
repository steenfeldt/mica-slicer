// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _extract_wipe_tower_information_from_archive, _extract_wipe_tower_information_from_archive_legacy
// Changes: Preserve finite per-bed model tower placement and legacy INI fallback without applying embedded printer presets.

use super::xml::{get, real, required};
use super::{RawConfig, ThreeMfError, invalid};
use quick_xml::{Reader, events::Event};
use std::{collections::BTreeMap, io::BufRead};

pub(super) fn parse(reader: impl BufRead) -> Result<BTreeMap<u32, [f64; 3]>, ThreeMfError> {
    let mut reader = Reader::from_reader(reader);
    let mut buf = vec![];
    let mut out = BTreeMap::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e)
                if e.local_name().as_ref() == b"wipe_tower_information" =>
            {
                let mut attrs = super::Settings::new();
                for a in e.attributes() {
                    let a = a.map_err(|e| invalid(e.to_string()))?;
                    attrs.insert(
                        String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                        a.decode_and_unescape_value(reader.decoder())
                            .map_err(|e| invalid(e.to_string()))?
                            .into_owned(),
                    );
                }
                let bed = get(&attrs, "bed_idx")
                    .unwrap_or("0")
                    .parse::<u32>()
                    .map_err(|_| invalid("invalid wipe tower bed index"))?;
                let placement = [
                    real(required(&attrs, "position_x")?)?,
                    real(required(&attrs, "position_y")?)?,
                    real(required(&attrs, "rotation_deg")?)?,
                ];
                if out.insert(bed, placement).is_some() {
                    return Err(invalid("duplicate wipe tower bed index"));
                }
            }
            Event::DocType(_) => return Err(invalid("XML DTDs are unsupported")),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}
pub(super) fn legacy(config: &RawConfig) -> Result<Option<[f64; 3]>, ThreeMfError> {
    if config.path != "Metadata/Slic3r_PE.config" {
        return Ok(None);
    }
    let keys = ["wipe_tower_x", "wipe_tower_y", "wipe_tower_rotation_angle"];
    let mut values = [180., 140., 0.];
    let mut found = false;
    for line in config.text.lines() {
        if let Some((k, v)) = line.trim().trim_start_matches(';').trim().split_once('=')
            && let Some(i) = keys.iter().position(|&key| key == k.trim())
        {
            values[i] = real(v.trim())?;
            found = true;
        }
    }
    Ok(found.then_some(values))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modern_placement_and_legacy_fallback() {
        let p=parse(&b"<wipe_tower_information position_x=\"42\" position_y=\"56\" rotation_deg=\"90\"/>"[..]).unwrap();
        assert_eq!(p[&0], [42., 56., 90.]);
        assert!(parse(&b"<wipe_tower_information position_x=\"NaN\" position_y=\"56\" rotation_deg=\"90\"/>"[..]).is_err());
        let c = RawConfig {
            path: "Metadata/Slic3r_PE.config".into(),
            text: "; wipe_tower_x = 31\n; wipe_tower_rotation_angle = 45".into(),
        };
        assert_eq!(legacy(&c).unwrap(), Some([31., 140., 45.]));
    }
}
