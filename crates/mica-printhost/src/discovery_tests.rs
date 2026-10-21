// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! LAN discovery (`discovery.rs`): packets, assembly, a fake responder on loopback, probes
//! against the scripted mock HTTP server. No real network traffic.

use super::*;
use crate::discovery::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// An Avahi-style answer as it comes off the wire, with name compression: Moonraker's
/// zeroconf announcement of a U1 ("U1 Workshop"), its SRV / TXT and the host's A record.
fn moonraker_fixture() -> Vec<u8> {
    let mut p: Vec<u8> = vec![
        0x12, 0x34, // id
        0x84, 0x00, // response, authoritative
        0, 0, 0, 1, 0, 0, 0, 3, // 0 questions, 1 answer, 0 authority, 3 additional
    ];
    // offset 12: _moonraker._tcp.local
    let service_at = p.len() as u8;
    for l in ["_moonraker", "_tcp", "local"] {
        p.push(l.len() as u8);
        p.extend_from_slice(l.as_bytes());
    }
    p.push(0);
    let local_at = service_at + 1 + 10 + 1 + 4; // the "local" label inside that name
    p.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0x11, 0x94]); // PTR IN ttl 4500
    let instance = b"U1 Workshop";
    p.extend_from_slice(&((instance.len() + 1 + 2) as u16).to_be_bytes());
    let instance_at = p.len() as u8;
    p.push(instance.len() as u8);
    p.extend_from_slice(instance);
    p.extend_from_slice(&[0xc0, service_at]);
    // SRV of the instance (cache-flush class): priority 0, weight 0, port 7125,
    // target snapmaker-u1.local
    p.extend_from_slice(&[0xc0, instance_at, 0, 33, 0x80, 1, 0, 0, 0, 120]);
    let host = b"snapmaker-u1";
    p.extend_from_slice(&((6 + 1 + host.len() + 2) as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    p.extend_from_slice(&7125u16.to_be_bytes());
    let host_at = p.len() as u8;
    p.push(host.len() as u8);
    p.extend_from_slice(host);
    p.extend_from_slice(&[0xc0, local_at]);
    // TXT
    p.extend_from_slice(&[0xc0, instance_at, 0, 16, 0x80, 1, 0, 0, 0, 120]);
    let txt: &[&[u8]] = &[b"uuid=0b2c", b"version=v0.9.3-24", b"route_prefix="];
    let len: usize = txt.iter().map(|t| t.len() + 1).sum();
    p.extend_from_slice(&(len as u16).to_be_bytes());
    for t in txt {
        p.push(t.len() as u8);
        p.extend_from_slice(t);
    }
    // A
    p.extend_from_slice(&[
        0xc0, host_at, 0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 192, 0, 2, 57,
    ]);
    p
}

#[test]
fn queries_and_compressed_answers_round_trip() {
    let q = query(7, &[(Name::parse("_moonraker._tcp.local"), 12)]);
    assert_eq!(&q[..6], &[0, 7, 0, 0, 0, 1]);
    let (id, qs) = questions(&q).unwrap();
    assert_eq!(id, 7);
    assert_eq!(qs, vec![(Name::parse("_moonraker._tcp.local"), 12)]);
    let m = parse(&moonraker_fixture()).unwrap();
    assert!(m.response);
    assert_eq!(m.id, 0x1234);
    assert_eq!(m.records.len(), 4);
    let mut b = Browse::default();
    b.add(&m);
    let s = b.services(&SERVICES);
    assert_eq!(s.len(), 1);
    let s = &s[0];
    assert_eq!(s.label(), "U1 Workshop");
    assert_eq!(s.service, "_moonraker._tcp.local");
    assert_eq!(s.port, Some(7125));
    assert_eq!(s.host, Some(Name::parse("snapmaker-u1.local")));
    assert_eq!(s.addresses, vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 57))]);
    assert_eq!(s.txt["version"], "v0.9.3-24");
    assert_eq!(s.txt["route_prefix"], "");
    let c = candidates(&b.services(&SERVICES));
    assert_eq!(
        c,
        vec![Candidate {
            name: "U1 Workshop".into(),
            address: "192.0.2.57:7125".into(),
            hint: Some(PrinterKind::Moonraker),
            txt: s.txt.clone(),
            host: Some("snapmaker-u1".into()),
        }]
    );
    // Our own encoder (the fake responders') parses back to the same records.
    let records = FakeResponder::instance(
        "_octoprint._tcp.local",
        "Prusa XL",
        "prusa-xl",
        80,
        &["model=XL", "version=6.2.3"],
    );
    assert_eq!(parse(&response(9, &records)).unwrap().records, records);
}

#[test]
fn malformed_packets_are_refused_not_panicked_on() {
    let good = moonraker_fixture();
    for n in 0..good.len() {
        assert!(parse(&good[..n]).is_err(), "truncated at {n}");
    }
    // A name pointing at itself.
    let mut looped = vec![0, 1, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    looped.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 1, 0, 2, 0xc0, 12]);
    assert!(parse(&looped).is_err());
    assert!(parse(&[1, 2, 3]).is_err());
}

#[test]
fn only_printer_looking_http_services_are_probed_and_duplicates_merge() {
    let mut b = Browse::default();
    let mut records = vec![];
    records.extend(FakeResponder::instance(
        "_http._tcp.local",
        "Router admin",
        "router",
        80,
        &[],
    ));
    records.extend(FakeResponder::instance(
        "_http._tcp.local",
        "PrusaLink XL",
        "prusa-xl",
        8080,
        &[],
    ));
    records.extend(FakeResponder::instance(
        "_octoprint._tcp.local",
        "Prusa XL",
        "prusa-xl",
        8080,
        &["model=XL"],
    ));
    b.add(&parse(&response(1, &records)).unwrap());
    let c = candidates(&b.services(&SERVICES));
    // The router is skipped; the XL's two announcements become one PrusaLink candidate.
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].hint, Some(PrinterKind::PrusaLink));
    assert_eq!(c[0].name, "Prusa XL");
    assert_eq!(c[0].address, "127.0.0.1:8080");
    assert_eq!(c[0].txt["model"], "XL");
}

#[test]
fn models_are_guessed_from_host_names() {
    assert_eq!(guess_model("prusa-xl"), Some("Original Prusa XL"));
    assert_eq!(guess_model("PrusaCoreOne"), Some("Prusa CORE One"));
    assert_eq!(guess_model("prusa-coreonel"), Some("Prusa CORE One L"));
    assert_eq!(guess_model("MK4S"), Some("Original Prusa MK4S"));
    assert_eq!(guess_model("snapmaker-u1"), Some("Snapmaker U1"));
    assert_eq!(guess_model("XL"), Some("Original Prusa XL"));
    assert_eq!(guess_model("nas-xlarge-box"), None);
    assert_eq!(guess_model("router"), None);
}

/// Answers each question with exactly the matching records (no additional section), so the
/// browser has to follow up for SRV, TXT and A.
struct SparseResponder(FakeResponder);
impl SparseResponder {
    fn serve(&self, socket: &UdpSocket, stop: impl Fn() -> bool) {
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut buf = vec![0u8; 2048];
        while !stop() {
            let Ok((n, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let (id, qs) = questions(&buf[..n]).unwrap();
            let mut out = vec![];
            for (name, kind) in &qs {
                for r in &self.0.records {
                    let k = match r.data {
                        Data::Ptr(_) => 12,
                        Data::Srv { .. } => 33,
                        Data::Txt(_) => 16,
                        Data::A(_) => 1,
                        _ => 0,
                    };
                    if r.name.key() == name.key() && k == *kind && !out.contains(r) {
                        out.push(r.clone());
                    }
                }
            }
            if !out.is_empty() {
                let _ = socket.send_to(&response(id, &out), from);
            }
        }
    }
}

fn udp() -> Option<UdpSocket> {
    match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => None,
        Err(e) => panic!("{e}"),
    }
}

fn instances() -> Vec<Record> {
    let mut records = FakeResponder::instance(
        "_moonraker._tcp.local",
        "U1",
        "snapmaker-u1",
        7125,
        &["version=v0.9.3"],
    );
    records.extend(FakeResponder::instance(
        "_octoprint._tcp.local",
        "Workshop CORE One",
        "prusa-coreone",
        18083,
        &[],
    ));
    records
}

#[test]
fn browsing_a_sparse_fake_responder_follows_up_for_missing_records() {
    let Some(socket) = udp() else { return };
    let at: SocketAddr = socket.local_addr().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let server = thread::spawn(move || {
        SparseResponder(FakeResponder {
            records: instances(),
        })
        .serve(&socket, || s2.load(Ordering::Relaxed));
    });
    let found = browse(at, &SERVICES, Duration::from_millis(300)).unwrap();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let mut names: Vec<(&str, Option<u16>, usize, Option<&str>)> = found
        .iter()
        .map(|s| {
            (
                s.label(),
                s.port,
                s.addresses.len(),
                s.txt.get("version").map(String::as_str),
            )
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            ("U1", Some(7125), 1, Some("v0.9.3")),
            ("Workshop CORE One", Some(18083), 1, None)
        ]
    );
}

#[test]
fn browsing_a_full_fake_responder_gets_everything_in_one_answer() {
    let Some(socket) = udp() else { return };
    let at: SocketAddr = socket.local_addr().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let server = thread::spawn(move || {
        FakeResponder {
            records: instances(),
        }
        .serve(&socket, || s2.load(Ordering::Relaxed));
    });
    let found = browse(at, &SERVICES, Duration::from_millis(300)).unwrap();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(Service::complete));
    let c = candidates(&found);
    assert_eq!(c.len(), 2);
    assert!(c.iter().all(|c| c.address.starts_with("127.0.0.1:")));
}

fn candidate(server: &Server, hint: Option<PrinterKind>, name: &str) -> Candidate {
    Candidate {
        name: name.into(),
        address: server.address.trim_start_matches("http://").into(),
        hint,
        txt: BTreeMap::new(),
        host: Some("prusa-xl".into()),
    }
}

#[test]
fn a_prusalink_without_a_key_reports_model_tools_nozzle_and_state() {
    let server = Server::new(vec![
        Reply::json(
            json!({"api":"2.0.0","server":"2.1.2","text":"PrusaLink","hostname":"prusa-xl","nozzle_diameter":0.4}),
        ),
        Reply::json(
            json!({"name":"XL 5T","printer_model":"Original Prusa XL 5T","firmware":"6.2.3+9089","nozzle_diameter":0.4,"tools":{"1":{},"2":{},"3":{},"4":{},"5":{}}}),
        ),
        Reply::json(prusa_idle()),
    ]);
    let c = candidate(&server, Some(PrinterKind::PrusaLink), "XL 5T");
    let d = identify(&c, Timeouts::default()).unwrap();
    let r = server.finish();
    assert_eq!(
        r.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        ["/api/version", "/api/v1/info", "/api/v1/status"]
    );
    assert!(r.iter().all(|r| r.method == "GET"));
    assert!(r.iter().all(|r| !r.headers.contains_key("x-api-key")));
    assert_eq!(d.kind, PrinterKind::PrusaLink);
    assert_eq!(d.model.as_deref(), Some("Original Prusa XL 5T"));
    assert_eq!(d.tools, Some(5));
    assert_eq!(d.nozzle, Some(0.4));
    assert_eq!(d.firmware.as_deref(), Some("6.2.3+9089"));
    assert_eq!(d.state, Some(PrinterState::Idle));
    assert!(!d.needs_key);
}

#[test]
fn a_prusalink_asking_for_its_key_is_listed_with_a_guessed_model() {
    let server = Server::new(vec![Reply::code(401)]);
    let mut c = candidate(&server, Some(PrinterKind::PrusaLink), "Prusa XL");
    c.txt.insert("model".into(), "XL".into());
    let d = identify(&c, Timeouts::default()).unwrap();
    server.finish();
    assert!(d.needs_key);
    assert_eq!(d.model.as_deref(), Some("Original Prusa XL"));
    // The same 401 from an unannounced web server is not a printer.
    let server = Server::new(vec![Reply::code(401), Reply::code(401)]);
    let mut c = candidate(&server, None, "Camera");
    c.host = Some("ipcam".into());
    assert_eq!(identify(&c, Timeouts::default()), None);
    server.finish();
}

#[test]
fn a_moonraker_with_snapmaker_objects_is_a_u1() {
    let server = Server::new(vec![
        Reply::json(json!({"result":{"klippy_state":"ready","moonraker_version":"v0.9.3"}})),
        Reply::json(json!({"result":{"hostname":"snapmaker-u1","software_version":"v0.12.0-418"}})),
        objects(4, &["print_task_config", "filament_detect"]),
        moon_status("standby", 4),
        moon_status("standby", 4),
        Reply::json(
            json!({"result":{"status":{"configfile":{"settings":{"extruder":{"nozzle_diameter":0.4}}}}}}),
        ),
    ]);
    let c = candidate(&server, Some(PrinterKind::Moonraker), "U1");
    let d = identify(&c, Timeouts::default()).unwrap();
    let r = server.finish();
    assert!(r.iter().all(|r| r.method == "GET"), "{r:?}");
    assert_eq!(d.kind, PrinterKind::SnapmakerU1);
    assert_eq!(d.model.as_deref(), Some("Snapmaker U1"));
    assert_eq!(d.tools, Some(4));
    assert_eq!(d.nozzle, Some(0.4));
    assert_eq!(d.state, Some(PrinterState::Idle));
    assert_eq!(d.firmware.as_deref(), Some("v0.12.0-418"));
}

/// The real U1's announcement, as seen on the LAN (serial replaced): `_snapmaker._tcp`,
/// instance "U1", the MQTT port 1884, and its address in the TXT record.
fn u1_announcement() -> Vec<Record> {
    FakeResponder::instance(
        "_snapmaker._tcp.local",
        "U1",
        "U1-0000000000000000000Z",
        1884,
        &[
            "version=1.5.2",
            "machine_type=Snapmaker U1",
            "device_name=U1",
            "sn=TEST-SERIAL-0001",
            "link_mode=lan",
            "userid=",
            "ip=192.0.2.25",
            "region=int",
        ],
    )
}

#[test]
fn the_u1_is_found_by_its_snapmaker_announcement_on_moonrakers_port() {
    let mut b = Browse::default();
    b.add(&parse(&response(1, &u1_announcement())).unwrap());
    let c = candidates(&b.services(&SERVICES));
    assert_eq!(c.len(), 1, "{c:?}");
    let c = &c[0];
    assert_eq!(c.name, "U1");
    assert_eq!(c.hint, Some(PrinterKind::SnapmakerU1));
    // The TXT address (not the resolved 127.0.0.1 of the fixture), Moonraker's port (not MQTT's).
    assert_eq!(c.address, "192.0.2.25:7125");
    assert_eq!(c.txt["machine_type"], "Snapmaker U1");
    // The serial number and the account id never leave the browser.
    assert!(!c.txt.contains_key("sn") && !c.txt.contains_key("userid"));
    assert!(!format!("{c:?}").contains("TEST-SERIAL"));
    // Without a TXT address the resolved host is used, still on 7125.
    let mut records = u1_announcement();
    for r in &mut records {
        if let Data::Txt(t) = &mut r.data {
            t.retain(|e| !e.starts_with("ip="));
        }
    }
    let mut b = Browse::default();
    b.add(&parse(&response(1, &records)).unwrap());
    assert_eq!(
        candidates(&b.services(&SERVICES))[0].address,
        "127.0.0.1:7125"
    );
}

#[test]
fn a_u1_answering_on_7125_reports_its_own_firmware_and_model() {
    let server = Server::new(vec![
        Reply::json(json!({"result":{"klippy_state":"ready","moonraker_version":"v0.9.3"}})),
        Reply::json(json!({"result":{"hostname":"U1","software_version":"v0.12.0-418"}})),
        objects(4, &["print_task_config", "filament_detect"]),
        moon_status("standby", 4),
        moon_status("standby", 4),
        Reply::json(json!({"result":{"status":{}}})),
    ]);
    let mut c = candidate(&server, Some(PrinterKind::SnapmakerU1), "U1");
    c.txt.insert("machine_type".into(), "Snapmaker U1".into());
    c.txt.insert("version".into(), "1.5.2".into());
    let d = identify(&c, Timeouts::default()).unwrap();
    server.finish();
    assert_eq!(d.kind, PrinterKind::SnapmakerU1);
    assert_eq!(d.model.as_deref(), Some("Snapmaker U1"));
    assert_eq!(d.firmware.as_deref(), Some("1.5.2"));
    assert_eq!(d.tools, Some(4));
    // Announced but not answering: listed, offline.
    let server = Server::new(vec![Reply::code(502)]);
    let c = candidate(&server, Some(PrinterKind::SnapmakerU1), "U1");
    let d = identify(&c, Timeouts::default()).unwrap();
    server.finish();
    assert_eq!(d.state, Some(PrinterState::Offline));
    assert_eq!(d.kind, PrinterKind::SnapmakerU1);
}

/// Smart plugs announce `_http._tcp` with printer-like names. Their names make them
/// worth one look, but only a PrusaLink / Moonraker API answer makes them printers.
#[test]
fn smart_plugs_named_after_printers_are_not_printers() {
    let mut records = vec![];
    for name in ["PrusaC1-Plug", "PrusaXL-Plug", "U1-Plug"] {
        records.extend(FakeResponder::instance(
            "_http._tcp.local",
            name,
            &name.to_ascii_lowercase(),
            80,
            &[],
        ));
    }
    let mut b = Browse::default();
    b.add(&parse(&response(1, &records)).unwrap());
    let c = candidates(&b.services(&SERVICES));
    assert!(c.iter().all(|c| c.hint.is_none()));
    // A plug's web page: 401 (a login) or 404 to both APIs, or HTML: never a printer.
    for replies in [
        vec![Reply::code(401), Reply::code(401)],
        vec![Reply::code(404), Reply::code(404)],
        vec![
            Reply::json(json!({"name":"PrusaXL-Plug","type":"plug"})),
            Reply::code(404),
        ],
    ] {
        for name in ["PrusaC1-Plug", "PrusaXL-Plug", "U1-Plug"] {
            let server = Server::new(
                replies
                    .iter()
                    .map(|r| Reply {
                        code: r.code,
                        body: r.body.clone(),
                        headers: vec![],
                        delay: Duration::ZERO,
                    })
                    .collect(),
            );
            let mut c = candidate(&server, None, name);
            c.host = Some(name.to_ascii_lowercase());
            assert_eq!(identify(&c, Timeouts::default()), None, "{name}");
            server.finish();
        }
    }
}

/// The U1's webcam list as it answers: an enabled WebRTC camera ("case") and a disabled one
/// ("gui"). The enabled one is kept, its relative URLs resolve against the printer's web
/// origin, and a snapshot is what Mica can show.
#[test]
fn the_u1_webcam_list_keeps_the_enabled_camera_with_an_absolute_snapshot() {
    let list = json!({"webcams":[
        {"name":"case","service":"webrtc-camerastreamer","enabled":true,"stream_url":"/webcam/webrtc","snapshot_url":"/webcam/snapshot.jpg"},
        {"name":"gui","service":"webrtc-camerastreamer","enabled":false,"stream_url":"/webcam2/webrtc","snapshot_url":"/webcam2/snapshot.jpg"}
    ]});
    let cams = crate::camera::parse_moonraker("http://192.0.2.25:7125", &list);
    assert_eq!(cams.len(), 1);
    assert_eq!(cams[0].name, "case");
    assert_eq!(cams[0].service, CameraService::WebRtc);
    assert_eq!(
        cams[0].snapshot_url.as_deref(),
        Some("http://192.0.2.25/webcam/snapshot.jpg")
    );
    assert_eq!(
        cams[0].stream_url.as_deref(),
        Some("http://192.0.2.25/webcam/webrtc")
    );
}
