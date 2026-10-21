// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A recording mock of the U1's Moonraker (HTTP and the WebSocket) for the mapped start:
//! every request is kept with its exact body, so the tests pin what Mica sends for both
//! firmware routes and for the fallbacks.

use crate::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
struct Call {
    method: String,
    path: String,
    /// The JSON body (HTTP) or the WebSocket message.
    body: Value,
}

#[derive(Clone)]
struct Behaviour {
    firmware: Option<&'static str>,
    /// `None`: no WebSocket (404 on `/websocket`).
    ws_reply: Option<Value>,
    /// Refuse the macro script with this Klipper error.
    macro_error: Option<&'static str>,
}

struct Mock {
    address: String,
    calls: Arc<Mutex<Vec<Call>>>,
    stop: Arc<Mutex<bool>>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Mock {
    /// `None` when this sandbox cannot listen on loopback.
    fn start(b: Behaviour) -> Option<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let address = format!("http://{}", listener.local_addr().ok()?);
        listener.set_nonblocking(true).ok()?;
        let calls = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(Mutex::new(false));
        let (c, s) = (calls.clone(), stop.clone());
        let handle = thread::spawn(move || {
            while !*s.lock().unwrap() {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &b, &c),
                    Err(_) => thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        Some(Self {
            address,
            calls,
            stop,
            handle: Some(handle),
        })
    }
    fn host(&self) -> SnapmakerU1 {
        SnapmakerU1::new(
            PrinterConfig {
                name: "U1".into(),
                kind: PrinterKind::SnapmakerU1,
                address: self.address.clone(),
                credentials: Credentials::None,
            },
            Timeouts {
                connect: Duration::from_secs(2),
                request: Duration::from_secs(5),
                upload: Duration::from_secs(5),
            },
        )
        .unwrap()
    }
    fn finish(mut self) -> Vec<Call> {
        *self.stop.lock().unwrap() = true;
        if let Some(h) = self.handle.take() {
            h.join().unwrap();
        }
        self.calls.lock().unwrap().clone()
    }
}

/// Method, path, lower-cased headers and body of one HTTP request.
type Head = (String, String, Vec<(String, String)>, Vec<u8>);

fn read_head(s: &mut TcpStream) -> Option<Head> {
    let mut data = vec![];
    let mut byte = [0];
    while !data.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).ok()?;
        data.push(byte[0]);
    }
    let text = String::from_utf8(data).ok()?;
    let mut lines = text.split("\r\n");
    let first: Vec<_> = lines.next()?.split(' ').collect();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    s.read_exact(&mut body).ok()?;
    Some((first[0].into(), first[1].into(), headers, body))
}

fn serve(mut s: TcpStream, b: &Behaviour, calls: &Mutex<Vec<Call>>) {
    s.set_nonblocking(false).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let Some((method, path, headers, body)) = read_head(&mut s) else {
        return;
    };
    let reply = |s: &mut TcpStream, code: u16, v: Value| {
        let body = v.to_string();
        let _ = write!(
            s,
            "HTTP/1.1 {code} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    };
    if path == "/websocket" {
        let Some(ws) = &b.ws_reply else {
            return reply(
                &mut s,
                404,
                json!({"error":{"code":404,"message":"Not Found"}}),
            );
        };
        let key = headers
            .iter()
            .find(|(k, _)| k == "sec-websocket-key")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let _ = write!(
            s,
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_key(&key)
        );
        // Moonraker broadcasts notifications to every socket; the client must skip them.
        let note = json!({"jsonrpc":"2.0","method":"notify_proc_stat_update","params":[{}]});
        let _ = s.write_all(&server_text_frame(note.to_string().as_bytes()));
        let Ok(Some(msg)) = read_client_text(&mut s) else {
            return;
        };
        let v: Value = serde_json::from_slice(&msg).unwrap();
        let mut answer = ws.clone();
        answer["id"] = v["id"].clone();
        answer["jsonrpc"] = json!("2.0");
        calls.lock().unwrap().push(Call {
            method: "WS".into(),
            path,
            body: v,
        });
        let _ = s.write_all(&server_text_frame(answer.to_string().as_bytes()));
        return;
    }
    let json_body = if path == "/server/files/upload" {
        // Keep only the form fields, not the file.
        let text = String::from_utf8_lossy(&body);
        let field = |name: &str| {
            text.split(&format!("name=\"{name}\"\r\n\r\n"))
                .nth(1)
                .and_then(|r| r.split("\r\n").next())
                .map(str::to_owned)
        };
        json!({"path": field("path"), "print": field("print"), "root": field("root")})
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    calls.lock().unwrap().push(Call {
        method: method.clone(),
        path: path.clone(),
        body: json_body.clone(),
    });
    match (method.as_str(), path.split('?').next().unwrap_or("")) {
        ("GET", "/machine/system_info") => match b.firmware {
            Some(f) => reply(
                &mut s,
                200,
                json!({"result":{"system_info":{"product_info":{"machine_type":"Snapmaker U1","firmware_version":f,"serial_number":"mock"}}}}),
            ),
            None => reply(
                &mut s,
                200,
                json!({"result":{"system_info":{"cpu_info":{}}}}),
            ),
        },
        ("GET", "/printer/info") => reply(
            &mut s,
            200,
            json!({"result":{"state":"ready","hostname":"u1","software_version":"v0.12.0-418-gmock"}}),
        ),
        ("POST", "/server/files/upload") => reply(
            &mut s,
            201,
            json!({"result":{"item":{"path":"cube.gcode","root":"gcodes"},"print_started":false,"action":"create_file"}}),
        ),
        ("POST", "/printer/gcode/script") => match b.macro_error {
            Some(e) => reply(&mut s, 400, json!({"error":{"code":400,"message":e}})),
            None => reply(&mut s, 200, json!({"result":"ok"})),
        },
        ("POST", "/printer/print/start") => reply(&mut s, 200, json!({"result":"ok"})),
        _ => reply(
            &mut s,
            404,
            json!({"error":{"code":404,"message":"Not Found"}}),
        ),
    }
}

fn gcode() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mica-u1-map-{}.gcode", std::process::id()));
    std::fs::write(&path, b"; mica\nT0\nG1 X1\nT1\nG1 X2\n").unwrap();
    path
}

/// T0 → slot 3, T1 → slot 1 (zero based 2 and 0), flow calibration on T0 only.
fn review_case() -> U1StartOptions {
    U1StartOptions {
        map: vec![(0, 2), (1, 0)],
        bed_leveling: false,
        flow_calibration: true,
        flow_tools: vec![0],
        timelapse: false,
    }
}

fn run(b: Behaviour) -> Option<(Result<U1Started>, Vec<Call>)> {
    let Some(mock) = Mock::start(b) else {
        eprintln!("skipped: cannot listen on loopback here");
        return None;
    };
    let host = mock.host();
    let result = host.start_with_mapping(&gcode(), "cube.gcode", &review_case(), &mut |_| {});
    let deadline = Instant::now() + Duration::from_millis(50);
    while Instant::now() < deadline {
        thread::yield_now();
    }
    Some((result, mock.finish()))
}

fn upload_call() -> Call {
    Call {
        method: "POST".into(),
        path: "/server/files/upload".into(),
        body: json!({"path":"","print":null,"root":"gcodes"}),
    }
}
fn firmware_call() -> Call {
    Call {
        method: "GET".into(),
        path: "/machine/system_info".into(),
        body: Value::Null,
    }
}
const SCRIPT: &str = "SET_PRINT_EXTRUDER_MAP CONFIG_EXTRUDER=0 MAP_EXTRUDER=2\nSET_PRINT_EXTRUDER_MAP CONFIG_EXTRUDER=1 MAP_EXTRUDER=0\nSET_PRINT_USED_EXTRUDERS EXTRUDERS=2,0\nSET_PRINT_PREFERENCES BED_LEVEL=0 FLOW_CALIBRATE=1 TIME_LAPSE_CAMERA=0 FLOW_CALIBRATE_EXTRUDERS=2";
fn macro_calls() -> Vec<Call> {
    vec![
        Call {
            method: "POST".into(),
            path: "/printer/gcode/script".into(),
            body: json!({ "script": SCRIPT }),
        },
        Call {
            method: "POST".into(),
            path: "/printer/print/start".into(),
            body: json!({"filename":"cube.gcode"}),
        },
    ]
}

#[test]
fn firmware_1_5_starts_with_start_local_print_over_the_websocket() {
    let Some((result, calls)) = run(Behaviour {
        firmware: Some("1.5.2.13"),
        ws_reply: Some(
            json!({"result":{"state":"success","message":"Print started","filename":"cube.gcode"}}),
        ),
        macro_error: None,
    }) else {
        return;
    };
    let started = result.unwrap();
    assert_eq!(started.route, U1Route::StartLocalPrint);
    assert_eq!(started.firmware.as_deref(), Some("1.5.2.13"));
    assert!(started.job.started);
    assert_eq!(calls.len(), 3, "{calls:#?}");
    assert_eq!(calls[0], firmware_call());
    assert_eq!(
        calls[1],
        upload_call(),
        "uploaded with print=false (no field)"
    );
    let mut ws = calls[2].body.clone();
    assert!(ws["id"].is_u64());
    ws["id"] = json!(0);
    assert_eq!(calls[2].path, "/websocket");
    assert_eq!(
        ws,
        json!({"jsonrpc":"2.0","method":"server.files.start_local_print","id":0,"params":{
            "type":"gcode","path":"cube.gcode","print_plate":1,
            "options":{"bed_level":0,"flow_calibrate":1,"flow_calibrate_extruders":[2],
                       "map_table":[[0,2],[1,0]],"time_lapse_camera":0}}})
    );
    assert!(
        started
            .summary()
            .contains("start_local_print (firmware 1.5.2.13)")
    );
}

#[test]
fn firmware_below_1_3_uses_the_macros_then_print_start() {
    let Some((result, calls)) = run(Behaviour {
        firmware: Some("1.2.0"),
        ws_reply: None,
        macro_error: None,
    }) else {
        return;
    };
    let started = result.unwrap();
    assert_eq!(started.route, U1Route::Macros);
    let mut want = vec![firmware_call(), upload_call()];
    want.extend(macro_calls());
    assert_eq!(calls, want);
}

#[test]
fn unknown_firmware_uses_the_macros_like_snapmakers_app() {
    // No product_info and Klipper's own version in /printer/info: not a firmware version.
    let Some((result, calls)) = run(Behaviour {
        firmware: None,
        ws_reply: Some(json!({"result":{"state":"success"}})),
        macro_error: None,
    }) else {
        return;
    };
    let started = result.unwrap();
    assert_eq!(started.route, U1Route::Macros);
    assert_eq!(started.firmware, None);
    assert_eq!(calls[1].path, "/printer/info");
    assert!(!calls.iter().any(|c| c.path == "/websocket"));
    assert_eq!(calls[calls.len() - 2..], macro_calls()[..]);
}

#[test]
fn start_local_print_failure_and_missing_method_fall_back_to_the_macros() {
    for reply in [
        json!({"result":{"state":"failure"}}),
        json!({"error":{"code":-32601,"message":"Method not found for transport WEBSOCKET"}}),
    ] {
        let Some((result, calls)) = run(Behaviour {
            firmware: Some("1.5.2.13"),
            ws_reply: Some(reply),
            macro_error: None,
        }) else {
            return;
        };
        let started = result.unwrap();
        assert!(
            matches!(started.route, U1Route::MacrosFallback(_)),
            "{started:?}"
        );
        assert_eq!(calls[2].path, "/websocket");
        assert_eq!(calls[3..], macro_calls()[..]);
    }
    // No WebSocket at all: the same fallback, and the reason is kept for the user.
    let Some((result, calls)) = run(Behaviour {
        firmware: Some("1.5.2.13"),
        ws_reply: None,
        macro_error: None,
    }) else {
        return;
    };
    let started = result.unwrap();
    match &started.route {
        U1Route::MacrosFallback(why) => assert!(why.contains("HTTP 404"), "{why}"),
        r => panic!("{r:?}"),
    }
    assert!(started.summary().contains("slot macros"));
    assert_eq!(calls[calls.len() - 2..], macro_calls()[..]);
}

#[test]
fn refusals_are_reported_and_never_retried() {
    // The printer refuses start_local_print: no second start through the macros.
    let Some((result, calls)) = run(Behaviour {
        firmware: Some("1.5.2.13"),
        ws_reply: Some(
            json!({"result":{"state":"error","message":"Printer is busy, cannot start print"}}),
        ),
        macro_error: None,
    }) else {
        return;
    };
    let e = result.unwrap_err();
    assert_eq!(
        e.message,
        "cube.gcode was uploaded, but the print did not start: Printer is busy, cannot start print"
    );
    assert!(!calls.iter().any(|c| c.path.starts_with("/printer/")));
    // Klipper refuses a macro: its reason reaches the message, nothing is started.
    let Some((result, calls)) = run(Behaviour {
        firmware: Some("1.2.0"),
        ws_reply: None,
        macro_error: Some("[print_task_config] not allowed to set extruder map during printing!"),
    }) else {
        return;
    };
    let e = result.unwrap_err();
    assert_eq!(e.http_status, Some(400));
    assert!(
        e.message
            .contains("was uploaded, but the print did not start")
            && e.message
                .contains("not allowed to set extruder map during printing"),
        "{}",
        e.message
    );
    assert!(!calls.iter().any(|c| c.path == "/printer/print/start"));
}

#[test]
fn bad_maps_fail_before_anything_is_sent() {
    let Some(mock) = Mock::start(Behaviour {
        firmware: Some("1.5.2.13"),
        ws_reply: None,
        macro_error: None,
    }) else {
        return;
    };
    let o = U1StartOptions {
        map: vec![(0, 5)],
        ..review_case()
    };
    assert!(
        mock.host()
            .start_with_mapping(&gcode(), "cube.gcode", &o, &mut |_| {})
            .is_err()
    );
    assert!(mock.finish().is_empty());
}
