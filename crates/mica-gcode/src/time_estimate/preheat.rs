// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::post_process, ExportLines::insert_lines
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::post_process
// Changes: Reuse Mica's disk-backed planner timeline; bounded streaming backtrace, no whole-file cache. Orca first-layer fix; Snapmaker delta only if explicitly configured.
// Changes: Orca M104 path tracks the active tool per line: a preheat whose upstream position lies in the tool's own
// Changes: active segment moves to the first line after its switch-away (that switch's cooldown is dropped); none for the active tool.
// Changes: Prusa rules keep upstream's temperature choice from the analysis pass's final layer count (first-layer
// Changes: temperature only for a one-layer print); Orca rules use the layer being rewritten (Orca's fix).

use mica_core::config::Config;
use std::collections::VecDeque;
use std::io::{self, Write};

struct Entry {
    text: String,
    time: [f64; 2],
    /// Tool selected once this line has run (None before the first tool selection).
    active: Option<usize>,
}
/// Backtrace cache uses at most 8 MiB even for large zero-time custom scripts.
pub(super) struct Preheat<'a> {
    dst: &'a mut dyn Write,
    config: Config,
    queue: VecDeque<Entry>,
    time: [f64; 2],
    window: f64,
    steps: usize,
    xl: bool,
    silent: bool,
    layer: usize,
    /// XL with Prusa rules: whether every preheat uses `first_layer_temperature` (see [`Preheat::new`]).
    first_layer_only: Option<bool>,
    bytes: usize,
    active: Option<usize>,
}
impl<'a> Preheat<'a> {
    /// `layers` is the analysis pass's layer count (the `;LAYER_CHANGE` tags of the whole program).
    pub fn new(dst: &'a mut dyn Write, c: &Config, silent: bool, layers: u64) -> Self {
        let xl = c
            .string("printer_notes")
            .unwrap_or("")
            .contains("PRINTER_MODEL_XL");
        let enabled = xl
            || (c.bool("ooze_prevention").unwrap_or(false)
                && !c.bool("single_extruder_multi_material").unwrap_or(false)
                && c.vector_len("nozzle_diameter") > 1);
        let window = if enabled {
            if xl {
                120.
            } else {
                c.float("preheat_time")
                    .or_else(|| c.extra("preheat_time").and_then(|s| s.parse().ok()))
                    .unwrap_or(0.)
                    .max(0.)
            }
        } else {
            0.
        };
        let steps = if xl {
            10
        } else {
            c.int("preheat_steps")
                .or_else(|| c.extra("preheat_steps").and_then(|s| s.parse().ok()))
                .unwrap_or(1)
                .clamp(1, 1000) as usize
        };
        Self {
            dst,
            config: c.clone(),
            queue: VecDeque::new(),
            time: [0.; 2],
            window,
            steps,
            xl,
            silent,
            layer: 0,
            // PrusaSlicer's post_process inserter reads `m_layer_id`, which still holds the analysis
            // pass's final layer count: `m_layer_id != 1 ? temperature : first_layer_temperature`.
            // Every preheat of a multi-layer print therefore uses the other-layers temperature.
            // Orca tracks the current layer instead (its `current_layer_id` fix); PrusaSlicer has no
            // non-XL preheat, so that path is Orca's in either case.
            first_layer_only: (xl && !mica_core::rules::Rules::of(c).is_orca())
                .then_some(layers == 1),
            bytes: 0,
            active: None,
        }
    }
    pub fn set_time(&mut self, time: [f64; 2]) {
        self.time = time;
    }
    fn command(line: &str) -> &str {
        line.split(';')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("")
    }
    fn push(&mut self, text: &str) -> io::Result<()> {
        if self.window <= 0. {
            return self.dst.write_all(text.as_bytes());
        }
        if text.trim() == ";LAYER_CHANGE" {
            self.layer += 1;
        }
        let command = Self::command(text);
        // Make postprocessing idempotent for the commands generated here.
        if command == "M104.1" || (command == "M104" && text.contains("; preheat T")) {
            return Ok(());
        }
        if let Some(tool) = command
            .strip_prefix('T')
            .and_then(|s| s.parse::<usize>().ok())
            && tool < self.config.vector_len("nozzle_diameter")
        {
            self.insert(tool, command);
            self.active = Some(tool);
        }
        self.bytes += text.len();
        self.queue.push_back(Entry {
            text: text.into(),
            time: self.time,
            active: self.active,
        });
        while self
            .queue
            .get(1)
            .is_some_and(|e| self.time[0] - e.time[0] > self.window.max(120.))
            || self.bytes > 8 * 1024 * 1024
        {
            self.flush_one()?;
        }
        Ok(())
    }
    fn insert(&mut self, tool: usize, cmd: &str) {
        let mut cursor = self.queue.len();
        let mut last = None;
        let xl = self.xl;
        for step in 1..=self.steps {
            let threshold = self.time[0] - self.window * step as f64 / self.steps as f64;
            let begin = cursor;
            while cursor > 0 {
                let entry = &mut self.queue[cursor - 1];
                let command = Self::command(&entry.text);
                if command == cmd || Self::is_start(xl, command) {
                    return;
                }
                if entry.time[0] <= threshold {
                    break;
                }
                if Self::is_cooldown(&entry.text, tool) {
                    self.bytes -= entry.text.len();
                    entry.text = "; removed M104\n".into();
                    self.bytes += entry.text.len();
                }
                cursor -= 1;
            }
            if cursor == 0 {
                break;
            }
            // Upstream inserts here even when the tool is still selected at this point (its window
            // reaches back past the switch away from it). Klipper/Marlin M104 would then retarget the
            // printing nozzle, so the Orca path moves the line to the start of the tool's idle period.
            let mut at = cursor;
            let clamped = !self.xl && self.queue[at - 1].active == Some(tool);
            if clamped {
                while at < self.queue.len() && self.queue[at - 1].active == Some(tool) {
                    at += 1;
                }
                if self.queue[at - 1].active == Some(tool) || at == self.queue.len() {
                    // Selected up to the tool change itself: this is the printing tool.
                    return;
                }
                // The window covers the switch away, so the tool is never meant to cool:
                // drop the cooldown that switch emitted, as upstream does for cooldowns in the window.
                for i in (0..at).rev() {
                    let entry = &mut self.queue[i];
                    if entry.active != Some(tool) && i + 1 < at {
                        break;
                    }
                    let command = Self::command(&entry.text);
                    if command == cmd {
                        break;
                    }
                    if Self::is_cooldown(&entry.text, tool) {
                        self.bytes -= entry.text.len();
                        entry.text = "; removed M104\n".into();
                        self.bytes += entry.text.len();
                    }
                }
            }
            let time = self.queue[at - 1].time;
            if at == begin || last == Some(time[0]) {
                if clamped {
                    break;
                }
                continue;
            }
            last = Some(time[0]);
            let temp = self
                .config
                .float_at(
                    if self.first_layer_only.unwrap_or(self.layer <= 1) {
                        "first_layer_temperature"
                    } else {
                        "temperature"
                    },
                    tool,
                )
                .unwrap_or(0.);
            let diff = [
                (self.time[0] - time[0]).round(),
                (self.time[1] - time[1]).round(),
            ];
            let text = if self.xl {
                format!(
                    "M104.1 T{tool} P{}{} S{}\n",
                    diff[0],
                    if self.silent {
                        format!(" Q{}", diff[1])
                    } else {
                        String::new()
                    },
                    temp.round()
                )
            } else {
                format!(
                    "M104 S{} T{tool} ; preheat T{tool} time: {}s\n",
                    (temp + self.config.float("delta_temperature").unwrap_or(0.)).round(),
                    diff[0]
                )
            };
            self.bytes += text.len();
            let active = self.queue[at - 1].active;
            self.queue.insert(at, Entry { text, time, active });
            if clamped {
                // Earlier steps would land in the same active segment.
                break;
            }
        }
    }
    /// Barriers the backtrace never crosses: homing on both paths, Orca's print-start macros too.
    fn is_start(xl: bool, command: &str) -> bool {
        if xl {
            return command == "G28" || command == "G29";
        }
        ["G28", "G29", "PRINT_START", "START_PRINT"]
            .iter()
            .any(|c| command.eq_ignore_ascii_case(c))
    }
    /// A generated standby for `tool` (`M104 … T<tool> … ; cooldown`).
    fn is_cooldown(text: &str, tool: usize) -> bool {
        Self::command(text) == "M104"
            && text.contains("cooldown")
            && text
                .split(';')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .any(|w| w == format!("T{tool}"))
    }
    fn flush_one(&mut self) -> io::Result<()> {
        if let Some(e) = self.queue.pop_front() {
            self.bytes -= e.text.len();
            self.dst.write_all(e.text.as_bytes())?;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> io::Result<()> {
        while !self.queue.is_empty() {
            self.flush_one()?;
        }
        Ok(())
    }
}
impl Write for Preheat<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = std::str::from_utf8(buf).map_err(io::Error::other)?;
        for line in text.split_inclusive('\n') {
            self.push(line)?;
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.dst.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_step_u1_preheat_keeps_the_boundary_sample() {
        use mica_core::config::{ConfigValue, Value};
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        c.set_raw("ooze_prevention", "1").unwrap();
        c.set_raw("first_layer_temperature", "220,230").unwrap();
        c.set("preheat_time", ConfigValue::Scalar(Value::Float(30.)));
        c.set("preheat_steps", ConfigValue::Scalar(Value::Int(1)));
        let mut out = Vec::new();
        let mut p = Preheat::new(&mut out, &c, false, 1);
        p.write_all(b"G28\nT0\n;LAYER_CHANGE\n").unwrap();
        for i in 1..=70 {
            p.set_time([i as f64, 0.]);
            p.write_all(b"G1 X1\n").unwrap();
        }
        p.write_all(b"T1\n").unwrap();
        p.finish().unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches("; preheat T1 time: 30s").count(), 1);
        assert!(text.contains("M104 S230 T1"));
    }

    fn orca(tools: usize, steps: i64) -> Config {
        use mica_core::config::{ConfigValue, Value};
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", &vec!["0.4"; tools].join(","))
            .unwrap();
        c.set_raw("ooze_prevention", "1").unwrap();
        c.set_raw("first_layer_temperature", &vec!["220"; tools].join(","))
            .unwrap();
        c.set_raw("temperature", &vec!["215"; tools].join(","))
            .unwrap();
        c.set("preheat_time", ConfigValue::Scalar(Value::Float(30.)));
        c.set("preheat_steps", ConfigValue::Scalar(Value::Int(steps)));
        c
    }

    /// Feeds `(time, line)` pairs and returns the post-processed text.
    fn run(c: &Config, xl: bool, lines: &[(f64, &str)]) -> String {
        let mut out = Vec::new();
        let layers = lines.iter().filter(|l| l.1 == ";LAYER_CHANGE").count() as u64;
        let mut p = Preheat::new(&mut out, c, false, layers);
        assert_eq!(p.xl, xl);
        for &(t, line) in lines {
            p.set_time([t, t]);
            p.write_all(format!("{line}\n").as_bytes()).unwrap();
        }
        p.finish().unwrap();
        String::from_utf8(out).unwrap()
    }

    /// U1-shaped job: T0 prints 0–50 s, switches to T1 for 10 s, returns to T0 at 60 s.
    /// The 30 s window for the second T0 reaches 20 s into T0's own print segment.
    fn overlapping_job() -> Vec<(f64, &'static str)> {
        let mut lines = vec![(0., "G28"), (0., "T0"), (0., ";LAYER_CHANGE")];
        for t in 1..=50 {
            lines.push((t as f64, "G1 X1 E1"));
        }
        lines.extend([
            (50., "M104 S70 T0 ; cooldown"),
            (51., "G1 Z1.5"),
            (52., "M109 S220 T1"),
            (52., "T1"),
        ]);
        for t in 53..=59 {
            lines.push((t as f64, "G1 X2 E1"));
        }
        lines.extend([
            (59., "M104 S70 T1 ; cooldown"),
            (60., "M109 S220 T0"),
            (60., "T0"),
        ]);
        lines
    }

    fn active_at_each_line(text: &str) -> Vec<(Option<usize>, &str)> {
        let mut active = None;
        text.lines()
            .map(|l| {
                if let Some(t) = Preheat::command(l)
                    .strip_prefix('T')
                    .and_then(|s| s.parse().ok())
                {
                    active = Some(t);
                }
                (active, l)
            })
            .collect()
    }

    #[test]
    fn orca_preheat_never_targets_the_tool_still_printing() {
        for steps in [1, 3] {
            let text = run(&orca(4, steps), false, &overlapping_job());
            for (active, line) in active_at_each_line(&text) {
                if let Some(rest) = line.split("; preheat T").nth(1) {
                    let tool: usize = rest.split(' ').next().unwrap().parse().unwrap();
                    assert_ne!(active, Some(tool), "steps {steps}: {line}\n{text}");
                }
            }
            // Moved to the first line after the switch away, with the idle period as its lead.
            assert_eq!(text.matches("; preheat T0").count(), 1, "{text}");
            assert!(
                text.contains("T1\nM104 S220 T0 ; preheat T0 time: 8s\n"),
                "{text}"
            );
            // The window covers the switch away, so T0 is never sent to standby.
            assert!(!text.contains("M104 S70 T0 ; cooldown"), "{text}");
            assert!(text.contains("M104 S70 T1 ; cooldown"));
        }
    }

    #[test]
    fn orca_preheat_inside_the_idle_period_is_unchanged() {
        // T1 idles 0–52 s; its 30 s window ends at 22 s, well after the last T1 use.
        let text = run(&orca(2, 1), false, &overlapping_job());
        let before_t1 = text.split("\nT1\n").next().unwrap();
        assert!(before_t1.contains("M104 S220 T1 ; preheat T1 time: 30s\nG1 X1 E1\n"));
        assert_eq!(text.matches("; preheat T1").count(), 1);
    }

    #[test]
    fn reselecting_the_printing_tool_gets_no_preheat() {
        let mut lines = vec![(0., "G28"), (0., "T2")];
        for t in 1..=80 {
            lines.push((t as f64, "G1 X1 E1"));
        }
        lines.push((80., "T2"));
        let text = run(&orca(4, 2), false, &lines);
        assert!(!text.contains("preheat"), "{text}");
        assert!(!text.contains("removed"), "{text}");
    }

    #[test]
    fn preheats_only_tools_selected_after_them() {
        // T3 is assigned but never selected: nothing heats it.
        let text = run(&orca(4, 1), false, &overlapping_job());
        for (tool, count) in [(0, 1), (1, 1), (2, 0), (3, 0)] {
            assert_eq!(text.matches(&format!("; preheat T{tool} ")).count(), count);
        }
    }

    #[test]
    fn orca_backtrace_stops_at_print_start_macros() {
        let mut lines = vec![(0., "print_start EXTRUDER=220"), (0., "T0")];
        for t in 1..=20 {
            lines.push((t as f64, "G1 X1 E1"));
        }
        lines.push((20., "T1"));
        assert!(!run(&orca(2, 1), false, &lines).contains("preheat"));
    }

    #[test]
    fn xl_keeps_prusa_backtrace_into_the_active_segment() {
        // Prusa firmware schedules M104.1 itself; PrusaSlicer emits it from the same position.
        let mut c = orca(2, 1);
        c.set_raw("printer_notes", "PRINTER_MODEL_XL").unwrap();
        let text = run(&c, true, &overlapping_job());
        let before_t1 = text.split("\nT1\n").next().unwrap();
        assert!(before_t1.contains("M104.1 T0 P60 S220\n"), "{text}");
        assert_eq!(text.matches("M104.1 T0").count(), 5, "{text}");
    }

    #[test]
    fn preheat_stops_at_homing_and_previous_use_and_uses_first_layer_temperature() {
        let mut c = Config::defaults();
        for (k, v) in [
            ("nozzle_diameter", "0.4,0.4"),
            ("printer_notes", "PRINTER_MODEL_XL"),
            ("temperature", "200,210"),
            ("first_layer_temperature", "220,230"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let mut out = Vec::new();
        let mut p = Preheat::new(&mut out, &c, true, 1);
        p.write_all(b"G28\nT0\n;LAYER_CHANGE\n").unwrap();
        for i in 1..=150 {
            p.set_time([i as f64, 2. * i as f64]);
            p.write_all(b"G1 X1\n").unwrap();
        }
        p.write_all(b"T1\n").unwrap();
        p.finish().unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("M104.1 T1 P120 Q240 S230"));
        assert_eq!(text.matches("M104.1").count(), 10);
        assert!(text.starts_with("G28\nT0\n"));
    }

    /// PrusaSlicer picks the preheat temperature from the analysis pass's final layer count, so a
    /// multi-layer XL print preheats with `temperature` even for first-layer tool changes; Orca
    /// rules use the layer the tool change is on.
    #[test]
    fn xl_preheat_temperature_follows_the_slicing_rules() {
        let job = |rules: &str, layers: u64| {
            let mut c = Config::defaults();
            for (k, v) in [
                ("nozzle_diameter", "0.4,0.4"),
                ("printer_notes", "PRINTER_MODEL_XL"),
                ("temperature", "225,250"),
                ("first_layer_temperature", "230,240"),
                ("mica_slicing_rules", rules),
            ] {
                c.set_raw(k, v).unwrap();
            }
            let mut out = Vec::new();
            let mut p = Preheat::new(&mut out, &c, false, layers);
            p.write_all(b"G28\nT0\n;LAYER_CHANGE\n").unwrap();
            for i in 1..=150 {
                p.set_time([i as f64, i as f64]);
                p.write_all(b"G1 X1\n").unwrap();
            }
            p.write_all(b"T1\n").unwrap();
            p.finish().unwrap();
            let text = String::from_utf8(out).unwrap();
            text.lines()
                .filter(|l| l.starts_with("M104.1"))
                .map(|l| l.rsplit(' ').next().unwrap().to_string())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let set = |s: &str| std::collections::BTreeSet::from([s.to_string()]);
        assert_eq!(job("prusaslicer", 20), set("S250"));
        assert_eq!(job("prusaslicer", 1), set("S240"));
        assert_eq!(job("orcaslicer", 20), set("S240"));
    }
}
