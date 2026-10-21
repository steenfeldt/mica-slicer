// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Buddy v1 API. Optional per-slot fields are tolerant aliases, pending hardware verification.
use crate::{
    http::{Http, optional, remote_path},
    *,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct PrusaLink {
    pub(crate) http: Http,
    filaments: AtomicBool,
    legacy: AtomicBool,
}
impl PrusaLink {
    pub fn new(config: PrinterConfig, timeouts: Timeouts) -> Result<Self> {
        Ok(Self {
            http: Http::new(config, timeouts, true)?,
            filaments: AtomicBool::new(false),
            legacy: AtomicBool::new(false),
        })
    }
    fn v1(&self, endpoint: &str) -> Result<Option<Value>> {
        let value = optional(self.http.call("GET", endpoint, None))?;
        if value.is_none() {
            self.legacy.store(true, Ordering::Relaxed);
        }
        Ok(value)
    }
    fn job_action(&self, action: Option<&str>) -> Result<()> {
        let job = self.http.call("GET", "/api/v1/job", None)?;
        let id = job["id"]
            .as_u64()
            .map(|id| id.to_string())
            .or_else(|| string(&job["id"]))
            .ok_or(Error {
                kind: ErrorKind::Busy,
                message: "printer has no active job".into(),
                http_status: None,
            })?;
        let path = format!(
            "/api/v1/job/{}{}",
            http::encode(&id),
            action.map(|a| format!("/{a}")).unwrap_or_default()
        );
        self.http
            .call(if action.is_some() { "PUT" } else { "DELETE" }, &path, None)
            .map(|_| ())
    }
    fn status_value(&self) -> Result<Value> {
        self.http.call("GET", "/api/v1/status", None)
    }
}
impl PrinterHost for PrusaLink {
    fn capabilities(&self) -> Capabilities {
        let v1 = !self.legacy.load(Ordering::Relaxed);
        Capabilities {
            upload: v1,
            start: v1,
            pause_resume: v1,
            cancel: v1,
            status: true,
            filaments: self.filaments.load(Ordering::Relaxed),
            ..Capabilities::default()
        }
    }
    fn info(&self) -> Result<PrinterInfo> {
        if let Some(v) = self.v1("/api/v1/info")? {
            let s = self.status()?;
            let tools = slots(&v).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
            return Ok(PrinterInfo {
                model: string(&v["printer_model"]).or_else(|| string(&v["model"])),
                firmware: string(&v["firmware"]),
                state: s.state,
                tools: if tools.is_empty() { None } else { Some(tools) },
                formats: vec![FileFormat::Bgcode, FileFormat::Gcode],
                bed_type: string(&v["bed_type"]),
            });
        }
        let v = self.http.call("GET", "/api/version", None)?;
        let s = self.status()?;
        Ok(PrinterInfo {
            model: string(&v["text"]),
            firmware: string(&v["server"]),
            state: s.state,
            tools: None,
            formats: vec![FileFormat::Gcode],
            bed_type: None,
        })
    }
    fn upload_with_progress(
        &self,
        path: &Path,
        remote_name: &str,
        start_after: bool,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<Job> {
        let name = remote_path(remote_name)?;
        if FileFormat::from_path(Path::new(remote_name)).is_none() {
            return Err(Error::protocol(
                "PrusaLink requires a .bgcode or .gcode file",
            ));
        }
        self.http.upload(
            "PUT",
            &format!("/api/v1/files/usb/{name}"),
            path,
            vec![
                ("Content-Type", "application/octet-stream".into()),
                (
                    "Print-After-Upload",
                    if start_after { "?1" } else { "?0" }.into(),
                ),
                ("Overwrite-File", "?1".into()),
            ],
            (vec![], vec![]),
            progress,
        )?;
        Ok(Job {
            name: remote_name.into(),
            id: None,
            started: start_after,
        })
    }
    fn start(&self, name: &str) -> Result<()> {
        self.http
            .call(
                "POST",
                &format!("/api/v1/files/usb/{}", remote_path(name)?),
                Some(json!({"command":"start"})),
            )
            .map(|_| ())
    }
    fn pause(&self) -> Result<()> {
        self.job_action(Some("pause"))
    }
    fn resume(&self) -> Result<()> {
        self.job_action(Some("resume"))
    }
    fn cancel(&self) -> Result<()> {
        self.job_action(None)
    }
    fn status(&self) -> Result<PrinterStatus> {
        match optional(self.status_value())? {
            Some(v) => {
                self.legacy.store(false, Ordering::Relaxed);
                let f = parse_filaments(&v);
                self.filaments.store(!f.is_empty(), Ordering::Relaxed);
                let mut s = parse_status(&v)?;
                if s.job_name.is_none()
                    && matches!(
                        s.state,
                        PrinterState::Printing | PrinterState::Paused | PrinterState::Finished
                    )
                    && let Some(job) = optional(self.http.call("GET", "/api/v1/job", None))?
                {
                    s.job_name = string(&job["file"]["display_name"])
                        .or_else(|| string(&job["file"]["name"]))
                        .or_else(|| string(&job["name"]));
                }
                Ok(s)
            }
            None => {
                self.legacy.store(true, Ordering::Relaxed);
                parse_legacy(&self.http.call("GET", "/api/printer", None)?)
            }
        }
    }
    /// Prusa-Link (the Raspberry Pi daemon) lists cameras here; Buddy firmware answers 404,
    /// which is "no cameras", not an error. See `camera.rs` for the sources.
    fn cameras(&self) -> Result<Vec<CameraInfo>> {
        match optional(self.http.call("GET", "/api/v1/cameras", None))? {
            Some(v) => Ok(crate::camera::parse_prusalink(&v)),
            None => Ok(vec![]),
        }
    }
    fn camera_snapshot(&self, path: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        // Only the read-only snapshot endpoints; never the POST that takes a new photo.
        let ok = path
            .strip_prefix("/api/v1/cameras/")
            .and_then(|p| p.strip_suffix("/snap"))
            .is_some_and(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
            || path == "/api/v1/cameras/snap";
        if !ok {
            return Err(Error::protocol("not a PrusaLink camera snapshot path"));
        }
        self.http.get_bytes(path, limit)
    }
    /// The current job's `file.refs.thumbnail` (`/thumb/l/…`, the large PNG the firmware reads
    /// from the file), else its `icon`. Only `/thumb/` paths are fetched.
    fn job_thumbnail(&self, _job: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let Some(job) = optional(self.http.call("GET", "/api/v1/job", None))? else {
            return Ok(None);
        };
        let refs = &job["file"]["refs"];
        let Some(r) = string(&refs["thumbnail"]).or_else(|| string(&refs["icon"])) else {
            return Ok(None);
        };
        let Some(rest) = r.strip_prefix("/thumb/") else {
            return Ok(None);
        };
        let path = format!("/thumb/{}", remote_path(rest)?);
        optional(self.http.get_bytes(&path, limit)).map(Option::flatten)
    }
    fn loaded_filaments(&self) -> Result<Vec<LoadedFilament>> {
        let Some(v) = self.v1("/api/v1/status")? else {
            return Ok(vec![]);
        };
        let mut f = parse_filaments(&v);
        if f.is_empty()
            && let Some(v) = self.v1("/api/v1/info")?
        {
            f = parse_filaments(&v);
        }
        self.filaments.store(!f.is_empty(), Ordering::Relaxed);
        Ok(f)
    }
}
// Buddy numeric slot ids are one based; OctoPrint-style `tool0` ids are zero based.
fn slots(v: &Value) -> Vec<(u32, &Value)> {
    let v = v
        .get("tools")
        .or_else(|| v.get("slots"))
        .or_else(|| v.get("printer").and_then(|p| p.get("tools")));
    match v {
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, v)| (i as u32, v)).collect(),
        Some(Value::Object(o)) => o
            .iter()
            .filter_map(|(key, v)| {
                let index = if let Some(s) = key.strip_prefix("tool") {
                    s.parse().ok()
                } else {
                    key.parse::<u32>().ok().and_then(|n| n.checked_sub(1))
                }?;
                Some((index, v))
            })
            .collect(),
        _ => vec![],
    }
}
fn parse_filaments(v: &Value) -> Vec<LoadedFilament> {
    slots(v)
        .into_iter()
        .filter_map(|(tool, v)| {
            let material = string(&v["material"]).or_else(|| string(&v["filament_type"]));
            let colour = string(&v["colour"])
                .or_else(|| string(&v["color"]))
                .or_else(|| string(&v["filament_colour"]));
            (material.is_some() || colour.is_some()).then_some(LoadedFilament {
                tool,
                material,
                colour,
            })
        })
        .collect()
}
fn pair(v: &Value, current: &str, target: &str) -> Option<Temperature> {
    let t = Temperature {
        current: number(&v[current]),
        target: number(&v[target]),
    };
    (t.current.is_some() || t.target.is_some()).then_some(t)
}
fn parse_status(v: &Value) -> Result<PrinterStatus> {
    let p = &v["printer"];
    let state = p["state"]
        .as_str()
        .ok_or_else(|| Error::protocol("missing PrusaLink printer state"))?;
    let mut s = PrinterStatus::empty(PrinterState::parse(state));
    let j = &v["job"];
    s.job_name = string(&j["name"])
        .or_else(|| string(&j["file"]["display_name"]))
        .or_else(|| string(&j["file"]["name"]));
    s.progress = number(&j["progress"]).map(|p| p.clamp(0., 100.));
    s.elapsed = number(&j["time_printing"]);
    s.remaining = number(&j["time_remaining"]);
    s.current_layer = j["current_layer"].as_u64();
    s.z = number(&p["axis_z"]);
    for (tool, v) in slots(v) {
        if let Some(t) = pair(v, "temp_nozzle", "target_nozzle").or_else(|| temperature(v)) {
            s.hotends.push(ToolTemperature {
                tool,
                temperature: t,
            });
        }
    }
    if s.hotends.is_empty()
        && let Some(t) = pair(p, "temp_nozzle", "target_nozzle")
    {
        s.hotends.push(ToolTemperature {
            tool: 0,
            temperature: t,
        });
    }
    s.hotends.sort_by_key(|t| t.tool);
    s.bed = pair(p, "temp_bed", "target_bed");
    s.chamber = pair(p, "temp_chamber", "target_chamber");
    Ok(s)
}
fn parse_legacy(v: &Value) -> Result<PrinterStatus> {
    let flags = &v["state"]["flags"];
    let state = if flags["error"].as_bool() == Some(true) {
        PrinterState::Error
    } else if flags["paused"].as_bool() == Some(true) {
        PrinterState::Paused
    } else if flags["printing"].as_bool() == Some(true) {
        PrinterState::Printing
    } else if flags["ready"].as_bool() == Some(true) || flags["operational"].as_bool() == Some(true)
    {
        PrinterState::Idle
    } else if let Some(s) = v["state"]["text"].as_str() {
        PrinterState::parse(s)
    } else {
        return Err(Error::protocol("missing legacy printer state"));
    };
    let mut s = PrinterStatus::empty(state);
    if let Some(o) = v["temperature"].as_object() {
        for (name, v) in o {
            if let Some(t) = pair(v, "actual", "target") {
                if name == "bed" {
                    s.bed = Some(t);
                } else if let Some(tool) = name.strip_prefix("tool").and_then(|n| n.parse().ok()) {
                    s.hotends.push(ToolTemperature {
                        tool,
                        temperature: t,
                    });
                }
            }
        }
    }
    s.hotends.sort_by_key(|t| t.tool);
    Ok(s)
}
