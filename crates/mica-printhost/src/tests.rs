// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use crate::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
#[derive(Debug)]
struct Reply {
    code: u16,
    body: String,
    headers: Vec<(&'static str, String)>,
    delay: Duration,
}
impl Reply {
    fn json(v: Value) -> Self {
        Self {
            code: 200,
            body: v.to_string(),
            headers: vec![],
            delay: Duration::ZERO,
        }
    }
    fn code(code: u16) -> Self {
        Self {
            code,
            body: String::new(),
            headers: vec![],
            delay: Duration::ZERO,
        }
    }
}
struct Server {
    address: String,
    handle: Option<thread::JoinHandle<Vec<Request>>>,
    memory: Option<std::sync::Arc<std::sync::Mutex<Memory>>>,
}
impl Server {
    fn new(replies: Vec<Reply>) -> Self {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Self::in_memory(replies);
            }
            Err(e) => panic!("cannot bind mock: {e}"),
        };
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let handle = thread::spawn(move || {
            let mut requests = vec![];
            for reply in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing mock request");
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                // macOS/BSD: an accepted socket inherits the listener's non-blocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                requests.push(read_request(&mut stream));
                thread::sleep(reply.delay);
                let mut response = format!(
                    "HTTP/1.1 {} Mock\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.code,
                    reply.body.len()
                );
                for (key, value) in reply.headers {
                    response.push_str(&format!("{key}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&reply.body);
                let _ = stream.write_all(response.as_bytes());
            }
            requests
        });
        Self {
            address,
            handle: Some(handle),
            memory: None,
        }
    }
    fn config(&self, kind: PrinterKind) -> PrinterConfig {
        PrinterConfig {
            name: "test".into(),
            kind,
            address: self.address.clone(),
            credentials: Credentials::ApiKey("private-key".into()),
        }
    }
    fn finish(self) -> Vec<Request> {
        if let Some(memory) = self.memory {
            let mut m = memory.lock().unwrap();
            assert!(
                m.replies.is_empty(),
                "unconsumed mock responses: {}",
                m.replies.len()
            );
            registry().lock().unwrap().remove(&self.address);
            std::mem::take(&mut m.requests)
        } else {
            self.handle.unwrap().join().unwrap()
        }
    }
}
fn read_request(s: &mut impl Read) -> Request {
    let mut data = Vec::new();
    let mut byte = [0];
    while !data.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).unwrap();
        data.push(byte[0]);
        assert!(data.len() < 16384);
    }
    let text = String::from_utf8(data).unwrap();
    let mut lines = text.split("\r\n");
    let first: Vec<_> = lines.next().unwrap().split(' ').collect();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    let length = headers
        .get("content-length")
        .map(|v| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = vec![0; length];
    s.read_exact(&mut body).unwrap();
    Request {
        method: first[0].into(),
        path: first[1].into(),
        headers,
        body,
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new(data: &[u8]) -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mica-host-{}-{}.gcode",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, data).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn prusa_idle() -> Value {
    json!({"printer":{"state":"IDLE","temp_nozzle":24.1,"target_nozzle":0,"temp_bed":23.2,"target_bed":0},"job":null,"new_firmware_field":42})
}
fn objects(count: u32, extra: &[&str]) -> Reply {
    let mut names: Vec<String> = (0..count)
        .map(|t| {
            if t == 0 {
                "extruder".into()
            } else {
                format!("extruder{t}")
            }
        })
        .collect();
    names.extend(
        [
            "print_stats",
            "virtual_sdcard",
            "heater_bed",
            "motion_report",
        ]
        .map(str::to_owned),
    );
    names.extend(extra.iter().map(|s| s.to_string()));
    Reply::json(json!({"result":{"objects":names}}))
}
fn moon_status(state: &str, count: u32) -> Reply {
    let mut status = json!({"print_stats":{"state":state,"filename":"cube.gcode","print_duration":120,"info":{"current_layer":12}},"virtual_sdcard":{"progress":0.25},"heater_bed":{"temperature":59.9,"target":60},"motion_report":{"live_position":[1,2,2.4]},"temperature_sensor chamber":{"temperature":32.5}});
    for tool in 0..count {
        status[if tool == 0 {
            "extruder".into()
        } else {
            format!("extruder{tool}")
        }] = json!({"temperature":210.0+tool as f64,"target":215});
    }
    Reply::json(json!({"result":{"status":status}}))
}
#[test]
fn prusa_binary_upload_is_byte_exact_and_reports_progress() {
    let data = b"GCDE\0\x01\xff\r\nG1 X5\n";
    let file = Temp::new(data);
    let server = Server::new(vec![Reply::code(201)]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let mut ticks = vec![];
    let job = host
        .upload_with_progress(&file.0, "folder/cube µ.bgcode", true, &mut |p| {
            ticks.push((p.sent, p.total))
        })
        .unwrap();
    assert!(job.started);
    assert_eq!(ticks.first(), Some(&(0, data.len() as u64)));
    assert_eq!(ticks.last(), Some(&(data.len() as u64, data.len() as u64)));
    let req = server.finish().pop().unwrap();
    assert_eq!(req.method, "PUT");
    assert_eq!(req.path, "/api/v1/files/usb/folder/cube%20%C2%B5.bgcode");
    assert_eq!(req.body, data);
    assert_eq!(req.headers["x-api-key"], "private-key");
    assert_eq!(req.headers["print-after-upload"], "?1");
    assert_eq!(req.headers["overwrite-file"], "?1");
}
#[test]
fn prusa_upload_only_and_controls() {
    let server = Server::new(vec![
        Reply::code(201),
        Reply::code(204),
        Reply::json(json!({"id":42})),
        Reply::code(204),
        Reply::json(json!({"id":42})),
        Reply::code(204),
        Reply::json(json!({"id":42})),
        Reply::code(204),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let file = Temp::new(b"G1 X1\n");
    assert!(!host.upload(&file.0, "cube.gcode", false).unwrap().started);
    host.start("cube.gcode").unwrap();
    host.pause().unwrap();
    host.resume().unwrap();
    host.cancel().unwrap();
    let r = server.finish();
    assert_eq!(r[0].headers["print-after-upload"], "?0");
    assert_eq!(
        (&*r[1].method, &*r[1].path),
        ("POST", "/api/v1/files/usb/cube.gcode")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&r[1].body).unwrap(),
        json!({"command":"start"})
    );
    assert_eq!(
        (&*r[3].method, &*r[3].path),
        ("PUT", "/api/v1/job/42/pause")
    );
    assert_eq!(r[5].path, "/api/v1/job/42/resume");
    assert_eq!((&*r[7].method, &*r[7].path), ("DELETE", "/api/v1/job/42"));
}
#[test]
fn prusa_states_progress_and_job_name() {
    let server = Server::new(vec![
        Reply::json(prusa_idle()),
        Reply::json(
            json!({"printer":{"state":"PRINTING","axis_z":1.2},"job":{"id":1,"progress":37,"time_printing":90,"time_remaining":180,"current_layer":6}}),
        ),
        Reply::json(json!({"id":1,"file":{"display_name":"cube.bgcode"}})),
        Reply::json(json!({"printer":{"state":"ERROR"}})),
        Reply::json(json!({"printer":{"state":"new_state"}})),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let idle = host.status().unwrap();
    assert_eq!(idle.state, PrinterState::Idle);
    assert_eq!(idle.hotends[0].temperature.current, Some(24.1));
    assert!(idle.progress.is_none());
    let s = host.status().unwrap();
    assert_eq!(s.state, PrinterState::Printing);
    assert_eq!(s.job_name.as_deref(), Some("cube.bgcode"));
    assert_eq!(s.progress, Some(37.));
    assert_eq!(s.elapsed, Some(90.));
    assert_eq!(s.remaining, Some(180.));
    assert_eq!(s.z, Some(1.2));
    assert_eq!(s.current_layer, Some(6));
    assert_eq!(host.status().unwrap().state, PrinterState::Error);
    assert_eq!(host.status().unwrap().state, PrinterState::Error);
    server.finish();
}
#[test]
fn xl_five_tools_and_optional_materials() {
    let mut status = prusa_idle();
    let mut tools = json!({});
    for t in 1..=5 {
        tools[t.to_string()] =
            json!({"temp_nozzle":20+t,"target_nozzle":0,"material":"PLA","colour":"#FF0000"});
    }
    status["printer"]["tools"] = tools.clone();
    let server = Server::new(vec![
        Reply::json(json!({"printer_model":"XL","firmware":"7.0.0","tools":tools})),
        Reply::json(status.clone()),
        Reply::json(status),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    assert_eq!(host.info().unwrap().tools, Some(vec![0, 1, 2, 3, 4]));
    let loaded = host.loaded_filaments().unwrap();
    assert_eq!(loaded.len(), 5);
    assert_eq!(loaded[4].tool, 4);
    assert_eq!(loaded[4].material.as_deref(), Some("PLA"));
    assert!(host.capabilities().filaments);
    server.finish();
}
#[test]
fn legacy_status_fallback_does_not_hide_auth_errors() {
    let server = Server::new(vec![
        Reply::code(404),
        Reply::json(
            json!({"state":{"text":"Operational","flags":{"operational":true}},"temperature":{"tool0":{"actual":23.0,"target":0},"bed":{"actual":22,"target":0}}}),
        ),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    assert_eq!(host.status().unwrap().state, PrinterState::Idle);
    assert!(!host.capabilities().upload);
    let r = server.finish();
    assert_eq!(r[1].path, "/api/printer");
    let server = Server::new(vec![Reply::code(401)]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    assert_eq!(host.status().unwrap_err().kind, ErrorKind::Unauthorised);
    assert_eq!(server.finish().len(), 1);
}
#[test]
fn moonraker_multipart_payload_and_print_confirmation() {
    let data = b"G1 X10\r\n\0\xff\n";
    let file = Temp::new(data);
    let server = Server::new(vec![Reply::json(
        json!({"item":{"path":"folder/cube.gcode"},"print_started":true}),
    )]);
    let host = Moonraker::new(server.config(PrinterKind::Moonraker), Timeouts::default()).unwrap();
    let mut ticks = vec![];
    assert!(
        host.upload_with_progress(&file.0, "folder/cube.gcode", true, &mut |p| ticks
            .push(p.sent))
            .unwrap()
            .started
    );
    assert_eq!(ticks.last(), Some(&(data.len() as u64)));
    let r = server.finish().pop().unwrap();
    assert_eq!((&*r.method, &*r.path), ("POST", "/server/files/upload"));
    let boundary = r.headers["content-type"]
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let mut expected=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"root\"\r\n\r\ngcodes\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\nfolder\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"print\"\r\n\r\ntrue\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"cube.gcode\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    expected.extend(data);
    expected.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    assert_eq!(r.body, expected);
    assert_eq!(r.headers["x-api-key"], "private-key");
}
#[test]
fn moonraker_no_auth_upload_only_and_controls() {
    let server = Server::new(vec![
        Reply::json(json!({"result":{"print_started":false}})),
        Reply::json(json!({"result":"ok"})),
        Reply::json(json!({"result":"ok"})),
        Reply::json(json!({"result":"ok"})),
        Reply::json(json!({"result":"ok"})),
    ]);
    let mut config = server.config(PrinterKind::Moonraker);
    config.credentials = Credentials::None;
    let host = Moonraker::new(config, Timeouts::default()).unwrap();
    let file = Temp::new(b"G28\n");
    assert!(!host.upload(&file.0, "cube.gcode", false).unwrap().started);
    host.start("folder/a b.gcode").unwrap();
    host.pause().unwrap();
    host.resume().unwrap();
    host.cancel().unwrap();
    let r = server.finish();
    assert!(!r[0].headers.contains_key("x-api-key"));
    assert!(!r[2].headers.contains_key("content-type"));
    assert_eq!(r[1].headers["content-type"], "application/json");
    assert!(!String::from_utf8_lossy(&r[0].body).contains("name=\"print\""));
    assert_eq!(
        serde_json::from_slice::<Value>(&r[1].body).unwrap(),
        json!({"filename":"folder/a b.gcode"})
    );
    assert_eq!(
        r.iter()
            .skip(1)
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/printer/print/start",
            "/printer/print/pause",
            "/printer/print/resume",
            "/printer/print/cancel"
        ]
    );
}
#[test]
fn moonraker_discovery_four_tools_and_states() {
    let server = Server::new(vec![
        objects(4, &["temperature_sensor chamber", "extruder_stepper spare"]),
        moon_status("printing", 4),
        moon_status("paused", 4),
        moon_status("complete", 4),
        moon_status("standby", 4),
        moon_status("error", 4),
    ]);
    let host =
        SnapmakerU1::new(server.config(PrinterKind::SnapmakerU1), Timeouts::default()).unwrap();
    let s = host.status().unwrap();
    assert_eq!(s.hotends.len(), 4);
    assert_eq!(s.hotends[3].tool, 3);
    assert_eq!(s.progress, Some(25.));
    assert_eq!(s.remaining, Some(360.));
    assert_eq!(s.z, Some(2.4));
    assert_eq!(s.current_layer, Some(12));
    assert_eq!(s.chamber.unwrap().current, Some(32.5));
    for state in [
        PrinterState::Paused,
        PrinterState::Finished,
        PrinterState::Idle,
        PrinterState::Error,
    ] {
        assert_eq!(host.status().unwrap().state, state);
    }
    let r = server.finish();
    assert!(r[1].path.contains("extruder3"));
    assert!(!r[1].path.contains("extruder_stepper"));
    assert!(!r[1].path.contains("extruder4"));
    assert!(r[1].path.contains("temperature_sensor%20chamber"));
}
#[test]
fn moonraker_info_and_disconnected_klippy() {
    let server = Server::new(vec![
        Reply::json(json!({"result":{"klippy_state":"ready"}})),
        Reply::json(json!({"result":{"hostname":"voron","software_version":"v0.13"}})),
        objects(2, &[]),
        moon_status("standby", 2),
    ]);
    let host = Moonraker::new(server.config(PrinterKind::Moonraker), Timeouts::default()).unwrap();
    let info = host.info().unwrap();
    assert_eq!(info.tools, Some(vec![0, 1]));
    assert_eq!(info.firmware.as_deref(), Some("v0.13"));
    server.finish();
    let server = Server::new(vec![Reply::json(
        json!({"result":{"klippy_state":"disconnected"}}),
    )]);
    let host = Moonraker::new(server.config(PrinterKind::Moonraker), Timeouts::default()).unwrap();
    assert_eq!(host.info().unwrap().state, PrinterState::Offline);
    server.finish();
}
#[test]
fn u1_optional_rfid_and_camera() {
    let server = Server::new(vec![
        objects(4, &["print_task_config", "filament_detect"]),
        Reply::json(
            json!({"result":{"status":{"print_task_config":{"filament_exist":[true,true,false,true],"filament_type":["PLA","PETG"],"filament_color_rgba":["FF0000FF","00FF00FF"]},"filament_detect":{"info":[]}}}}),
        ),
        Reply::json(
            json!({"result":{"webcams":[{"enabled":true,"snapshot_url":"/webcam/?action=snapshot"}]}}),
        ),
    ]);
    let host =
        SnapmakerU1::new(server.config(PrinterKind::SnapmakerU1), Timeouts::default()).unwrap();
    let f = host.loaded_filaments().unwrap();
    assert_eq!(f.len(), 3);
    assert_eq!(f[1].material.as_deref(), Some("PETG"));
    assert_eq!(f[1].colour.as_deref(), Some("00FF00FF"));
    assert_eq!(f[2].tool, 3);
    assert_eq!(f[2].material, None);
    assert!(host.capabilities().filaments);
    assert_eq!(
        host.camera_snapshot_url().unwrap(),
        Some(format!("{}/webcam/?action=snapshot", server.address))
    );
    assert!(host.capabilities().camera);
    let r = server.finish();
    assert_eq!(
        r[1].path,
        "/printer/objects/query?print_task_config&filament_detect"
    );
}
#[test]
fn prusalink_cameras_buddy_has_none_and_prusa_link_snapshots_use_the_key() {
    // Buddy firmware (XL, CORE One): 404, which means "no cameras".
    let server = Server::new(vec![Reply::code(404)]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    assert!(host.cameras().unwrap().is_empty());
    server.finish();
    // Prusa-Link on a Raspberry Pi: a camera list, snapshots through the authenticated API.
    let mut jpeg = Reply::code(200);
    jpeg.body = "\u{ff}JPEGDATA".into();
    let server = Server::new(vec![
        Reply::json(
            json!({"camera_list":[{"camera_id":"cam1","config":{"name":"Pi cam"},"connected":true,"detected":true,"stored":true,"registered":false}]}),
        ),
        jpeg,
        Reply::code(204),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let cams = host.cameras().unwrap();
    assert_eq!(cams.len(), 1);
    let path = cams[0].printer_snapshot.clone().unwrap();
    let bytes = host.camera_snapshot(&path, 1024).unwrap().unwrap();
    assert!(bytes.ends_with(b"JPEGDATA"));
    // 204: no photo taken yet.
    assert_eq!(host.camera_snapshot(&path, 1024).unwrap(), None);
    // Anything but a snapshot path is refused without a request.
    assert!(host.camera_snapshot("/api/v1/job", 1024).is_err());
    assert!(
        host.camera_snapshot("/api/v1/cameras/../snap", 1024)
            .is_err()
    );
    let r = server.finish();
    assert_eq!(r[1].method, "GET");
    assert_eq!(r[1].path, "/api/v1/cameras/cam1/snap");
    assert_eq!(
        r[1].headers.get("x-api-key").map(String::as_str),
        Some("private-key")
    );
}
#[test]
fn job_thumbnails_from_moonraker_metadata_and_prusalink_refs() {
    let mut png = Reply::code(200);
    png.body = "\u{89}PNGDATA".into();
    // Moonraker: the largest thumbnail up to 400 px, next to the file in its folder.
    let server = Server::new(vec![
        Reply::json(json!({"result":{"size":1,"thumbnails":[
            {"width":32,"height":32,"relative_path":".thumbs/a b-32x32.png"},
            {"width":300,"height":300,"relative_path":".thumbs/a b-300x300.png"},
            {"width":600,"height":600,"relative_path":".thumbs/a b-600x600.png"}]}})),
        png,
        Reply::json(json!({"result":{"size":1}})),
        Reply::code(404),
    ]);
    let host = Moonraker::new(server.config(PrinterKind::Moonraker), Timeouts::default()).unwrap();
    let bytes = host.job_thumbnail("jobs/a b.gcode", 1024).unwrap().unwrap();
    assert!(bytes.ends_with(b"PNGDATA"));
    // No thumbnails in the file, or a file Moonraker does not know: none, not an error.
    assert_eq!(host.job_thumbnail("plain.gcode", 1024).unwrap(), None);
    assert_eq!(host.job_thumbnail("gone.gcode", 1024).unwrap(), None);
    assert!(host.job_thumbnail("../x.gcode", 1024).is_err());
    let r = server.finish();
    assert_eq!(
        r[0].path,
        "/server/files/metadata?filename=jobs%2Fa%20b.gcode"
    );
    assert_eq!(
        r[1].path,
        "/server/files/gcodes/jobs/.thumbs/a%20b-300x300.png"
    );
    // PrusaLink: the job's `refs.thumbnail`, with the key; no job (204) is none.
    let mut png = Reply::code(200);
    png.body = "\u{89}PNGDATA".into();
    let server = Server::new(vec![
        Reply::json(
            json!({"id":42,"file":{"name":"BENCHY~1.BGC","refs":{"icon":"/thumb/s/usb/BENCHY~1.BGC","thumbnail":"/thumb/l/usb/BENCHY~1.BGC"}}}),
        ),
        png,
        Reply::code(204),
        Reply::json(json!({"id":42,"file":{"refs":{"thumbnail":"/api/v1/files/x"}}})),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let bytes = host.job_thumbnail("Benchy.bgcode", 1024).unwrap().unwrap();
    assert!(bytes.ends_with(b"PNGDATA"));
    assert_eq!(host.job_thumbnail("Benchy.bgcode", 1024).unwrap(), None);
    // Only `/thumb/` paths are fetched.
    assert_eq!(host.job_thumbnail("Benchy.bgcode", 1024).unwrap(), None);
    let r = server.finish();
    assert_eq!(r[1].path, "/thumb/l/usb/BENCHY~1.BGC");
    assert_eq!(
        r[1].headers.get("x-api-key").map(String::as_str),
        Some("private-key")
    );
}
#[test]
fn u1_without_extensions_degrades_gracefully() {
    let server = Server::new(vec![objects(4, &[]), Reply::code(404)]);
    let host =
        SnapmakerU1::new(server.config(PrinterKind::SnapmakerU1), Timeouts::default()).unwrap();
    assert!(host.loaded_filaments().unwrap().is_empty());
    assert_eq!(host.camera_snapshot_url().unwrap(), None);
    assert!(!host.capabilities().filaments);
    assert!(!host.capabilities().camera);
    server.finish();
}
#[test]
fn errors_timeouts_busy_and_unreachable_are_typed() {
    for (code, kind) in [
        (401, ErrorKind::Unauthorised),
        (403, ErrorKind::Unauthorised),
        (409, ErrorKind::Busy),
        (500, ErrorKind::Protocol),
        (302, ErrorKind::Protocol),
    ] {
        let server = Server::new(vec![Reply::code(code)]);
        let host =
            Moonraker::new(server.config(PrinterKind::Moonraker), Timeouts::default()).unwrap();
        assert_eq!(host.pause().unwrap_err().kind, kind);
        server.finish();
    }
    let mut reply = Reply::json(prusa_idle());
    reply.delay = Duration::from_millis(180);
    let server = Server::new(vec![reply]);
    let limits = Timeouts {
        request: Duration::from_millis(30),
        ..Timeouts::default()
    };
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), limits).unwrap();
    let start = Instant::now();
    assert_eq!(host.status().unwrap_err().kind, ErrorKind::Unreachable);
    assert!(start.elapsed() < Duration::from_secs(1));
    server.finish();
    let address = "127.0.0.1:1";
    let config = PrinterConfig {
        name: "missing".into(),
        kind: PrinterKind::PrusaLink,
        address: address.to_string(),
        credentials: Credentials::None,
    };
    let host = connect(config, Timeouts::default()).unwrap();
    assert_eq!(host.status().unwrap_err().kind, ErrorKind::Unreachable);
}
#[test]
fn malformed_json_missing_state_and_rpc_error() {
    let mut invalid = Reply::code(200);
    invalid.body = "{bad-json".into();
    let server = Server::new(vec![
        invalid,
        Reply::json(json!({})),
        Reply::json(json!({"error":{"code":500,"message":"secret"}})),
    ]);
    let config = server.config(PrinterKind::PrusaLink);
    let prusa = PrusaLink::new(config.clone(), Timeouts::default()).unwrap();
    assert_eq!(prusa.status().unwrap_err().kind, ErrorKind::Protocol);
    assert_eq!(prusa.status().unwrap_err().kind, ErrorKind::Protocol);
    let moon = Moonraker::new(config, Timeouts::default()).unwrap();
    let error = moon.pause().unwrap_err();
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert!(!error.to_string().contains("secret"));
    server.finish();
}
#[test]
fn digest_challenge_is_answered_and_secrets_redacted() {
    let mut challenge = Reply::code(401);
    challenge.headers.push((
        "WWW-Authenticate",
        "Digest realm=\"PrusaLink\", nonce=\"abcdef\", qop=\"auth,auth-int\", algorithm=MD5".into(),
    ));
    let server = Server::new(vec![challenge, Reply::json(prusa_idle())]);
    let mut config = server.config(PrinterKind::PrusaLink);
    config.credentials = Credentials::Digest {
        username: "maker".into(),
        password: "private-password".into(),
    };
    assert!(!format!("{config:?}").contains("private-password"));
    let host = PrusaLink::new(config, Timeouts::default()).unwrap();
    host.status().unwrap();
    let r = server.finish();
    assert!(!r[0].headers.contains_key("authorization"));
    assert!(r[1].headers["authorization"].contains("uri=\"/api/v1/status\""));
    assert!(r[1].headers["authorization"].contains("qop=auth"));
    assert!(!r[1].headers["authorization"].contains("private-password"));
}
#[test]
fn invalid_inputs_fail_without_network_requests() {
    let config = PrinterConfig {
        name: "test".into(),
        kind: PrinterKind::PrusaLink,
        address: "http://127.0.0.1:1".into(),
        credentials: Credentials::None,
    };
    let prusa = PrusaLink::new(config.clone(), Timeouts::default()).unwrap();
    let moon = Moonraker::new(config.clone(), Timeouts::default()).unwrap();
    for name in [
        "../x.gcode",
        "/x.gcode",
        "a//x.gcode",
        "a\r\nx.gcode",
        "a\\x.gcode",
        "a\"x.gcode",
    ] {
        assert_eq!(
            prusa
                .upload(std::path::Path::new("unused"), name, false)
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
        assert_eq!(moon.start(name).unwrap_err().kind, ErrorKind::Protocol);
    }
    assert!(
        moon.upload(std::path::Path::new("unused"), "x.bgcode", false)
            .is_err()
    );
    for address in [
        "ftp://printer",
        "http://user:secret@printer",
        "http://printer/?api_key=secret",
        "http://printer/#secret",
        "",
    ] {
        let mut c = config.clone();
        c.address = address.into();
        assert!(connect(c, Timeouts::default()).is_err());
    }
}

// A test-only HTTP byte transport for environments that deny loopback sockets.
// The same ureq HTTP encoder/decoder and all assertions run; no production transport is replaced.
use std::{
    collections::VecDeque,
    io::Cursor,
    sync::{Arc, Mutex, OnceLock},
};
use ureq::unversioned::{
    resolver::DefaultResolver,
    transport::{Buffers, ConnectionDetails, Connector, LazyBuffers, NextTimeout, Transport},
};
#[derive(Debug)]
struct Memory {
    replies: VecDeque<Reply>,
    requests: Vec<Request>,
}
type Registry = Mutex<BTreeMap<String, Arc<Mutex<Memory>>>>;
fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}
impl Server {
    fn in_memory(replies: Vec<Reply>) -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let address = format!(
            "http://127.0.0.1:{}",
            30000 + ID.fetch_add(1, Ordering::Relaxed)
        );
        let memory = Arc::new(Mutex::new(Memory {
            replies: replies.into(),
            requests: vec![],
        }));
        registry()
            .lock()
            .unwrap()
            .insert(address.clone(), memory.clone());
        Self {
            address,
            handle: None,
            memory: Some(memory),
        }
    }
}
pub(crate) fn memory_agent(base: &str, config: ureq::config::Config) -> Option<ureq::Agent> {
    registry().lock().unwrap().get(base).map(|m| {
        ureq::Agent::with_parts(
            config,
            MemoryConnector(m.clone()),
            DefaultResolver::default(),
        )
    })
}
#[derive(Debug)]
struct MemoryConnector(Arc<Mutex<Memory>>);
impl Connector for MemoryConnector {
    type Out = MemoryTransport;
    fn connect(
        &self,
        _: &ConnectionDetails,
        _: Option<()>,
    ) -> std::result::Result<Option<Self::Out>, ureq::Error> {
        let reply = self
            .0
            .lock()
            .unwrap()
            .replies
            .pop_front()
            .ok_or(ureq::Error::ConnectionFailed)?;
        Ok(Some(MemoryTransport {
            memory: self.0.clone(),
            reply: Some(reply),
            request: vec![],
            response: Cursor::new(vec![]),
            buffers: LazyBuffers::new(16384, 16384),
        }))
    }
}
#[derive(Debug)]
struct MemoryTransport {
    memory: Arc<Mutex<Memory>>,
    reply: Option<Reply>,
    request: Vec<u8>,
    response: Cursor<Vec<u8>>,
    buffers: LazyBuffers,
}
impl Transport for MemoryTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }
    fn transmit_output(
        &mut self,
        amount: usize,
        _: NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        self.request
            .extend_from_slice(&self.buffers.output()[..amount]);
        Ok(())
    }
    fn await_input(&mut self, timeout: NextTimeout) -> std::result::Result<bool, ureq::Error> {
        if let Some(reply) = self.reply.take() {
            self.memory
                .lock()
                .unwrap()
                .requests
                .push(read_request(&mut Cursor::new(&self.request)));
            if reply.delay > *timeout.after {
                thread::sleep(*timeout.after);
                return Err(ureq::Error::Timeout(timeout.reason));
            }
            thread::sleep(reply.delay);
            let mut response = format!(
                "HTTP/1.1 {} Mock\r\nContent-Length: {}\r\nConnection: close\r\n",
                reply.code,
                reply.body.len()
            );
            for (key, value) in reply.headers {
                response.push_str(&format!("{key}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&reply.body);
            self.response = Cursor::new(response.into_bytes());
        }
        let n = self.response.read(self.buffers.input_append_buf())?;
        self.buffers.input_appended(n);
        Ok(n > 0)
    }
    fn is_open(&mut self) -> bool {
        false
    }
}

#[test]
fn preflight_over_http_warns_for_wrong_tools_plate_and_filament() {
    let mut status = prusa_idle();
    status["printer"]["state"] = json!("PRINTING");
    status["job"] = json!({"name":"existing.bgcode"});
    status["printer"]["tools"] = json!({"1":{"material":"PETG","colour":"#FF0000"}});
    let server = Server::new(vec![
        Reply::json(status.clone()),
        Reply::json(json!({"printer_model":"XL","tools":{"1":{}},"bed_type":"Smooth"})),
        Reply::json(status.clone()),
        Reply::json(status),
    ]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let job = JobRequirements {
        tools: vec![
            ToolRequirement {
                tool: 0,
                material: Some("PLA".into()),
                colour: Some("#00FF00".into()),
            },
            ToolRequirement {
                tool: 4,
                material: None,
                colour: None,
            },
        ],
        format: FileFormat::Bgcode,
        bed_type: Some("Satin".into()),
        needs_chamber_sensor: true,
    };
    let issues = host.preflight(&job);
    for kind in [
        IssueKind::NotIdle,
        IssueKind::MissingTool,
        IssueKind::Material,
        IssueKind::Colour,
        IssueKind::BedType,
        IssueKind::ChamberSensor,
    ] {
        assert!(
            issues.iter().any(|i| i.kind == kind),
            "{kind:?}: {issues:?}"
        );
    }
    assert!(server.finish().iter().all(|r| r.method == "GET"));
}
struct AdvisoryHost {
    state: PrinterState,
    tools: Option<Vec<u32>>,
    bed: Option<String>,
    filaments: Vec<LoadedFilament>,
}
impl PrinterHost for AdvisoryHost {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn info(&self) -> Result<PrinterInfo> {
        Ok(PrinterInfo {
            model: None,
            firmware: None,
            state: self.state,
            tools: self.tools.clone(),
            formats: vec![FileFormat::Gcode],
            bed_type: self.bed.clone(),
        })
    }
    fn status(&self) -> Result<PrinterStatus> {
        Ok(PrinterStatus::empty(self.state))
    }
    fn loaded_filaments(&self) -> Result<Vec<LoadedFilament>> {
        Ok(self.filaments.clone())
    }
    fn upload_with_progress(
        &self,
        _: &std::path::Path,
        _: &str,
        _: bool,
        _: &mut ProgressCallback<'_>,
    ) -> Result<Job> {
        Err(Error::protocol("unused"))
    }
    fn start(&self, _: &str) -> Result<()> {
        Err(Error::protocol("unused"))
    }
    fn pause(&self) -> Result<()> {
        Err(Error::protocol("unused"))
    }
    fn resume(&self) -> Result<()> {
        Err(Error::protocol("unused"))
    }
    fn cancel(&self) -> Result<()> {
        Err(Error::protocol("unused"))
    }
}
#[test]
fn preflight_unknowns_format_and_colour_normalization() {
    let mut h = AdvisoryHost {
        state: PrinterState::Idle,
        tools: None,
        bed: None,
        filaments: vec![],
    };
    let mut job = JobRequirements {
        tools: vec![ToolRequirement {
            tool: 0,
            material: Some("PLA".into()),
            colour: Some("#FF0000".into()),
        }],
        format: FileFormat::Bgcode,
        bed_type: Some("Satin".into()),
        needs_chamber_sensor: false,
    };
    let issues = h.preflight(&job);
    for kind in [
        IssueKind::UnknownTools,
        IssueKind::FileFormat,
        IssueKind::UnknownBed,
        IssueKind::UnknownFilament,
    ] {
        assert!(issues.iter().any(|i| i.kind == kind));
    }
    h.tools = Some(vec![0]);
    h.bed = Some("satin".into());
    h.filaments = vec![LoadedFilament {
        tool: 0,
        material: Some("pla".into()),
        colour: Some("FF0000FF".into()),
    }];
    job.format = FileFormat::Gcode;
    assert!(h.preflight(&job).is_empty());
    h.state = PrinterState::Offline;
    assert!(
        h.preflight(&job)
            .iter()
            .any(|i| i.kind == IssueKind::NotIdle)
    );
}
#[test]
fn preflight_auth_failure_is_a_connection_warning() {
    let server = Server::new(vec![Reply::code(401)]);
    let h = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let issues = h.preflight(&JobRequirements {
        tools: vec![],
        format: FileFormat::Gcode,
        bed_type: None,
        needs_chamber_sensor: false,
    });
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, IssueKind::Connection);
    server.finish();
}
#[test]
fn digest_upload_and_failed_credentials() {
    let challenge = || {
        let mut r = Reply::code(401);
        r.headers.push((
            "WWW-Authenticate",
            "Digest realm=\"Buddy\", nonce=\"abc\", qop=\"auth\"".into(),
        ));
        r
    };
    let server = Server::new(vec![challenge(), Reply::code(201)]);
    let mut config = server.config(PrinterKind::PrusaLink);
    config.credentials = Credentials::Digest {
        username: "user".into(),
        password: "password".into(),
    };
    let h = PrusaLink::new(config, Timeouts::default()).unwrap();
    let f = Temp::new(b"G28\n");
    h.upload(&f.0, "cube.bgcode", false).unwrap();
    let r = server.finish();
    assert_eq!(r[0].method, "GET");
    assert_eq!(r[0].path, "/api/printer");
    assert_eq!(r[1].body, b"G28\n");
    assert!(r[1].headers["authorization"].contains("uri=\"/api/v1/files/usb/cube.bgcode\""));
    let server = Server::new(vec![challenge(), challenge()]);
    let mut config = server.config(PrinterKind::PrusaLink);
    config.credentials = Credentials::Digest {
        username: "wrong".into(),
        password: "wrong".into(),
    };
    let h = PrusaLink::new(config, Timeouts::default()).unwrap();
    assert_eq!(h.status().unwrap_err().kind, ErrorKind::Unauthorised);
    server.finish();
}
#[test]
fn large_upload_streams_file_bytes_and_uses_upload_timeout() {
    let data = vec![b'X'; 512 * 1024];
    let file = Temp::new(&data);
    let mut reply = Reply::code(201);
    reply.delay = Duration::from_millis(100);
    let server = Server::new(vec![reply]);
    let h = PrusaLink::new(
        server.config(PrinterKind::PrusaLink),
        Timeouts {
            upload: Duration::from_millis(25),
            request: Duration::from_secs(1),
            ..Timeouts::default()
        },
    )
    .unwrap();
    assert_eq!(
        h.upload(&file.0, "large.gcode", false).unwrap_err().kind,
        ErrorKind::Unreachable
    );
    assert_eq!(server.finish()[0].body, data);
}
#[test]
fn legacy_info_version_and_temperatures() {
    let server = Server::new(vec![
        Reply::code(404),
        Reply::json(json!({"server":"1.0","text":"PrusaLink"})),
        Reply::code(404),
        Reply::json(
            json!({"state":{"flags":{"paused":true}},"temperature":{"tool0":{"actual":200,"target":210}}}),
        ),
    ]);
    let h = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let info = h.info().unwrap();
    assert_eq!(info.state, PrinterState::Paused);
    assert_eq!(info.firmware.as_deref(), Some("1.0"));
    assert_eq!(info.tools, None);
    assert!(!h.capabilities().start);
    server.finish();
}

#[path = "discovery_tests.rs"]
mod discovery_tests;

#[path = "connect_tests.rs"]
mod connect_tests;

#[test]
fn preflight_preserves_earlier_warnings_when_inventory_read_fails() {
    let mut busy = prusa_idle();
    busy["printer"]["state"] = json!("PRINTING");
    busy["job"] = json!({"name":"existing.bgcode"});
    let server = Server::new(vec![Reply::json(busy), Reply::code(401)]);
    let host = PrusaLink::new(server.config(PrinterKind::PrusaLink), Timeouts::default()).unwrap();
    let warnings = host.preflight(&JobRequirements {
        tools: vec![ToolRequirement {
            tool: 0,
            material: Some("PLA".into()),
            colour: None,
        }],
        format: FileFormat::Gcode,
        bed_type: None,
        needs_chamber_sensor: true,
    });
    assert_eq!(
        warnings.iter().map(|issue| issue.kind).collect::<Vec<_>>(),
        vec![
            IssueKind::NotIdle,
            IssueKind::ChamberSensor,
            IssueKind::Connection
        ]
    );
    assert!(
        server
            .finish()
            .iter()
            .all(|request| request.method == "GET")
    );
}

#[test]
fn preflight_requires_reported_t0_and_distinguishes_empty_from_unknown_inventory() {
    let mut host = AdvisoryHost {
        state: PrinterState::Finished,
        tools: Some(vec![]),
        bed: None,
        filaments: vec![],
    };
    let job = JobRequirements {
        tools: vec![ToolRequirement {
            tool: 0,
            material: None,
            colour: None,
        }],
        format: FileFormat::Gcode,
        bed_type: None,
        needs_chamber_sensor: false,
    };
    let warnings = host.preflight(&job);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].kind, IssueKind::MissingTool);
    assert_eq!(warnings[0].tool, Some(0));
    host.tools = None;
    assert_eq!(host.preflight(&job)[0].kind, IssueKind::UnknownTools);
}
