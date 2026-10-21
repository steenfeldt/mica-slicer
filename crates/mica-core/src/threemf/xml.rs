// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::{Settings, ThreeMfError, invalid};
use quick_xml::{Reader, events::Event};
use std::io::BufRead;

/// A small streaming XML adapter; the callback receives local names and the parent stack.
/// Attribute keys retain their prefixes, except `get()` below also accepts a local name.
pub(super) fn walk(
    reader: impl BufRead,
    root: &str,
    mut visit: impl FnMut(&str, Option<&Settings>, &[String]) -> Result<(), ThreeMfError>,
) -> Result<(), ThreeMfError> {
    let mut reader = Reader::from_reader(reader);
    let mut buf = Vec::new();
    let mut stack = Vec::new();
    let mut seen_root = false;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| invalid(format!("XML at byte {}: {e}", reader.error_position())))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = String::from_utf8(e.local_name().as_ref().to_vec())
                    .map_err(|_| invalid("non-UTF8 element name"))?;
                if stack.is_empty() {
                    if seen_root || name != root {
                        return Err(invalid(format!("expected {root} XML root")));
                    }
                    seen_root = true;
                }
                let mut attrs = Settings::new();
                for attr in e.attributes() {
                    let a = attr.map_err(|e| invalid(e.to_string()))?;
                    let key = std::str::from_utf8(a.key.as_ref())
                        .map_err(|_| invalid("non-UTF8 attribute"))?;
                    let value = a
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|e| invalid(e.to_string()))?;
                    attrs.insert(key.to_owned(), value.into_owned());
                }
                visit(&name, Some(&attrs), &stack)?;
                if matches!(event, Event::Empty(_)) {
                    visit(&name, None, &stack)?;
                } else {
                    if stack.len() >= 128 {
                        return Err(invalid("XML nesting exceeds 128"));
                    }
                    stack.push(name);
                }
            }
            Event::End(e) => {
                let name = stack
                    .pop()
                    .ok_or_else(|| invalid("unexpected XML end tag"))?;
                if e.local_name().as_ref() != name.as_bytes() {
                    return Err(invalid("mismatched XML end tag"));
                }
                visit(&name, None, &stack)?;
            }
            Event::DocType(_) => return Err(invalid("XML DTDs are unsupported")),
            Event::Eof => {
                if !seen_root || !stack.is_empty() {
                    return Err(invalid("truncated XML"));
                }
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

pub(super) fn get<'a>(attrs: &'a Settings, key: &str) -> Option<&'a str> {
    attrs
        .get(key)
        .or_else(|| {
            attrs
                .iter()
                .find(|(k, _)| k.rsplit(':').next() == Some(key))
                .map(|(_, v)| v)
        })
        .map(String::as_str)
}
pub(super) fn required<'a>(attrs: &'a Settings, key: &str) -> Result<&'a str, ThreeMfError> {
    get(attrs, key).ok_or_else(|| invalid(format!("missing {key} attribute")))
}
pub(super) fn number<T: std::str::FromStr>(s: &str) -> Result<T, ThreeMfError> {
    s.parse()
        .map_err(|_| invalid(format!("invalid number {s:?}")))
}
pub(super) fn integer(attrs: &Settings, key: &str) -> Result<u32, ThreeMfError> {
    number(required(attrs, key)?)
}
pub(super) fn real(s: &str) -> Result<f64, ThreeMfError> {
    let n: f64 = number(s)?;
    if !n.is_finite() {
        return Err(invalid("non-finite coordinate or transform"));
    }
    Ok(n)
}
