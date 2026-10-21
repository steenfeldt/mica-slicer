// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Format/3mf.cpp :: _3MF_Importer::_extract_layer_config_ranges_from_archive
// Source: orca@8500fcdcca src/libslic3r/Format/bbs_3mf.cpp :: _BBS_3MF_Importer::_extract_layer_config_ranges_from_archive
// Changes: Streaming reader with finite, nonnegative, non-overlapping bounds; preserve raw option text.
// Upstream attribution (Format/3mf.cpp):
// Copyright (c) Prusa Research 2018 - 2023 Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Enrico Turri @enricoturri1966, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas, Filip Sykala @Jony01, Vojtěch Bubník @bubnikv, Tomáš Mészáros @tamasmeszaros
// Copyright (c) 2020 Henner Zeller
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::{HeightRange, Settings, ThreeMfError, invalid};
use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;
use std::io::BufRead;

pub(super) fn parse(
    reader: impl BufRead,
) -> Result<BTreeMap<usize, Vec<HeightRange>>, ThreeMfError> {
    let mut reader = Reader::from_reader(reader);
    reader.config_mut().expand_empty_elements = true;
    let mut buf = vec![];
    let mut result = BTreeMap::<usize, Vec<HeightRange>>::new();
    let mut stack = Vec::<String>::new();
    let mut object = None;
    let mut range: Option<HeightRange> = None;
    let mut key: Option<String> = None;
    let mut value = String::new();
    let mut root = false;
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs = e
                    .attributes()
                    .map(|a| {
                        let a = a.map_err(|e| invalid(e.to_string()))?;
                        Ok((
                            String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                            a.decode_and_unescape_value(reader.decoder())
                                .map_err(|e| invalid(e.to_string()))?
                                .into_owned(),
                        ))
                    })
                    .collect::<Result<Settings, ThreeMfError>>()?;
                match (stack.last().map(String::as_str), name.as_str()) {
                    (None, "objects") if !root => root = true,
                    (Some("objects"), "object") => {
                        let id: usize = super::xml::integer(&attrs, "id")? as usize;
                        if id == 0 || result.insert(id, vec![]).is_some() {
                            return Err(invalid("invalid or duplicate height range object"));
                        }
                        object = Some(id);
                    }
                    (Some("object"), "range") => {
                        let min_z = super::xml::real(super::xml::required(&attrs, "min_z")?)?;
                        let max_z = super::xml::real(super::xml::required(&attrs, "max_z")?)?;
                        if min_z < 0. || max_z <= min_z {
                            return Err(invalid("invalid height range bounds"));
                        }
                        range = Some(HeightRange {
                            min_z,
                            max_z,
                            settings: Settings::new(),
                        });
                    }
                    (Some("range"), "option") => {
                        key = Some(super::xml::required(&attrs, "opt_key")?.into());
                        value.clear();
                    }
                    _ => return Err(invalid("unexpected height range XML element")),
                }
                stack.push(name);
            }
            Event::Text(t) if key.is_some() => {
                value.push_str(&t.unescape().map_err(|e| invalid(e.to_string()))?)
            }
            Event::CData(t) if key.is_some() => {
                value.push_str(&t.decode().map_err(|e| invalid(e.to_string()))?)
            }
            Event::End(e) => {
                let name = stack
                    .pop()
                    .ok_or_else(|| invalid("unexpected height range closing tag"))?;
                if e.local_name().as_ref() != name.as_bytes() {
                    return Err(invalid("mismatched height range tag"));
                }
                match name.as_str() {
                    "option" => {
                        range
                            .as_mut()
                            .ok_or_else(|| invalid("option without range"))?
                            .settings
                            .insert(
                                key.take().ok_or_else(|| invalid("option has no key"))?,
                                std::mem::take(&mut value),
                            );
                    }
                    "range" => result
                        .get_mut(&object.ok_or_else(|| invalid("range without object"))?)
                        .ok_or_else(|| invalid("unknown range object"))?
                        .push(
                            range
                                .take()
                                .ok_or_else(|| invalid("range without bounds"))?,
                        ),
                    "object" => object = None,
                    _ => {}
                }
            }
            Event::DocType(_) => return Err(invalid("XML DTDs are unsupported")),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !root || !stack.is_empty() {
        return Err(invalid("truncated height range XML"));
    }
    for ranges in result.values_mut() {
        ranges.sort_by(|a, b| a.min_z.total_cmp(&b.min_z));
        if ranges.windows(2).any(|r| r[0].max_z > r[1].min_z) {
            return Err(invalid("overlapping height ranges"));
        }
    }
    Ok(result)
}
