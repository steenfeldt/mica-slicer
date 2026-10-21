// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/Utils/SnapmakerPrinterAgent.cpp :: SnapmakerPrinterAgent::fetch_filament_info
// Changes: optional discovered print_task_config/filament_detect query; raw material and RGBA retained, no preset matching or invented defaults; slots without filament_exist count by their type; the start-relevant config (slots, start flags) for the mapped start in u1.rs.

use crate::{http::optional, moonraker::rpc, *};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// U1 extension of Moonraker. No assumed private RPCs or default camera URLs.
pub struct SnapmakerU1 {
    inner: Moonraker,
    filaments: AtomicBool,
}
impl SnapmakerU1 {
    pub fn new(config: PrinterConfig, timeouts: Timeouts) -> Result<Self> {
        Ok(Self {
            inner: Moonraker::new(config, timeouts)?,
            filaments: AtomicBool::new(false),
        })
    }
    /// The raw `print_task_config` (and `filament_detect`) status, if the printer has it.
    fn task_status(&self) -> Result<Option<serde_json::Value>> {
        let objects = self.inner.objects()?;
        if !objects.iter().any(|o| o == "print_task_config") {
            return Ok(None);
        }
        let endpoint = if objects.iter().any(|o| o == "filament_detect") {
            "/printer/objects/query?print_task_config&filament_detect"
        } else {
            "/printer/objects/query?print_task_config"
        };
        let Some(v) = optional(self.inner.http.call("GET", endpoint, None))? else {
            return Ok(None);
        };
        Ok(Some(rpc(v)?["status"].take()))
    }
    /// The slots and start flags (`print_task_config`); `None` when the printer has none.
    pub fn task_config(&self) -> Result<Option<U1TaskConfig>> {
        Ok(self
            .task_status()?
            .map(|v| crate::u1::parse_task_config(&v["print_task_config"]))
            .filter(|c| !c.slots.is_empty()))
    }
    /// The firmware version (`1.5.2.13`), when the printer reports one.
    pub fn firmware_version(&self) -> Result<Option<String>> {
        crate::u1::firmware_version(&self.inner.http)
    }
    /// Uploads `path` as `remote_name` without starting it, then starts it with the tool →
    /// slot map and start options: `server.files.start_local_print` on firmware 1.3.0 and
    /// later, the `SET_PRINT_*` macros + `printer.print.start` before that (and as the
    /// fallback when `start_local_print` is not available). See `u1.rs`.
    pub fn start_with_mapping(
        &self,
        path: &Path,
        remote_name: &str,
        options: &U1StartOptions,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<U1Started> {
        crate::u1::start_with_mapping(
            &self.inner.http,
            |p| self.inner.upload_with_progress(path, remote_name, false, p),
            remote_name,
            options,
            progress,
        )
    }
}
impl PrinterHost for SnapmakerU1 {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            filaments: self.filaments.load(Ordering::Relaxed),
            ..self.inner.capabilities()
        }
    }
    fn info(&self) -> Result<PrinterInfo> {
        let mut info = self.inner.info()?;
        info.model = Some("Snapmaker U1".into());
        Ok(info)
    }
    fn upload_with_progress(
        &self,
        path: &Path,
        name: &str,
        start: bool,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<Job> {
        self.inner.upload_with_progress(path, name, start, progress)
    }
    fn start(&self, name: &str) -> Result<()> {
        self.inner.start(name)
    }
    fn pause(&self) -> Result<()> {
        self.inner.pause()
    }
    fn resume(&self) -> Result<()> {
        self.inner.resume()
    }
    fn cancel(&self) -> Result<()> {
        self.inner.cancel()
    }
    fn job_thumbnail(&self, job: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        self.inner.job_thumbnail(job, limit)
    }
    fn file_slicer(&self, name: &str) -> Result<Option<String>> {
        self.inner.file_slicer(name)
    }
    fn status(&self) -> Result<PrinterStatus> {
        self.inner.status()
    }
    fn cameras(&self) -> Result<Vec<CameraInfo>> {
        self.inner.cameras()
    }
    fn loaded_filaments(&self) -> Result<Vec<LoadedFilament>> {
        let Some(v) = self.task_status()? else {
            self.filaments.store(false, Ordering::Relaxed);
            return Ok(vec![]);
        };
        let config = &v["print_task_config"];
        let exists = config["filament_exist"].as_array();
        // Without `filament_exist` a slot counts as loaded when it reports a type.
        let Some(types) = exists.or(config["filament_type"].as_array()) else {
            self.filaments.store(false, Ordering::Relaxed);
            return Ok(vec![]);
        };
        self.filaments.store(true, Ordering::Relaxed);
        Ok(types
            .iter()
            .enumerate()
            .filter(|(_, v)| match exists {
                Some(_) => v.as_bool() == Some(true),
                None => string(v).is_some(),
            })
            .map(|(i, _)| LoadedFilament {
                tool: i as u32,
                material: string(&config["filament_type"][i]),
                colour: string(&config["filament_color_rgba"][i]),
            })
            .collect())
    }
    fn as_u1(&self) -> Option<&SnapmakerU1> {
        Some(self)
    }
}
