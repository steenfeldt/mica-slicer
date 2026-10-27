// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The project summary: what all sliced plates of a project add up to. Totals (time, weight,
//! length, cost, tool changes, what the tower took), the filaments to have on hand (per tool,
//! with the plates using each), the plates in print order and where the time goes by feature.
//! Only plates whose slice is up to date count ([`PlateStatus::Sliced`]); plates not sliced,
//! changed since, failed or waiting are listed as missing. Every number is the sum of the
//! plates' own slice summaries, which are what each plate's Preview and G-code say.
//! Preview's plate list shows it (`viewport::summary_panel`); [`ProjectSummary::text`] is
//! what Copy summary puts on the clipboard.

use crate::plate_batch::PlateStatus;
use crate::scene::plates::Plate;

/// One filament (tool) over the project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilamentUse {
    /// Zero-based tool.
    pub tool: usize,
    /// Preset name of the filament (as the first plate using it has it).
    pub filament: String,
    /// Its type (PLA, PETG, …).
    pub material: String,
    pub grams: f64,
    pub mm: f64,
    pub cost: f64,
    /// Of `grams`: what the tower took.
    pub tower_grams: f64,
    /// `(plate index, grams, mm)` of the plates using it.
    pub plates: Vec<(usize, f64, f64)>,
}

/// One sliced plate in print order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlateUse {
    pub index: usize,
    /// "Plate 3 · Lid".
    pub title: String,
    pub time_s: f64,
    pub grams: f64,
    /// When it starts: the time of the sliced plates before it.
    pub start_s: f64,
    /// Zero-based tools it prints with.
    pub tools: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectSummary {
    /// Plates with something to slice.
    pub plates: usize,
    pub sliced: usize,
    /// Plates with objects that do not count: `(plate index, title)`.
    pub missing: Vec<(usize, String)>,
    pub time_s: f64,
    pub grams: f64,
    pub mm: f64,
    pub cost: f64,
    pub tool_changes: usize,
    /// The tower's name ("Prime tower", "Wipe tower") when a plate prints one, and what all
    /// towers took.
    pub tower: Option<String>,
    pub tower_grams: f64,
    pub tower_mm: f64,
    /// By tool.
    pub filaments: Vec<FilamentUse>,
    /// The sliced plates in plate order.
    pub timeline: Vec<PlateUse>,
    /// `(legend role slot, seconds)`, most time first.
    pub features: Vec<(usize, f64)>,
}

/// "Plate 3 · Lid" (or "Plate 3").
pub fn title(plate: &Plate) -> String {
    match &plate.name {
        Some(name) => format!("Plate {} · {name}", plate.index + 1),
        None => format!("Plate {}", plate.index + 1),
    }
}

impl ProjectSummary {
    pub fn of(plates: &[Plate]) -> Self {
        let mut s = ProjectSummary::default();
        for plate in plates {
            if plate.status == PlateStatus::Empty {
                continue;
            }
            s.plates += 1;
            let PlateStatus::Sliced {
                time_s,
                grams,
                filament_mm,
                summary,
            } = &plate.status
            else {
                s.missing.push((plate.index, title(plate)));
                continue;
            };
            s.sliced += 1;
            s.timeline.push(PlateUse {
                index: plate.index,
                title: title(plate),
                time_s: *time_s,
                grams: *grams,
                start_s: s.time_s,
                tools: summary.tools.iter().map(|t| t.tool).collect(),
            });
            s.time_s += time_s;
            s.grams += grams;
            s.mm += filament_mm;
            s.cost += summary.cost;
            s.tool_changes += summary.tool_changes;
            if s.tower.is_none() {
                s.tower = summary.tower.clone();
            }
            for t in &summary.tools {
                s.tower_grams += t.tower_grams;
                s.tower_mm += t.tower_mm;
                let f = match s.filaments.iter_mut().position(|f| f.tool == t.tool) {
                    Some(i) => &mut s.filaments[i],
                    None => {
                        s.filaments.push(FilamentUse {
                            tool: t.tool,
                            filament: t.filament.clone(),
                            material: t.material.clone(),
                            ..Default::default()
                        });
                        s.filaments.last_mut().unwrap()
                    }
                };
                f.grams += t.grams;
                f.mm += t.filament_mm;
                f.cost += t.cost;
                f.tower_grams += t.tower_grams;
                f.plates.push((plate.index, t.grams, t.filament_mm));
            }
            for &(role, secs) in &summary.features {
                match s.features.iter_mut().find(|(r, _)| *r == role) {
                    Some((_, t)) => *t += secs,
                    None => s.features.push((role, secs)),
                }
            }
        }
        s.filaments.sort_by_key(|f| f.tool);
        s.features.sort_by(|a, b| b.1.total_cmp(&a.1));
        s
    }

    pub fn complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// "all 15 sliced", "12 of 15 sliced", "none sliced yet".
    pub fn note(&self) -> String {
        match (self.sliced, self.plates) {
            (_, 0) => "no plate has objects".into(),
            (0, _) => "none sliced yet".into(),
            (s, n) if s == n && n > 1 => format!("all {n} sliced"),
            (s, n) => format!("{s} of {n} sliced"),
        }
    }

    /// "72 h 10 min · 2.31 kg" over the sliced plates.
    pub fn total(&self) -> String {
        format!("{} · {}", duration(self.time_s), weight(self.grams))
    }

    /// The missing plates in a few words: "plates 3, 8 and 9 not sliced".
    pub fn missing_text(&self) -> String {
        let numbers: Vec<String> = self
            .missing
            .iter()
            .map(|(i, _)| (i + 1).to_string())
            .collect();
        match numbers.as_slice() {
            [] => String::new(),
            [one] => format!("plate {one} not sliced"),
            [rest @ .., last] => format!("plates {} and {last} not sliced", rest.join(", ")),
        }
    }

    /// Share of the total weight, percent.
    pub fn share(&self, grams: f64) -> f64 {
        if self.grams > 0. {
            100. * grams / self.grams
        } else {
            0.
        }
    }

    /// The plain-text summary Copy summary puts on the clipboard.
    pub fn text(&self, project: &str) -> String {
        let mut out = vec![];
        let head = if project.is_empty() {
            "Project summary".to_string()
        } else {
            format!("Project summary: {project}")
        };
        out.push(head);
        out.push(if self.complete() {
            format!("{} {} sliced", self.plates, plural(self.plates, "plate"))
        } else {
            format!(
                "{} of {} plates sliced; {} (not in the totals)",
                self.sliced,
                self.plates,
                self.missing_text()
            )
        });
        out.push(String::new());
        out.push(format!("Print time: {}", duration(self.time_s)));
        out.push(format!(
            "Filament: {} · {}",
            weight(self.grams),
            length(self.mm)
        ));
        if self.cost > 0. {
            out.push(format!("Cost: {:.2}", self.cost));
        }
        if self.tool_changes > 0 {
            out.push(format!("Tool changes: {}", self.tool_changes));
        }
        if let Some(tower) = &self.tower
            && self.tower_grams > 0.
        {
            out.push(format!(
                "{tower}: {} of the filament ({:.0} %)",
                weight(self.tower_grams),
                self.share(self.tower_grams)
            ));
        }
        if !self.filaments.is_empty() {
            out.push(String::new());
            out.push("By filament".into());
            for f in &self.filaments {
                let mut line = format!(
                    "  {}: {} · {}",
                    filament_label(f),
                    weight(f.grams),
                    length(f.mm)
                );
                if f.cost > 0. {
                    line.push_str(&format!(" · {:.2}", f.cost));
                }
                line.push_str(&format!(
                    " ({:.0} %, {} {})",
                    self.share(f.grams),
                    f.plates.len(),
                    plural(f.plates.len(), "plate")
                ));
                out.push(line);
            }
        }
        if !self.timeline.is_empty() || !self.missing.is_empty() {
            out.push(String::new());
            out.push("By plate".into());
            let mut rows: Vec<(usize, String)> = self
                .timeline
                .iter()
                .map(|p| {
                    (
                        p.index,
                        format!(
                            "  {}: {} · {} · {}",
                            p.title,
                            duration(p.time_s),
                            weight(p.grams),
                            tools_text(&p.tools)
                        ),
                    )
                })
                .collect();
            rows.extend(
                self.missing
                    .iter()
                    .map(|(i, t)| (*i, format!("  {t}: not sliced"))),
            );
            rows.sort_by_key(|(i, _)| *i);
            out.extend(rows.into_iter().map(|(_, r)| r));
        }
        if !self.features.is_empty() {
            out.push(String::new());
            out.push("Where the time goes".into());
            for &(role, secs) in &self.features {
                out.push(format!(
                    "  {}: {} ({:.0} %)",
                    feature_name(role),
                    duration(secs),
                    if self.time_s > 0. {
                        100. * secs / self.time_s
                    } else {
                        0.
                    }
                ));
            }
        }
        out.join("\n") + "\n"
    }
}

/// "Tool 2 · PLA · Generic PLA" (the type left out when the name already says it).
pub fn filament_label(f: &FilamentUse) -> String {
    let name = crate::preset_names::display_name(&f.filament);
    let mut parts = vec![format!("Tool {}", f.tool + 1)];
    if !f.material.is_empty() && !name.contains(&f.material) {
        parts.push(f.material.clone());
    }
    if !name.is_empty() {
        parts.push(name.to_string());
    }
    parts.join(" · ")
}

/// "tools 1, 2" / "tool 3".
pub fn tools_text(tools: &[usize]) -> String {
    let n: Vec<String> = tools.iter().map(|t| (t + 1).to_string()).collect();
    match n.len() {
        0 => "no filament".into(),
        1 => format!("tool {}", n[0]),
        _ => format!("tools {}", n.join(", ")),
    }
}

pub fn feature_name(role: usize) -> &'static str {
    crate::preview::legend::NAMES[role.min(crate::preview::legend::NAMES.len() - 1)]
}

/// Durations as the top bar shows them: "72 h 10 min", "38 min".
pub fn duration(seconds: f64) -> String {
    crate::panels::topbar::short_duration(seconds)
}

/// "46.3 g", "2.31 kg".
pub fn weight(grams: f64) -> String {
    if grams >= 1000. {
        format!("{:.2} kg", grams / 1000.)
    } else {
        format!("{grams:.1} g")
    }
}

/// "15.42 m", "771.2 m".
pub fn length(mm: f64) -> String {
    let m = mm / 1000.;
    if m >= 100. {
        format!("{m:.1} m")
    } else {
        format!("{m:.2} m")
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{Summary, ToolUsage};
    use std::sync::Arc;

    fn plate(index: usize, name: Option<&str>, status: PlateStatus) -> Plate {
        Plate {
            index,
            origin: glam::Vec2::ZERO,
            current: false,
            name: name.map(str::to_string),
            locked: false,
            objects: 1,
            alert: 0,
            notes: Vec::new(),
            status,
        }
    }

    fn tool(tool: usize, grams: f64, mm: f64, cost: f64, tower_grams: f64) -> ToolUsage {
        ToolUsage {
            tool,
            filament_mm: mm,
            grams,
            cost,
            filament: format!("Generic {} @System", if tool == 2 { "PETG" } else { "PLA" }),
            material: if tool == 2 { "PETG" } else { "PLA" }.into(),
            tower_mm: tower_grams * 330.,
            tower_grams,
        }
    }

    fn sliced(time_s: f64, tools: Vec<ToolUsage>, changes: usize) -> PlateStatus {
        let grams = tools.iter().map(|t| t.grams).sum();
        let mm = tools.iter().map(|t| t.filament_mm).sum();
        let tower = tools.iter().any(|t| t.tower_grams > 0.);
        PlateStatus::Sliced {
            time_s,
            grams,
            filament_mm: mm,
            summary: Arc::new(Summary {
                time_s,
                grams,
                filament_mm: mm,
                cost: tools.iter().map(|t| t.cost).sum(),
                tools,
                tool_changes: changes,
                tower: tower.then(|| "Prime tower".to_string()),
                // Outer wall, sparse infill, travel.
                features: vec![(2, time_s * 0.5), (5, time_s * 0.4), (14, time_s * 0.1)],
                ..Default::default()
            }),
        }
    }

    fn project() -> Vec<Plate> {
        vec![
            // Tools 1 and 2, with a tower.
            plate(
                0,
                Some("Base"),
                sliced(
                    3600.,
                    vec![tool(0, 20., 6600., 0.5, 2.), tool(1, 10., 3300., 0.25, 1.)],
                    40,
                ),
            ),
            // Not sliced: left out of every total.
            plate(1, None, PlateStatus::NotSliced),
            // Tools 2 and 3.
            plate(
                2,
                Some("Lid"),
                sliced(
                    1800.,
                    vec![tool(1, 5., 1650., 0.125, 0.5), tool(2, 8., 2600., 0.4, 0.5)],
                    10,
                ),
            ),
            // Changed since its slice: not counted either.
            plate(3, Some("Knobs"), PlateStatus::Outdated),
            plate(4, None, PlateStatus::Empty),
            // One tool, no tower.
            plate(5, None, sliced(600., vec![tool(0, 3., 990., 0.075, 0.)], 0)),
        ]
    }

    #[test]
    fn sums_per_tool_over_plates_with_different_tools() {
        let s = ProjectSummary::of(&project());
        assert_eq!((s.sliced, s.plates), (3, 5));
        assert_eq!(s.time_s, 6000.);
        assert!((s.grams - 46.).abs() < 1e-9);
        assert!((s.mm - 15140.).abs() < 1e-9);
        assert!((s.cost - 1.35).abs() < 1e-9);
        assert_eq!(s.tool_changes, 50);
        assert_eq!(s.tower.as_deref(), Some("Prime tower"));
        assert!((s.tower_grams - 4.).abs() < 1e-9);
        let tools: Vec<usize> = s.filaments.iter().map(|f| f.tool).collect();
        assert_eq!(tools, [0, 1, 2]);
        let t1 = &s.filaments[0];
        assert!((t1.grams - 23.).abs() < 1e-9 && (t1.mm - 7590.).abs() < 1e-9);
        assert_eq!(t1.plates, vec![(0, 20., 6600.), (5, 3., 990.)]);
        let t2 = &s.filaments[1];
        assert!((t2.grams - 15.).abs() < 1e-9 && (t2.cost - 0.375).abs() < 1e-9);
        assert_eq!(
            t2.plates.iter().map(|p| p.0).collect::<Vec<_>>(),
            [0, 2],
            "tool 2 is on plates 1 and 3"
        );
        assert_eq!(s.filaments[2].material, "PETG");
        // The per-tool rows add up to the totals.
        let sum: f64 = s.filaments.iter().map(|f| f.grams).sum();
        assert!((sum - s.grams).abs() < 1e-9);
        // The timeline: sliced plates in order, each starting where the one before ended.
        let starts: Vec<(usize, f64)> = s.timeline.iter().map(|p| (p.index, p.start_s)).collect();
        assert_eq!(starts, [(0, 0.), (2, 3600.), (5, 5400.)]);
        assert_eq!(s.timeline[1].tools, [1, 2]);
        // Features summed and sorted by time.
        assert_eq!(s.features, vec![(2, 3000.), (5, 2400.), (14, 600.)]);
    }

    #[test]
    fn partial_and_stale_plates_are_listed_as_missing() {
        let s = ProjectSummary::of(&project());
        assert!(!s.complete());
        assert_eq!(
            s.missing,
            vec![
                (1, "Plate 2".to_string()),
                (3, "Plate 4 · Knobs".to_string())
            ]
        );
        assert_eq!(s.note(), "3 of 5 sliced");
        assert_eq!(s.missing_text(), "plates 2 and 4 not sliced");
        // Queued, slicing and failed plates do not count either.
        let s = ProjectSummary::of(&[
            plate(0, None, PlateStatus::Queued),
            plate(1, None, PlateStatus::Slicing { progress: 0.5 }),
            plate(
                2,
                None,
                PlateStatus::Failed {
                    short: "x".into(),
                    message: "x".into(),
                },
            ),
        ]);
        assert_eq!((s.sliced, s.plates, s.missing.len()), (0, 3, 3));
        assert_eq!(s.note(), "none sliced yet");
        assert_eq!(s.time_s, 0.);
        assert_eq!(s.missing_text(), "plates 1, 2 and 3 not sliced");
        let all = ProjectSummary::of(&project()[..1]);
        assert!(all.complete() && all.missing_text().is_empty());
    }

    #[test]
    fn formats_large_totals() {
        let mut s = ProjectSummary {
            time_s: 72. * 3600. + 600.,
            grams: 2310.,
            ..Default::default()
        };
        assert_eq!(s.total(), "72 h 10 min · 2.31 kg");
        s.grams = 846.25;
        assert_eq!(s.total(), "72 h 10 min · 846.2 g");
        assert_eq!(length(771_234.), "771.2 m");
        assert_eq!(length(15_420.), "15.42 m");
        s.sliced = 15;
        s.plates = 15;
        assert_eq!(s.note(), "all 15 sliced");
    }

    #[test]
    fn copied_text() {
        let s = ProjectSummary::of(&project());
        let text = s.text("Rudolph");
        let expected = "\
Project summary: Rudolph
3 of 5 plates sliced; plates 2 and 4 not sliced (not in the totals)

Print time: 1 h 40 min
Filament: 46.0 g · 15.14 m
Cost: 1.35
Tool changes: 50
Prime tower: 4.0 g of the filament (9 %)

By filament
  Tool 1 · Generic PLA: 23.0 g · 7.59 m · 0.57 (50 %, 2 plates)
  Tool 2 · Generic PLA: 15.0 g · 4.95 m · 0.38 (33 %, 2 plates)
  Tool 3 · Generic PETG: 8.0 g · 2.60 m · 0.40 (17 %, 1 plate)

By plate
  Plate 1 · Base: 1 h 00 min · 30.0 g · tools 1, 2
  Plate 2: not sliced
  Plate 3 · Lid: 30 min · 13.0 g · tools 2, 3
  Plate 4 · Knobs: not sliced
  Plate 6: 10 min · 3.0 g · tool 1

Where the time goes
  Outer wall: 50 min (50 %)
  Sparse infill: 40 min (40 %)
  Travel: 10 min (10 %)
";
        assert_eq!(text, expected, "{text}");
        // Complete projects say so; no tool change or tower lines when there are none.
        let one = ProjectSummary::of(&project()[5..]);
        let text = one.text("");
        assert!(
            text.starts_with("Project summary\n1 plate sliced\n"),
            "{text}"
        );
        assert!(text.contains("Cost: 0.07") && !text.contains("Tool changes"));
        assert!(!text.contains("tower"));
    }
}
