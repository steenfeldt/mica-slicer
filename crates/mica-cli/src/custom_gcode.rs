// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Per-layer custom G-code on the command line: `--color-change`, `--tool-change`, `--pause`,
//! `--custom-gcode` and `--template-gcode`, added to whatever the project already carries.

use mica_core::config::Config;
use mica_core::custom_gcode::{COLOR_PRINT_COLORS, Info, Item, Kind, Mode};

pub(crate) const USAGE: &str = "\
  --color-change Z[:N]    color change (M600) at height Z mm, for tool N (default 1); repeatable
  --tool-change Z:N       switch the plate to tool N from height Z (multi-tool printers)
  --pause Z[:message]     pause at height Z, showing the message on the printer
  --custom-gcode Z:text   insert G-code at height Z (\\n separates lines)
  --template-gcode Z      insert the printer's template custom G-code at height Z";

fn height(z: &str, option: &str) -> Result<f64, String> {
    let z: f64 = z
        .trim()
        .parse()
        .map_err(|_| format!("{option} expects a height in mm, got '{z}'"))?;
    if !z.is_finite() || z <= 0. {
        return Err(format!("{option} height must be above 0 mm"));
    }
    Ok(z)
}

fn tool(n: &str, option: &str) -> Result<usize, String> {
    match n.trim().parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(format!("{option} expects a tool number from 1, got '{n}'")),
    }
}

/// Parse one option's value; `None` when `option` is not a custom G-code option.
pub(crate) fn parse(option: &str, value: &str) -> Option<Result<Item, String>> {
    let (z, rest) = match value.split_once(':') {
        Some((z, rest)) => (z, Some(rest)),
        None => (value, None),
    };
    Some((|| match option {
        "--color-change" => Ok(Item::new(
            height(z, option)?,
            Kind::ColorChange,
            rest.map(|n| tool(n, option)).transpose()?.unwrap_or(1),
        )),
        "--tool-change" => Ok(Item::new(
            height(z, option)?,
            Kind::ToolChange,
            tool(rest.ok_or("--tool-change expects Z:N")?, option)?,
        )),
        "--pause" => Ok(Item {
            extra: rest.unwrap_or("").to_string(),
            ..Item::new(height(z, option)?, Kind::PausePrint, 1)
        }),
        "--custom-gcode" => Ok(Item {
            extra: rest
                .filter(|t| !t.trim().is_empty())
                .ok_or("--custom-gcode expects Z:text")?
                .replace("\\n", "\n"),
            ..Item::new(height(z, option)?, Kind::Custom, 1)
        }),
        "--template-gcode" => Ok(Item::new(height(z, option)?, Kind::Template, 1)),
        _ => Err(String::new()),
    })())
    .filter(|r| !matches!(r, Err(e) if e.is_empty()))
}

/// The project's items with the command line's added (an item at the same height replaces the
/// project's). Colour and tool changes get the colours PrusaSlicer uses for converted projects
/// or the tool's filament colour. A mode is chosen when the project has none.
pub(crate) fn merge(
    mut info: Info,
    added: &[Item],
    config: &Config,
    tools: usize,
    object_tools: usize,
) -> Info {
    let colours = super::project_settings::colours(config);
    for (i, item) in added.iter().enumerate() {
        let mut item = item.clone();
        match item.kind {
            Kind::ColorChange => item.color = COLOR_PRINT_COLORS[(i + 1) % 7].into(),
            Kind::ToolChange => {
                item.color = colours
                    .get(item.extruder - 1)
                    .cloned()
                    .unwrap_or_else(|| COLOR_PRINT_COLORS[(i + 1) % 7].into())
            }
            _ => {}
        }
        info.put(item);
    }
    if !added.is_empty() && info.mode == Mode::Undef {
        info.mode = Mode::of_print(tools, object_tools);
    }
    info.check_mode();
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse_heights_tools_and_text() {
        let c = parse("--color-change", "5").unwrap().unwrap();
        assert_eq!((c.print_z, c.kind, c.extruder), (5.0, Kind::ColorChange, 1));
        let t = parse("--tool-change", "2.4:3").unwrap().unwrap();
        assert_eq!((t.kind, t.extruder), (Kind::ToolChange, 3));
        let p = parse("--pause", "10:Insert magnets: now").unwrap().unwrap();
        assert_eq!(p.extra, "Insert magnets: now");
        let g = parse("--custom-gcode", "3:M117 a\\nG4 S1")
            .unwrap()
            .unwrap();
        assert_eq!(g.extra, "M117 a\nG4 S1");
        assert!(parse("--pause", "-1").unwrap().is_err());
        assert!(parse("--tool-change", "2").unwrap().is_err());
        assert!(parse("--output", "x").is_none());
    }
}
