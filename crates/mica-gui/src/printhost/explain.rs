// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Plain language for what the printer said: backend errors become a headline plus what to
//! do, and the preflight result becomes the rows the Print dialog shows (idle or busy, tools
//! present, the job's filament next to the loaded one per tool, the file format).

use super::config::Kind;
use mica_printhost::{
    Error, ErrorKind, FileFormat, Issue, IssueKind, LoadedFilament, PrinterInfo, PrinterState,
    PrinterStatus,
};

/// A readable error: one line of what happened and one of what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub headline: String,
    pub advice: String,
}

/// Where the API key lives, per backend.
fn key_location(kind: Kind) -> &'static str {
    match kind {
        Kind::PrusaConnect => {
            "Prusa Connect → select the printer → Settings → API key (the Connect key, not the LAN key)"
        }
        Kind::PrusaLink => "the printer's Settings → Network → PrusaLink → API key",
        Kind::Moonraker | Kind::SnapmakerU1 => {
            "Moonraker's configuration ([authorization] / the API key page of Fluidd or Mainsail)"
        }
    }
}

/// Explains a backend error for printer `name` of `kind` at `address`.
pub fn explain(e: &Error, kind: Kind, name: &str, address: &str) -> Explanation {
    if kind == Kind::PrusaConnect {
        return Explanation {
            headline: format!("Prusa Connect: {}{}", e.http_status.map(|s| format!("HTTP {s}: ")).unwrap_or_default(), e.message),
            advice: match e.kind {
                ErrorKind::Unauthorised => format!("Check the per-printer API key in My printers against {}.", key_location(kind)),
                ErrorKind::Busy => "Connect refused this request. Check the printer and queue in Prusa Connect before retrying.".into(),
                ErrorKind::Unreachable => "Check your internet connection. If the upload had begun, check Prusa Connect before retrying to avoid a duplicate job.".into(),
                ErrorKind::Protocol => "Check Prusa Connect before retrying; the service may have changed or rejected the request.".into(),
            },
        };
    }
    let status = e.http_status.map(|s| format!("{s}: ")).unwrap_or_default();
    match e.kind {
        ErrorKind::Unauthorised => Explanation {
            headline: format!("{status}{name} refused the credentials"),
            advice: format!(
                "Check the {} in My printers against {}.",
                if kind == Kind::PrusaLink {
                    "API key (or user name and password)"
                } else {
                    "API key"
                },
                key_location(kind)
            ),
        },
        ErrorKind::Unreachable => Explanation {
            headline: format!("Mica could not reach {name} at {address}"),
            advice: "Check that the printer is on and on the same network, and that the address \
                     (and port) is right. Firewalls and VPNs can block it too."
                .into(),
        },
        ErrorKind::Busy => Explanation {
            headline: format!("{status}{name} is busy and did not accept the request"),
            advice:
                "Wait for the current job to finish, or pause / cancel it in the Machine view, \
                     then try again."
                    .into(),
        },
        ErrorKind::Protocol => Explanation {
            headline: format!("{status}{name} answered in a way Mica did not expect"),
            advice: format!(
                "{}. Check the printer type ({}) and the address: a PrusaLink address is the \
                 printer itself, a Klipper address is the Moonraker port (usually 7125).",
                capitalise(&e.message),
                kind.label()
            ),
        },
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A preflight issue as a sentence for people.
pub fn issue_text(issue: &Issue) -> String {
    let tool = |t: Option<u32>| t.map(|t| format!("T{}", t + 1)).unwrap_or_default();
    match issue.kind {
        IssueKind::Connection => format!("Could not check the printer: {}", issue.message),
        IssueKind::NotIdle => "The printer is not idle; the upload works, but printing can only \
                               start once it is free."
            .into(),
        IssueKind::MissingTool => format!(
            "The job uses {} but the printer has no such tool.",
            tool(issue.tool)
        ),
        IssueKind::UnknownTools => "The printer did not report which tools it has.".into(),
        IssueKind::FileFormat => "The printer does not accept this file format; export the other \
                                  G-code flavor for it."
            .into(),
        IssueKind::BedType => issue.message.clone(),
        IssueKind::UnknownBed => issue.message.clone(),
        IssueKind::Material => format!(
            "{}: the job wants another material than the one loaded.",
            tool(issue.tool)
        ),
        IssueKind::Colour => format!(
            "{}: the loaded color differs from the project's.",
            tool(issue.tool)
        ),
        IssueKind::UnknownFilament => format!(
            "{}: the printer does not know what is loaded.",
            tool(issue.tool)
        ),
        IssueKind::ChamberSensor => "The job needs a chamber sensor this printer lacks.".into(),
    }
}

/// What the job needs of one tool, from the sliced plate.
#[derive(Clone, Debug, PartialEq)]
pub struct JobTool {
    /// Zero based.
    pub tool: u32,
    pub preset: String,
    pub material: Option<String>,
    pub colour: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Material and colour (where both sides know them) agree.
    Match,
    /// The printer reports something different.
    Differs,
    /// The printer does not report what is loaded.
    Unknown,
    /// The printer has no such tool.
    Missing,
}

/// One row of the filament comparison: the job's side next to the printer's.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub tool: u32,
    pub job: JobTool,
    pub loaded_material: Option<String>,
    pub loaded_colour: Option<[u8; 3]>,
    pub verdict: Verdict,
}

/// The preflight as the dialog shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Preflight {
    pub state: PrinterState,
    /// `Idle`, `Printing 3DBenchy.bgcode · 37 %`, …
    pub state_text: String,
    pub ready: bool,
    /// `5 tools (T1–T5)`, `1 tool`, or that the printer did not say.
    pub tools_text: String,
    pub rows: Vec<Row>,
    /// `Binary G-code (.bgcode) · accepted` or the mismatch.
    pub format_text: String,
    pub format_ok: bool,
    /// Plain-language warnings (everything that is not a simple match).
    pub warnings: Vec<String>,
}

/// The printer's side of a filament row: `PLA · yellow`, never a hex code.
pub fn loaded_text(material: Option<&str>, colour: Option<[u8; 3]>) -> String {
    let name = colour.map(mica_printhost::colour::colour_name);
    match (material.map(str::trim).filter(|m| !m.is_empty()), name) {
        (Some(m), Some(c)) => format!("{m} · {c}"),
        (Some(m), None) => m.to_string(),
        (None, Some(c)) => format!("unknown material · {c}"),
        (None, None) => "unknown material".into(),
    }
}

fn same_material(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Builds the presentation from what the backend reported.
pub fn present(
    info: &PrinterInfo,
    status: &PrinterStatus,
    loaded: &[LoadedFilament],
    job: &[JobTool],
    format: FileFormat,
    issues: &[Issue],
) -> Preflight {
    let ready = matches!(status.state, PrinterState::Idle | PrinterState::Finished);
    let state_text = match status.state {
        PrinterState::Idle => "Idle, ready to print".to_string(),
        PrinterState::Finished => "Finished the last job; ready".to_string(),
        PrinterState::Printing | PrinterState::Paused => {
            let verb = if status.state == PrinterState::Printing {
                "Printing"
            } else {
                "Paused"
            };
            let mut s = verb.to_string();
            if let Some(j) = &status.job_name {
                s.push_str(&format!(" {j}"));
            }
            if let Some(p) = status.progress {
                s.push_str(&format!(" · {p:.0} %"));
            }
            s
        }
        PrinterState::Error => "The printer reports an error; see its screen".to_string(),
        PrinterState::Offline => "The printer is offline (firmware not ready)".to_string(),
    };
    let tools_text = match &info.tools {
        Some(t) if t.is_empty() => "No tools reported".to_string(),
        Some(t) if t.len() == 1 => "1 tool".to_string(),
        Some(t) => {
            let mut sorted = t.clone();
            sorted.sort_unstable();
            format!(
                "{} tools (T{}–T{})",
                t.len(),
                sorted[0] + 1,
                sorted[sorted.len() - 1] + 1
            )
        }
        None => "The printer did not report its tools".to_string(),
    };
    let rows: Vec<Row> = job
        .iter()
        .map(|jt| {
            let present = info.tools.as_ref().is_none_or(|t| t.contains(&jt.tool));
            let f = loaded.iter().find(|f| f.tool == jt.tool);
            let loaded_material = f.and_then(|f| f.material.clone());
            let loaded_colour = f
                .and_then(|f| f.colour.as_deref())
                .and_then(mica_core::color::parse_hex);
            let verdict = if !present {
                Verdict::Missing
            } else if f.is_none() || (loaded_material.is_none() && loaded_colour.is_none()) {
                Verdict::Unknown
            } else {
                let material_ok = match (&jt.material, &loaded_material) {
                    (Some(a), Some(b)) => same_material(a, b),
                    _ => true,
                };
                // Colors compare by eye (CIEDE2000 within the tolerance), not by hex code.
                let colour_ok = match (jt.colour, loaded_colour) {
                    (Some(a), Some(b)) => mica_printhost::colour::same_colour(a, b),
                    _ => true,
                };
                if material_ok && colour_ok {
                    Verdict::Match
                } else {
                    Verdict::Differs
                }
            };
            Row {
                tool: jt.tool,
                job: jt.clone(),
                loaded_material,
                loaded_colour,
                verdict,
            }
        })
        .collect();
    let format_ok = info.formats.contains(&format);
    let format_name = match format {
        FileFormat::Bgcode => "Binary G-code (.bgcode)",
        FileFormat::Gcode => "Text G-code (.gcode)",
    };
    let format_text = if format_ok {
        format!("{format_name} · accepted")
    } else {
        format!(
            "{format_name} · not accepted; this printer takes {}",
            info.formats
                .iter()
                .map(|f| match f {
                    FileFormat::Bgcode => ".bgcode",
                    FileFormat::Gcode => ".gcode",
                })
                .collect::<Vec<_>>()
                .join(" / ")
        )
    };
    let mut warnings: Vec<String> = issues
        .iter()
        .filter(|i| !matches!(i.kind, IssueKind::Colour | IssueKind::Material))
        .map(issue_text)
        .collect();
    // Filament differences are summarised once, from the rows (they carry the swatches).
    let differing: Vec<String> = rows
        .iter()
        .filter(|r| r.verdict == Verdict::Differs)
        .map(|r| format!("T{}", r.tool + 1))
        .collect();
    if !differing.is_empty() {
        warnings.push(format!(
            "Loaded filament differs from the project on {}; check the spools before starting.",
            differing.join(", ")
        ));
    }
    warnings.dedup();
    Preflight {
        state: status.state,
        state_text,
        ready,
        tools_text,
        rows,
        format_text,
        format_ok,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_printhost::{Temperature, ToolTemperature};

    fn info(tools: Option<Vec<u32>>, formats: Vec<FileFormat>) -> PrinterInfo {
        PrinterInfo {
            model: Some("Original Prusa XL 5T".into()),
            firmware: Some("6.2.3".into()),
            state: PrinterState::Idle,
            tools,
            formats,
            bed_type: None,
        }
    }
    fn status(state: PrinterState) -> PrinterStatus {
        PrinterStatus {
            state,
            job_name: Some("3DBenchy.bgcode".into()),
            progress: Some(37.),
            elapsed: Some(1333.),
            remaining: Some(2267.),
            current_layer: Some(78),
            z: Some(17.8),
            hotends: vec![ToolTemperature {
                tool: 0,
                temperature: Temperature {
                    current: Some(215.),
                    target: Some(215.),
                },
            }],
            bed: None,
            chamber: None,
        }
    }
    fn loaded(tool: u32, material: &str, colour: &str) -> LoadedFilament {
        LoadedFilament {
            tool,
            material: Some(material.into()),
            colour: Some(colour.into()),
        }
    }
    fn job(tool: u32, material: &str, colour: [u8; 3]) -> JobTool {
        JobTool {
            tool,
            preset: format!("Prusament {material}"),
            material: Some(material.into()),
            colour: Some(colour),
        }
    }

    #[test]
    fn errors_become_a_headline_and_advice() {
        let e = Error {
            kind: ErrorKind::Unauthorised,
            message: "printer returned HTTP 401".into(),
            http_status: Some(401),
        };
        let x = explain(&e, Kind::PrusaLink, "XL", "xl.local");
        assert_eq!(x.headline, "401: XL refused the credentials");
        assert!(x.advice.contains("PrusaLink → API key"));
        let x = explain(&e, Kind::SnapmakerU1, "U1", "u1.local");
        assert!(x.advice.contains("Moonraker"));
        let e = Error {
            kind: ErrorKind::Unreachable,
            message: "printer unreachable or request timed out".into(),
            http_status: None,
        };
        let x = explain(&e, Kind::PrusaLink, "XL", "192.0.2.50");
        assert!(x.headline.contains("could not reach XL at 192.0.2.50"));
        assert!(x.advice.contains("same network"));
        let e = Error {
            kind: ErrorKind::Busy,
            message: "printer returned HTTP 409".into(),
            http_status: Some(409),
        };
        assert!(
            explain(&e, Kind::PrusaLink, "XL", "xl")
                .advice
                .contains("Machine view")
        );
        let e = Error {
            kind: ErrorKind::Protocol,
            message: "invalid JSON response".into(),
            http_status: None,
        };
        let x = explain(&e, Kind::Moonraker, "K", "k:7125");
        assert!(x.advice.starts_with("Invalid JSON response."));
        assert!(x.advice.contains("7125"));
    }

    #[test]
    fn preflight_rows_compare_job_and_loaded_filament() {
        let i = info(
            Some(vec![0, 1, 2, 3, 4]),
            vec![FileFormat::Bgcode, FileFormat::Gcode],
        );
        let s = status(PrinterState::Idle);
        let loaded = vec![
            loaded(0, "PLA", "#FF7D38"),
            loaded(1, "PETG", "#41B4F0"),
            LoadedFilament {
                tool: 2,
                material: None,
                colour: None,
            },
        ];
        let job = vec![
            job(0, "PLA", [0xFF, 0x7D, 0x38]),
            job(1, "PLA", [0x41, 0xB4, 0xF0]),
            job(2, "ASA", [1, 2, 3]),
            job(5, "PLA", [1, 2, 3]),
        ];
        let issues = vec![Issue {
            kind: IssueKind::Material,
            tool: Some(1),
            message: "T1 requires PLA; loaded PETG".into(),
        }];
        let p = present(&i, &s, &loaded, &job, FileFormat::Bgcode, &issues);
        assert!(p.ready);
        assert_eq!(p.state_text, "Idle, ready to print");
        assert_eq!(p.tools_text, "5 tools (T1–T5)");
        assert_eq!(
            p.rows.iter().map(|r| r.verdict).collect::<Vec<_>>(),
            [
                Verdict::Match,
                Verdict::Differs,
                Verdict::Unknown,
                Verdict::Missing
            ]
        );
        assert_eq!(p.rows[1].loaded_colour, Some([0x41, 0xB4, 0xF0]));
        assert_eq!(p.rows[1].loaded_material.as_deref(), Some("PETG"));
        assert!(p.format_ok);
        assert_eq!(p.format_text, "Binary G-code (.bgcode) · accepted");
        // The material issue is folded into the one filament warning, naming the tool as T2.
        assert_eq!(p.warnings.len(), 1);
        assert!(p.warnings[0].contains("T2"), "{:?}", p.warnings);
        // Busy printer, wrong format.
        let i = info(None, vec![FileFormat::Gcode]);
        let s = status(PrinterState::Printing);
        let p = present(&i, &s, &[], &job[..1], FileFormat::Bgcode, &[]);
        assert!(!p.ready);
        assert_eq!(p.state_text, "Printing 3DBenchy.bgcode · 37 %");
        assert_eq!(p.tools_text, "The printer did not report its tools");
        assert!(!p.format_ok);
        assert!(p.format_text.contains("takes .gcode"));
        assert_eq!(p.rows[0].verdict, Verdict::Unknown);
    }

    #[test]
    fn issue_sentences_use_one_based_tools() {
        let i = Issue {
            kind: IssueKind::MissingTool,
            tool: Some(4),
            message: "Printer has no T4".into(),
        };
        assert_eq!(
            issue_text(&i),
            "The job uses T5 but the printer has no such tool."
        );
        let parse_colour = mica_core::color::parse_hex;
        assert_eq!(parse_colour("#41B4F0"), Some([0x41, 0xB4, 0xF0]));
        assert_eq!(parse_colour("FF7D38FF"), Some([0xFF, 0x7D, 0x38]));
        assert_eq!(parse_colour("red"), None);
    }
}
