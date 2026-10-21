// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/Utils/Moonraker.cpp :: Moonraker::upload
// Source: orca@8500fcdcca src/slic3r/Utils/MoonrakerPrinterAgent.cpp :: MoonrakerPrinterAgent::query_printer_status, MoonrakerPrinterAgent::fetch_object_list
// Source: prusaslicer@b028299c77 src/slic3r/Utils/OctoPrint.cpp :: PrusaLink::post_inner
// Changes: Rust HTTP transport and discovered multi-tool status; streamed multipart with directory/name split;
// Changes: Moonraker's documented print form flag and print_started acknowledgement, optional cameras/metadata.

//! Wire contract: Moonraker's public External API, Printer Administration (objects list/query),
//! File Management (upload/metadata) and Webcam Management (list):
//! https://moonraker.readthedocs.io/en/latest/external_api/
//! Mica estimates remaining seconds from elapsed time and SD progress; it does not treat
//! Klipper's total_duration (which includes pauses) as a predicted job duration.

use crate::{
    http::{Http, encode, optional, remote_path},
    *,
};
use serde_json::{Value, json};
use std::{
    fmt::Write as _,
    io::Read,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub struct Moonraker {
    pub(crate) http: Http,
    objects: Mutex<Option<Vec<String>>>,
    camera: AtomicBool,
}
impl Moonraker {
    pub fn new(config: PrinterConfig, timeouts: Timeouts) -> Result<Self> {
        Ok(Self {
            http: Http::new(config, timeouts, false)?,
            objects: Mutex::new(None),
            camera: AtomicBool::new(false),
        })
    }
    pub(crate) fn objects(&self) -> Result<Vec<String>> {
        // Cache discovery for this connection. Reconnect after firmware/config changes.
        let mut cached = self
            .objects
            .lock()
            .map_err(|_| Error::protocol("discovery cache unavailable"))?;
        if let Some(objects) = cached.as_ref() {
            return Ok(objects.clone());
        }
        let v = self.http.call("GET", "/printer/objects/list", None)?;
        let a = v["result"]["objects"]
            .as_array()
            .ok_or_else(|| Error::protocol("missing Moonraker objects list"))?;
        let objects: Vec<_> = a.iter().filter_map(string).collect();
        *cached = Some(objects.clone());
        Ok(objects)
    }
    fn command(&self, command: &str) -> Result<()> {
        self.http
            .call("POST", &format!("/printer/print/{command}"), None)
            .and_then(rpc)
            .map(|_| ())
    }
}
impl PrinterHost for Moonraker {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            upload: true,
            start: true,
            pause_resume: true,
            cancel: true,
            status: true,
            camera: self.camera.load(Ordering::Relaxed),
            ..Capabilities::default()
        }
    }
    fn info(&self) -> Result<PrinterInfo> {
        let server = rpc(self.http.call("GET", "/server/info", None)?)?;
        let klippy = server["klippy_state"]
            .as_str()
            .ok_or_else(|| Error::protocol("missing Klippy state"))?;
        if klippy != "ready" {
            return Ok(PrinterInfo {
                model: None,
                firmware: None,
                state: if klippy == "shutdown" || klippy == "error" {
                    PrinterState::Error
                } else {
                    PrinterState::Offline
                },
                tools: None,
                formats: vec![FileFormat::Gcode],
                bed_type: None,
            });
        }
        let p = rpc(self.http.call("GET", "/printer/info", None)?)?;
        let tools = self.objects()?.iter().filter_map(|n| tool_id(n)).collect();
        Ok(PrinterInfo {
            model: string(&p["hostname"]),
            firmware: string(&p["software_version"]),
            state: self.status()?.state,
            tools: Some(tools),
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
        remote_path(remote_name)?;
        if FileFormat::from_path(Path::new(remote_name)) != Some(FileFormat::Gcode) {
            return Err(Error::protocol("Moonraker requires text .gcode"));
        }
        let delimiter = boundary(path)?;
        let (prefix, suffix) = upload_parts(remote_name, start_after, &delimiter);
        let response = self.http.upload(
            "POST",
            "/server/files/upload",
            path,
            vec![(
                "Content-Type",
                format!("multipart/form-data; boundary={delimiter}"),
            )],
            (prefix, suffix),
            progress,
        )?;
        // The upload endpoint returns an object directly on some servers and the usual
        // result envelope on others. An API error must never look like a successful upload.
        let receipt = match (response.get("result"), response.get("error")) {
            (None, None) => response,
            _ => rpc(response)?,
        };
        Ok(Job {
            name: remote_name.to_owned(),
            id: None,
            started: receipt
                .get("print_started")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
    fn start(&self, name: &str) -> Result<()> {
        remote_path(name)?;
        rpc(self.http.call(
            "POST",
            "/printer/print/start",
            Some(json!({"filename":name})),
        )?)
        .map(|_| ())
    }
    fn pause(&self) -> Result<()> {
        self.command("pause")
    }
    fn resume(&self) -> Result<()> {
        self.command("resume")
    }
    fn cancel(&self) -> Result<()> {
        self.command("cancel")
    }
    fn status(&self) -> Result<PrinterStatus> {
        let inventory = self.objects()?;
        let query = status_query(&inventory)?;
        let result = rpc(self.http.call("GET", &query, None)?)?;
        parse_status(&result["status"], &inventory)
    }
    fn cameras(&self) -> Result<Vec<CameraInfo>> {
        let Some(v) = optional(self.http.call("GET", "/server/webcams/list", None))? else {
            self.camera.store(false, Ordering::Relaxed);
            return Ok(vec![]);
        };
        let cameras = crate::camera::parse_moonraker(&self.http.base, &rpc(v)?);
        self.camera.store(!cameras.is_empty(), Ordering::Relaxed);
        Ok(cameras)
    }
    fn file_slicer(&self, name: &str) -> Result<Option<String>> {
        remote_path(name)?;
        let Some(v) = optional(self.http.call(
            "GET",
            &format!("/server/files/metadata?filename={}", encode(name)),
            None,
        ))?
        else {
            return Ok(None);
        };
        Ok(rpc(v)?["slicer"].as_str().map(String::from))
    }
    /// Moonraker's file metadata lists the thumbnails it extracted from the G-code
    /// (`relative_path`, next to the file); the one `pick_thumbnail` chooses is fetched.
    fn job_thumbnail(&self, job: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let file = remote_path(job)?;
        let Some(v) = optional(self.http.call(
            "GET",
            &format!("/server/files/metadata?filename={}", encode(job)),
            None,
        ))?
        else {
            return Ok(None);
        };
        let Some(relative) = pick_thumbnail(&rpc(v)?["thumbnails"]) else {
            return Ok(None);
        };
        let relative = remote_path(&relative)?;
        let path = match file.rsplit_once('/') {
            Some((directory, _)) => format!("/server/files/gcodes/{directory}/{relative}"),
            None => format!("/server/files/gcodes/{relative}"),
        };
        optional(self.http.get_bytes(&path, limit)).map(Option::flatten)
    }
}
/// The thumbnail to show from Moonraker's metadata list: the largest up to 400 px wide, else
/// the smallest larger one.
pub(crate) fn pick_thumbnail(list: &Value) -> Option<String> {
    let mut all: Vec<(u64, String)> = list
        .as_array()?
        .iter()
        .filter_map(|t| {
            Some((
                t["width"].as_u64().unwrap_or(0),
                string(&t["relative_path"])?,
            ))
        })
        .collect();
    all.sort_by_key(|(w, _)| *w);
    all.iter()
        .rev()
        .find(|(w, _)| *w <= 400)
        .or_else(|| all.first())
        .map(|(_, p)| p.clone())
}
pub(crate) fn rpc(v: Value) -> Result<Value> {
    if !v["error"].is_null() {
        return Err(Error::protocol("Moonraker returned an API error"));
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| Error::protocol("missing Moonraker result"))
}
/// Build form framing separately from the file: Http streams the file between these bytes.
/// root/path/print/file are the fields specified by Moonraker's file-upload API.
fn upload_parts(remote: &str, print: bool, delimiter: &str) -> (Vec<u8>, Vec<u8>) {
    let (folder, filename) = remote.rsplit_once('/').unwrap_or(("", remote));
    let mut form = String::new();
    for (field, value) in [("root", "gcodes"), ("path", folder)]
        .into_iter()
        .chain(print.then_some(("print", "true")))
    {
        write!(
            form,
            "--{delimiter}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
        )
        .expect("writing a String cannot fail");
    }
    write!(form, "--{delimiter}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n")
        .expect("writing a String cannot fail");
    (
        form.into_bytes(),
        format!("\r\n--{delimiter}--\r\n").into_bytes(),
    )
}

pub(crate) fn tool_id(name: &str) -> Option<u32> {
    match name.strip_prefix("extruder")? {
        "" => Some(0),
        digits if digits.bytes().all(|digit| digit.is_ascii_digit()) => {
            digits.parse().ok().filter(|id| *id != 0)
        }
        _ => None,
    }
}

fn chamber(name: &str) -> bool {
    let Some((kind, _)) = name.split_once(' ') else {
        return false;
    };
    matches!(
        kind,
        "temperature_sensor" | "temperature_fan" | "heater_generic"
    ) && name.to_ascii_lowercase().contains("chamber")
}

fn status_query(inventory: &[String]) -> Result<String> {
    if !inventory.iter().any(|name| name == "print_stats") {
        return Err(Error::protocol("printer has no print_stats object"));
    }
    let mut selected: Vec<&str> = inventory
        .iter()
        .map(String::as_str)
        .filter(|name| {
            matches!(
                *name,
                "print_stats" | "virtual_sdcard" | "heater_bed" | "motion_report"
            ) || tool_id(name).is_some()
                || chamber(name)
        })
        .collect();
    selected.sort_unstable();
    let arguments = selected
        .into_iter()
        .map(encode)
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("/printer/objects/query?{arguments}"))
}

fn parse_status(snapshot: &Value, inventory: &[String]) -> Result<PrinterStatus> {
    let stats = &snapshot["print_stats"];
    let state = stats
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::protocol("missing Moonraker print state"))?;
    let progress = number(&snapshot["virtual_sdcard"]["progress"])
        .map(|fraction| (fraction * 100.).clamp(0., 100.));
    let elapsed = number(&stats["print_duration"]);
    let remaining = match (progress, elapsed) {
        (Some(percent), Some(seconds)) if percent > 0. && percent < 100. => {
            Some((seconds * (100. - percent) / percent).max(0.))
        }
        _ => None,
    };
    let mut hotends: Vec<_> = inventory
        .iter()
        .filter_map(|name| {
            tool_id(name).map(|tool| ToolTemperature {
                tool,
                temperature: temperature(&snapshot[name]).unwrap_or_default(),
            })
        })
        .collect();
    hotends.sort_by_key(|hotend| hotend.tool);
    Ok(PrinterStatus {
        state: PrinterState::parse(state),
        job_name: string(&stats["filename"]),
        progress,
        elapsed,
        remaining,
        current_layer: stats["info"]["current_layer"].as_u64(),
        z: snapshot["motion_report"]["live_position"]
            .get(2)
            .and_then(number),
        hotends,
        bed: temperature(&snapshot["heater_bed"]),
        // Discovery order decides which available chamber sensor is shown.
        chamber: inventory
            .iter()
            .filter(|name| chamber(name))
            .find_map(|name| temperature(&snapshot[name])),
    })
}
pub(crate) fn boundary(path: &Path) -> Result<String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    loop {
        let b = format!(
            "mica-{:x}-{:x}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut file =
            std::fs::File::open(path).map_err(|_| Error::protocol("cannot open upload file"))?;
        if !file
            .metadata()
            .map_err(|_| Error::protocol("cannot stat upload file"))?
            .is_file()
        {
            return Err(Error::protocol("upload path must be a regular file"));
        }
        let mut buf = vec![0; 65536 + b.len()];
        let mut kept = 0;
        let collision = loop {
            let n = file
                .read(&mut buf[kept..])
                .map_err(|_| Error::protocol("cannot read upload file"))?;
            if n == 0 {
                break false;
            }
            let end = kept + n;
            if buf[..end].windows(b.len()).any(|w| w == b.as_bytes()) {
                break true;
            }
            kept = end.min(b.len() - 1);
            buf.copy_within(end - kept..end, 0);
        };
        if !collision {
            return Ok(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_snapshot_preserves_unknowns_and_chamber_discovery_order() {
        let inventory = [
            "print_stats",
            "extruder2",
            "temperature_sensor chamber_missing",
            "heater_generic chamber_second",
            "temperature_fan chamber_first",
            "virtual_sdcard",
        ]
        .map(String::from);
        let snapshot = json!({
            "print_stats":{"state":"standby", "print_duration":0},
            "virtual_sdcard":{"progress":0},
            "heater_generic chamber_second":{"temperature":31},
            "temperature_fan chamber_first":{"temperature":40}
        });
        let status = parse_status(&snapshot, &inventory).unwrap();
        assert_eq!(
            status.hotends,
            vec![ToolTemperature {
                tool: 2,
                temperature: Temperature::default()
            }]
        );
        assert_eq!(status.chamber.unwrap().current, Some(31.));
        assert!(status.bed.is_none());
        assert!(status.z.is_none());
        assert!(status.remaining.is_none());
        let finished = json!({
            "print_stats":{"state":"complete", "print_duration":120},
            "virtual_sdcard":{"progress":1.5}
        });
        let status = parse_status(&finished, &inventory).unwrap();
        assert_eq!(status.progress, Some(100.));
        assert!(status.remaining.is_none());
        assert!(status.chamber.is_none());
    }
}
