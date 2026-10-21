// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Starting a Snapmaker U1 print with a tool → slot map and start options (bed leveling,
//! flow calibration per slot, timelapse), without re-slicing.
//!
//! Interface facts this follows (no code copied):
//! * Snapmaker Orca's Device UI (the compiled Flutter bundle `resources/web/flutter_web`,
//!   read for its wire format only): firmware below 1.3.0 gets one `printer.gcode.script`
//!   with `SET_PRINT_EXTRUDER_MAP CONFIG_EXTRUDER=<tool> MAP_EXTRUDER=<slot>` per tool,
//!   `SET_PRINT_USED_EXTRUDERS EXTRUDERS=<slots>` and `SET_PRINT_PREFERENCES <KEY>=<value>…`,
//!   then `printer.print.start`; 1.3.0 and later get `server.files.start_local_print
//!   {type, path, print_plate, options}` with `options = {bed_level, flow_calibrate,
//!   time_lapse_camera, map_table: [[tool, slot], …]}`; a `start_local_print` answering
//!   "failure" is retried as macros + `printer.print.start`. The version is
//!   `machine.system_info` → `system_info.product_info.firmware_version`.
//! * Snapmaker's GPL U1 firmware forks (github.com/Snapmaker/u1-moonraker, u1-klipper, read
//!   for parameter formats): Moonraker registers `start_local_print` for every transport
//!   except HTTP (so it is called over the WebSocket here) and passes each option to Klipper
//!   as `KEY="str(value)"`; Klipper reads the flags with `get_int` (so 0 / 1, not booleans),
//!   `MAP_TABLE` and `FLOW_CALIBRATE_EXTRUDERS` as Python literals (lists of physical slot
//!   indices), and the macro form's `FLOW_CALIBRATE_EXTRUDERS` as comma-separated slots.
//!
//! Tools and slots are zero based here (T0, slot index 0 = the printer's slot 1).

use crate::{
    Error, ErrorKind, Job, ProgressCallback, Result,
    http::{Http, remote_path, sanitize},
    moonraker::rpc,
    websocket::{self, WsError},
};
use serde_json::{Value, json};

/// One of the U1's filament slots as `print_task_config` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct U1Slot {
    /// Zero based.
    pub index: u32,
    /// `filament_exist[i]`; `None` when the printer does not report it.
    pub exists: Option<bool>,
    pub material: Option<String>,
    pub subtype: Option<String>,
    pub vendor: Option<String>,
    /// `RRGGBBAA` as the printer sends it.
    pub colour: Option<String>,
}

/// The start-relevant part of `print_task_config`.
#[derive(Clone, Debug, PartialEq)]
pub struct U1TaskConfig {
    pub slots: Vec<U1Slot>,
    pub bed_leveling: bool,
    pub flow_calibrate: bool,
    /// Per slot (physical extruder).
    pub flow_calib_slots: Vec<bool>,
    pub timelapse: bool,
}

/// Parses the `print_task_config` object. Slots come from the longest per-slot array.
pub fn parse_task_config(v: &Value) -> U1TaskConfig {
    let arr = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let (types, colours, exist, subs, vendors) = (
        arr("filament_type"),
        arr("filament_color_rgba"),
        arr("filament_exist"),
        arr("filament_sub_type"),
        arr("filament_vendor"),
    );
    let n = [types.len(), colours.len(), exist.len()]
        .into_iter()
        .max()
        .unwrap_or(0)
        .min(16);
    let text = |a: &[Value], i: usize| a.get(i).and_then(crate::string);
    let slots = (0..n)
        .map(|i| U1Slot {
            index: i as u32,
            exists: exist.get(i).and_then(Value::as_bool),
            material: text(&types, i),
            subtype: text(&subs, i),
            vendor: text(&vendors, i),
            colour: text(&colours, i),
        })
        .collect();
    U1TaskConfig {
        slots,
        bed_leveling: v["auto_bed_leveling"].as_bool().unwrap_or(false),
        flow_calibrate: v["flow_calibrate"].as_bool().unwrap_or(false),
        flow_calib_slots: arr("flow_calib_extruders")
            .iter()
            .map(|b| b.as_bool().unwrap_or(true))
            .collect(),
        timelapse: v["time_lapse_camera"].as_bool().unwrap_or(false),
    }
}

/// What to start: the tool → slot map and the start options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct U1StartOptions {
    /// `(tool, slot)`, zero based, in the job's tool order.
    pub map: Vec<(u32, u32)>,
    pub bed_leveling: bool,
    /// Flow calibration before printing, for the slots of `flow_tools` only.
    pub flow_calibration: bool,
    /// Tools (not slots) to calibrate; ignored while `flow_calibration` is off.
    pub flow_tools: Vec<u32>,
    pub timelapse: bool,
}
impl U1StartOptions {
    /// The physical slots to calibrate, in map order, without repeats.
    pub fn flow_slots(&self) -> Vec<u32> {
        let mut out = vec![];
        for (t, s) in &self.map {
            if self.flow_tools.contains(t) && !out.contains(s) {
                out.push(*s);
            }
        }
        out
    }
    fn calibrate(&self) -> bool {
        self.flow_calibration && !self.flow_slots().is_empty()
    }
    /// The slots used, in map order, without repeats (`SET_PRINT_USED_EXTRUDERS`).
    fn used_slots(&self) -> Vec<u32> {
        let mut out = vec![];
        for (_, s) in &self.map {
            if !out.contains(s) {
                out.push(*s);
            }
        }
        out
    }
    pub fn validate(&self) -> Result<()> {
        if self.map.is_empty() {
            return Err(Error::protocol("no tool → slot map to send"));
        }
        for (i, (t, s)) in self.map.iter().enumerate() {
            if *t >= 32 || *s >= 4 {
                return Err(Error::protocol(format!(
                    "T{} → slot {} is outside the U1's 32 tools / 4 slots",
                    t + 1,
                    s + 1
                )));
            }
            if self.map[..i].iter().any(|(u, _)| u == t) {
                return Err(Error::protocol(format!("T{} is mapped twice", t + 1)));
            }
        }
        Ok(())
    }
    /// `server.files.start_local_print`'s `options` (firmware 1.3.0+).
    pub fn local_print_options(&self) -> Value {
        let flag = |b: bool| json!(b as u8);
        let mut o = json!({
            "bed_level": flag(self.bed_leveling),
            "flow_calibrate": flag(self.calibrate()),
            "time_lapse_camera": flag(self.timelapse),
            "map_table": self.map.iter().map(|(t, s)| json!([t, s])).collect::<Vec<_>>(),
        });
        if self.calibrate() {
            o["flow_calibrate_extruders"] = json!(self.flow_slots());
        }
        o
    }
    /// The macro script for firmware below 1.3.0 (and the fallback).
    pub fn macro_script(&self) -> String {
        let mut s = String::new();
        for (t, slot) in &self.map {
            s.push_str(&format!(
                "SET_PRINT_EXTRUDER_MAP CONFIG_EXTRUDER={t} MAP_EXTRUDER={slot}\n"
            ));
        }
        let join = |v: Vec<u32>| v.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        s.push_str(&format!(
            "SET_PRINT_USED_EXTRUDERS EXTRUDERS={}\n",
            join(self.used_slots())
        ));
        s.push_str(&format!(
            "SET_PRINT_PREFERENCES BED_LEVEL={} FLOW_CALIBRATE={} TIME_LAPSE_CAMERA={}",
            self.bed_leveling as u8,
            self.calibrate() as u8,
            self.timelapse as u8
        ));
        if self.calibrate() {
            s.push_str(&format!(
                " FLOW_CALIBRATE_EXTRUDERS={}",
                join(self.flow_slots())
            ));
        }
        s
    }
}

/// How the print was started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum U1Route {
    /// `server.files.start_local_print` over the WebSocket.
    StartLocalPrint,
    /// Macros + `printer.print.start` (firmware below 1.3.0 or unknown).
    Macros,
    /// Macros + `printer.print.start` after `start_local_print` was not available.
    MacrosFallback(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct U1Started {
    pub job: Job,
    pub firmware: Option<String>,
    pub route: U1Route,
}
impl U1Started {
    /// One line for the user.
    pub fn summary(&self) -> String {
        let fw = self
            .firmware
            .as_deref()
            .map(|f| format!("firmware {f}"))
            .unwrap_or_else(|| "firmware version unknown".into());
        match &self.route {
            U1Route::StartLocalPrint => format!("Started with start_local_print ({fw})"),
            U1Route::Macros => format!("Started with the slot macros ({fw})"),
            U1Route::MacrosFallback(why) => {
                format!("Started with the slot macros ({fw}; start_local_print: {why})")
            }
        }
    }
}

/// `1.5.2.13` → `[1, 5, 2, 13]`; `None` for anything else (e.g. Klipper's `v0.12.0-418-g…`).
pub fn parse_firmware(text: &str) -> Option<Vec<u64>> {
    let t = text.trim();
    let parts: Vec<_> = t.split('.').collect();
    if !(2..=5).contains(&parts.len()) {
        return None;
    }
    parts.iter().map(|p| p.parse().ok()).collect()
}

/// True when `version` is at least `min` (missing parts count as 0).
pub fn at_least(version: &[u64], min: &[u64]) -> bool {
    for i in 0..version.len().max(min.len()) {
        let (a, b) = (
            version.get(i).copied().unwrap_or(0),
            min.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    true
}

/// The firmware version: `machine.system_info` → `product_info.firmware_version` (what
/// Snapmaker's app reads), else `/printer/info`'s `firmware_version` or a plain dotted
/// `software_version`. `Ok(None)`: the printer did not say.
pub(crate) fn firmware_version(http: &Http) -> Result<Option<String>> {
    let clean = |v: &Value| crate::string(v).filter(|s| parse_firmware(s).is_some());
    if let Some(v) = crate::http::optional(http.call("GET", "/machine/system_info", None))? {
        let v = rpc(v)?;
        if let Some(f) = clean(&v["system_info"]["product_info"]["firmware_version"]) {
            return Ok(Some(f));
        }
    }
    let p = rpc(http.call("GET", "/printer/info", None)?)?;
    Ok(clean(&p["firmware_version"]).or_else(|| clean(&p["software_version"])))
}

/// What a `start_local_print` answer means.
enum Answer {
    Started,
    /// Not available here: use the macro route (nothing happened on the printer).
    Fallback(String),
    Refused(String),
}

/// Interprets the JSON-RPC reply: an RPC error, or a `result` with `state`.
fn answer(v: &Value) -> Answer {
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        let message = e["message"].as_str().map(sanitize).unwrap_or_default();
        if e["code"].as_i64() == Some(-32601) {
            return Answer::Fallback(if message.is_empty() {
                "method not found".into()
            } else {
                message
            });
        }
        return Answer::Refused(if message.is_empty() {
            "the printer returned an error".into()
        } else {
            message
        });
    }
    let r = &v["result"];
    let message = r["message"]
        .as_str()
        .or(r["msg"].as_str())
        .map(sanitize)
        .unwrap_or_default();
    match r["state"].as_str() {
        None | Some("success") => Answer::Started,
        Some("failure") => Answer::Fallback(if message.is_empty() {
            "failure".into()
        } else {
            message
        }),
        Some(state) => Answer::Refused(if message.is_empty() {
            format!("state {state}")
        } else {
            message
        }),
    }
}

fn refused(message: String) -> Error {
    Error {
        kind: ErrorKind::Protocol,
        message,
        http_status: None,
    }
}

/// Upload (never auto-start), then start with the map. Every failure after the upload says
/// that the file is on the printer.
pub(crate) fn start_with_mapping(
    http: &Http,
    upload: impl FnOnce(&mut ProgressCallback<'_>) -> Result<Job>,
    remote_name: &str,
    options: &U1StartOptions,
    progress: &mut ProgressCallback<'_>,
) -> Result<U1Started> {
    options.validate()?;
    remote_path(remote_name)?;
    // Ask before uploading: a printer that cannot say its version is still printable.
    let firmware = firmware_version(http)?;
    let modern = firmware
        .as_deref()
        .and_then(parse_firmware)
        .is_some_and(|v| at_least(&v, &[1, 3, 0]));
    let mut job = upload(progress)?;
    let after = |e: Error| Error {
        message: format!(
            "{} was uploaded, but the print did not start: {}",
            remote_name, e.message
        ),
        ..e
    };
    let route = if modern {
        let params = json!({
            "type": "gcode",
            "path": remote_name,
            "print_plate": 1,
            "options": options.local_print_options(),
        });
        let reply = websocket::call(
            &http.base,
            http.api_key(),
            "server.files.start_local_print",
            params,
            http.timeouts.connect,
            http.timeouts.upload,
        );
        match reply {
            Ok(v) => match answer(&v) {
                Answer::Started => U1Route::StartLocalPrint,
                Answer::Fallback(why) => {
                    macros_then_start(http, remote_name, options).map_err(after)?;
                    U1Route::MacrosFallback(why)
                }
                Answer::Refused(why) => return Err(after(refused(why))),
            },
            Err(WsError::Unavailable(why)) => {
                macros_then_start(http, remote_name, options).map_err(after)?;
                U1Route::MacrosFallback(why)
            }
            // The request may have reached the printer: never start a second time.
            Err(WsError::Ambiguous(why)) => {
                return Err(after(Error {
                    kind: ErrorKind::Unreachable,
                    message: format!("{why}; check the printer's screen before trying again"),
                    http_status: None,
                }));
            }
        }
    } else {
        macros_then_start(http, remote_name, options).map_err(after)?;
        U1Route::Macros
    };
    job.started = true;
    Ok(U1Started {
        job,
        firmware,
        route,
    })
}

fn macros_then_start(http: &Http, remote_name: &str, options: &U1StartOptions) -> Result<()> {
    let script = options.macro_script();
    http.call_reporting(
        "POST",
        "/printer/gcode/script",
        Some(json!({ "script": script })),
    )
    .and_then(rpc)
    .map_err(|e| Error {
        message: format!("the slot macros were refused ({})", e.message),
        ..e
    })?;
    http.call_reporting(
        "POST",
        "/printer/print/start",
        Some(json!({ "filename": remote_name })),
    )
    .and_then(rpc)
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T0 → slot 3, T1 → slot 1 (zero based 2 and 0), flow calibration on T0 only.
    pub(crate) fn review_case() -> U1StartOptions {
        U1StartOptions {
            map: vec![(0, 2), (1, 0)],
            bed_leveling: false,
            flow_calibration: true,
            flow_tools: vec![0],
            timelapse: false,
        }
    }

    #[test]
    fn options_and_macros_have_the_firmware_format() {
        let o = review_case();
        assert_eq!(
            o.local_print_options().to_string(),
            r#"{"bed_level":0,"flow_calibrate":1,"flow_calibrate_extruders":[2],"map_table":[[0,2],[1,0]],"time_lapse_camera":0}"#
        );
        assert_eq!(
            o.macro_script(),
            "SET_PRINT_EXTRUDER_MAP CONFIG_EXTRUDER=0 MAP_EXTRUDER=2\n\
             SET_PRINT_EXTRUDER_MAP CONFIG_EXTRUDER=1 MAP_EXTRUDER=0\n\
             SET_PRINT_USED_EXTRUDERS EXTRUDERS=2,0\n\
             SET_PRINT_PREFERENCES BED_LEVEL=0 FLOW_CALIBRATE=1 TIME_LAPSE_CAMERA=0 FLOW_CALIBRATE_EXTRUDERS=2"
        );
        // Flow calibration with no tool picked is off; two tools on one slot use it once.
        let o = U1StartOptions {
            map: vec![(0, 1), (1, 1)],
            bed_leveling: true,
            flow_calibration: true,
            flow_tools: vec![],
            timelapse: true,
        };
        assert_eq!(
            o.local_print_options().to_string(),
            r#"{"bed_level":1,"flow_calibrate":0,"map_table":[[0,1],[1,1]],"time_lapse_camera":1}"#
        );
        assert!(o.macro_script().ends_with(
            "SET_PRINT_USED_EXTRUDERS EXTRUDERS=1\nSET_PRINT_PREFERENCES BED_LEVEL=1 FLOW_CALIBRATE=0 TIME_LAPSE_CAMERA=1"
        ));
        let o = U1StartOptions {
            flow_tools: vec![0, 1],
            ..o
        };
        assert_eq!(o.flow_slots(), [1]);
    }

    #[test]
    fn validation_rejects_bad_maps() {
        assert!(review_case().validate().is_ok());
        for map in [vec![], vec![(0, 4)], vec![(32, 0)], vec![(0, 1), (0, 2)]] {
            let o = U1StartOptions {
                map,
                ..review_case()
            };
            assert!(o.validate().is_err(), "{o:?}");
        }
    }

    #[test]
    fn firmware_versions_compare_numerically() {
        let v = parse_firmware("1.5.2.13").unwrap();
        assert!(at_least(&v, &[1, 3, 0]));
        assert!(at_least(&[1, 3], &[1, 3, 0]));
        assert!(!at_least(&parse_firmware("1.2.9").unwrap(), &[1, 3, 0]));
        assert!(at_least(&parse_firmware("1.10.0").unwrap(), &[1, 3, 0]));
        assert_eq!(parse_firmware("v0.12.0-418-gabc"), None);
        assert_eq!(parse_firmware("1"), None);
        assert_eq!(parse_firmware(""), None);
    }

    #[test]
    fn replies_are_classified_like_snapmakers_app() {
        let started = |v: Value| matches!(answer(&v), Answer::Started);
        assert!(started(
            json!({"jsonrpc":"2.0","result":{"state":"success"},"id":1})
        ));
        assert!(started(json!({"jsonrpc":"2.0","result":"ok","id":1})));
        assert!(matches!(
            answer(&json!({"result":{"state":"failure"}})),
            Answer::Fallback(_)
        ));
        assert!(matches!(
            answer(&json!({"error":{"code":-32601,"message":"Method not found for transport HTTP"}})),
            Answer::Fallback(m) if m.contains("transport HTTP")
        ));
        assert!(matches!(
            answer(&json!({"result":{"state":"error","message":"File not found: a.gcode"}})),
            Answer::Refused(m) if m == "File not found: a.gcode"
        ));
        assert!(matches!(
            answer(&json!({"error":{"code":400,"message":"[print_task_config] Invalid MAP_TABLE"}})),
            Answer::Refused(m) if m.contains("MAP_TABLE")
        ));
    }

    #[test]
    fn sample_u1_config_parses() {
        let c = parse_task_config(&crate::u1_mapping::tests::sample_u1());
        assert_eq!(c.slots.len(), 4);
        assert_eq!(c.slots[2].colour.as_deref(), Some("6F4C2FFF"));
        assert_eq!(c.slots[0].material.as_deref(), Some("PLA"));
        assert!(!c.bed_leveling && !c.flow_calibrate && !c.timelapse);
        assert_eq!(c.flow_calib_slots, [true; 4]);
    }
}
