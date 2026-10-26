// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica job requirements checked against read-only host observations. Missing information
//! is an advisory issue, never evidence that a requested tool or sensor exists. The caller
//! decides whether to upload; this module neither changes printer state nor blocks a job.

use crate::{FileFormat, PrinterHost, PrinterState};

#[derive(Clone, Debug)]
pub struct ToolRequirement {
    pub tool: u32,
    pub material: Option<String>,
    pub colour: Option<String>,
}
#[derive(Clone, Debug)]
pub struct JobRequirements {
    pub tools: Vec<ToolRequirement>,
    pub format: FileFormat,
    pub bed_type: Option<String>,
    /// If a job requires a chamber sensor, None in status produces a warning.
    pub needs_chamber_sensor: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    Connection,
    NotIdle,
    MissingTool,
    UnknownTools,
    FileFormat,
    BedType,
    UnknownBed,
    Material,
    Colour,
    UnknownFilament,
    ChamberSensor,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    pub tool: Option<u32>,
    pub message: String,
}
/// Read-only, advisory checks. Even connection/auth failures are warnings to the caller.
pub fn preflight<H: PrinterHost + ?Sized>(host: &H, job: &JobRequirements) -> Vec<Issue> {
    let mut issues = Vec::new();
    // Keep the read order stable: a failed observation stops later reads, retaining warnings
    // already collected. This gives the caller one useful connection diagnostic per run.
    let _ = collect(host, job, &mut issues).map_err(|error| {
        issues.push(issue(IssueKind::Connection, None, error.to_string()));
    });
    issues
}

fn issue(kind: IssueKind, tool: Option<u32>, message: String) -> Issue {
    Issue {
        kind,
        tool,
        message,
    }
}

fn collect<H: PrinterHost + ?Sized>(
    host: &H,
    job: &JobRequirements,
    issues: &mut Vec<Issue>,
) -> crate::Result<()> {
    let status = host.status()?;
    if !matches!(status.state, PrinterState::Idle | PrinterState::Finished) {
        issues.push(issue(
            IssueKind::NotIdle,
            None,
            format!("Printer is {:?}", status.state),
        ));
    }
    if job.needs_chamber_sensor && status.chamber.is_none() {
        issues.push(issue(
            IssueKind::ChamberSensor,
            None,
            "Required chamber sensor is unavailable".into(),
        ));
    }

    let info = host.info()?;
    if !info.formats.contains(&job.format) {
        issues.push(issue(
            IssueKind::FileFormat,
            None,
            format!("Printer does not support {:?}", job.format),
        ));
    }
    check_inventory(info.tools.as_deref(), &job.tools, issues);
    if let Some(wanted) = job.bed_type.as_deref() {
        let warning = match info.bed_type.as_deref() {
            None => Some((
                IssueKind::UnknownBed,
                format!("Confirm the {wanted} plate is installed"),
            )),
            Some(actual) if !actual.eq_ignore_ascii_case(wanted) => Some((
                IssueKind::BedType,
                format!("Job plate is {wanted}; printer reports {actual}"),
            )),
            Some(_) => None,
        };
        if let Some((kind, message)) = warning {
            issues.push(issue(kind, None, message));
        }
    }

    if job
        .tools
        .iter()
        .any(|tool| tool.material.is_some() || tool.colour.is_some())
    {
        let loaded = host.loaded_filaments()?;
        for requirement in &job.tools {
            let filament = loaded.iter().find(|entry| entry.tool == requirement.tool);
            check_attribute(
                requirement.tool,
                IssueKind::Material,
                requirement.material.as_deref(),
                filament.and_then(|entry| entry.material.as_deref()),
                issues,
            );
            check_attribute(
                requirement.tool,
                IssueKind::Colour,
                requirement.colour.as_deref(),
                filament.and_then(|entry| entry.colour.as_deref()),
                issues,
            );
        }
    }
    Ok(())
}

fn check_inventory(
    available: Option<&[u32]>,
    required: &[ToolRequirement],
    issues: &mut Vec<Issue>,
) {
    match available {
        None if !required.is_empty() => issues.push(issue(
            IssueKind::UnknownTools,
            None,
            "Printer did not report its tool inventory".into(),
        )),
        Some(ids) => issues.extend(
            required
                .iter()
                .filter(|tool| !ids.contains(&tool.tool))
                .map(|tool| {
                    issue(
                        IssueKind::MissingTool,
                        Some(tool.tool),
                        format!("Printer has no T{}", tool.tool),
                    )
                }),
        ),
        None => {}
    }
}

fn check_attribute(
    tool: u32,
    kind: IssueKind,
    wanted: Option<&str>,
    actual: Option<&str>,
    issues: &mut Vec<Issue>,
) {
    let Some(wanted) = wanted else { return };
    let (kind, message) = match actual {
        Some(actual) if same(actual, wanted, kind) => return,
        Some(actual) => (kind, format!("T{tool} requires {wanted}; loaded {actual}")),
        None => (
            IssueKind::UnknownFilament,
            format!("T{tool} loaded {kind:?} is unknown"),
        ),
    };
    issues.push(issue(kind, Some(tool), message));
}
/// Materials compare by name; colors by eye (CIEDE2000 within the tolerance), so `#FFFFFF`
/// loaded for a `#F5F5F5` project is no issue.
fn same(actual: &str, wanted: &str, kind: IssueKind) -> bool {
    if kind == IssueKind::Colour
        && let (Some(a), Some(w)) = (
            mica_core::color::parse_hex(actual),
            mica_core::color::parse_hex(wanted),
        )
    {
        return crate::colour::same_colour(a, w);
    }
    normalized(actual, kind) == normalized(wanted, kind)
}

fn normalized(value: &str, kind: IssueKind) -> String {
    let value = value.trim().to_ascii_uppercase();
    if kind == IssueKind::Colour {
        let v = value.trim_start_matches('#');
        // Snapmaker's RGBA and RGB encode the same opaque colour.
        if v.len() == 8 && v.ends_with("FF") {
            return v[..6].into();
        }
        return v.into();
    }
    value
}
