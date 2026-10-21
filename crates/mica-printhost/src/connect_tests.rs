// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;

fn version(put: bool) -> Reply {
    Reply::json(
        json!({"api":"0.1", "text":"PrusaLink Connect", "capabilities":{"upload-by-put":put}}),
    )
}
fn acceptance(code: u16, text: &str) -> Reply {
    Reply {
        body: text.into(),
        ..Reply::code(code)
    }
}
fn host(s: &Server) -> PrusaConnect {
    PrusaConnect::new(s.config(PrinterKind::PrusaConnect), Timeouts::default()).unwrap()
}

#[test]
fn connect_exact_multipart_for_all_actions_and_202_warning() {
    let file = Temp::new(b"G28\nG1 X1\n");
    for (action, field) in [
        (UploadAction::UploadOnly, None),
        (UploadAction::Print, Some("to_print")),
        (UploadAction::Queue, Some("to_queue")),
    ] {
        for status in [200, 202] {
            let server = Server::new(vec![
                version(false),
                acceptance(status, "Printer is busy: private-key\n"),
            ]);
            let h = host(&server);
            assert!(!h.capabilities().status);
            assert!(h.status().is_err()); // no request
            assert!(h.pause().is_err());
            assert_eq!(
                h.preflight(&JobRequirements {
                    tools: vec![],
                    format: FileFormat::Gcode,
                    bed_type: None,
                    needs_chamber_sensor: false
                })
                .len(),
                1
            );
            let mut progress = vec![];
            let out = h
                .upload_action_with_progress(&file.0, "folder/test name.bgcode", action, &mut |p| {
                    progress.push((p.sent, p.total))
                })
                .unwrap();
            assert!(!out.job.started);
            assert_eq!(out.warning, status == 202);
            assert_eq!(out.message.as_deref(), Some("Printer is busy: [redacted]"));
            assert_eq!(progress.first(), Some(&(0, 10)));
            assert_eq!(progress.last(), Some(&(10, 10)));
            let requests = server.finish();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].method, "GET");
            assert_eq!(requests[0].path, "/api/version");
            assert!(requests[0].body.is_empty());
            for r in &requests {
                assert_eq!(r.headers["x-api-key"], "private-key");
                assert!(!r.headers.contains_key("authorization"));
            }
            let r = &requests[1];
            assert_eq!(r.method, "POST");
            assert_eq!(r.path, "/api/files/local");
            assert_eq!(r.headers["accept-language"], "en");
            let boundary = r.headers["content-type"]
                .strip_prefix("multipart/form-data; boundary=")
                .unwrap();
            let field = field.map(|f| format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{f}\"\r\n\r\nTrue\r\n")).unwrap_or_default();
            let expected = format!(
                "{field}--{boundary}\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\nfolder\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test name.bgcode\"\r\nContent-Type: application/octet-stream\r\n\r\nG28\nG1 X1\n\r\n--{boundary}--\r\n"
            );
            assert_eq!(r.body, expected.as_bytes());
            assert_eq!(r.headers["content-length"], expected.len().to_string());
        }
    }
}

#[test]
fn connect_root_path_and_empty_accepted_warning() {
    let file = Temp::new(b"G28\n");
    let s = Server::new(vec![version(false), Reply::code(202)]);
    let out = host(&s)
        .upload_action_with_progress(&file.0, "test.gcode", UploadAction::Print, &mut |_| {})
        .unwrap();
    assert!(out.warning);
    assert!(out.message.unwrap().contains("not confirmed"));
    let r = s.finish();
    assert!(String::from_utf8_lossy(&r[1].body).contains("name=\"path\"\r\n\r\n\r\n"));
}

#[test]
fn connect_put_negotiation_and_queue_refusal() {
    let file = Temp::new(b"G28\n");
    for action in [UploadAction::UploadOnly, UploadAction::Print] {
        let s = Server::new(vec![version(true), Reply::code(201)]);
        let out = host(&s)
            .upload_action_with_progress(&file.0, "folder/test name.gcode", action, &mut |_| {})
            .unwrap();
        assert!(!out.job.started);
        let requests = s.finish();
        let r = &requests[1];
        assert_eq!(r.method, "PUT");
        assert_eq!(r.path, "/api/v1/files/local/folder/test%20name.gcode");
        assert_eq!(r.headers["content-type"], "text/x.gcode");
        assert_eq!(r.headers["overwrite"], "?1");
        assert_eq!(
            r.headers.get("print-after-upload").map(String::as_str),
            if action == UploadAction::Print {
                Some("?1")
            } else {
                None
            }
        );
        assert_eq!(r.headers["content-length"], "4");
        assert_eq!(r.body, b"G28\n");
    }
    let s = Server::new(vec![version(true)]);
    let e = host(&s)
        .upload_action_with_progress(&file.0, "test.gcode", UploadAction::Queue, &mut |_| {})
        .unwrap_err();
    assert!(e.message.contains("no file was sent"));
    assert_eq!(s.finish().len(), 1);
}

#[test]
fn connect_errors_and_network_failure_never_replay() {
    let file = Temp::new(b"G28\n");
    for code in [401, 409] {
        let s = Server::new(vec![version(false), acceptance(code, "private-key")]);
        let e = host(&s).upload(&file.0, "test.gcode", true).unwrap_err();
        assert_eq!(e.http_status, Some(code));
        assert_eq!(
            e.kind,
            if code == 401 {
                ErrorKind::Unauthorised
            } else {
                ErrorKind::Busy
            }
        );
        assert!(!e.to_string().contains("private-key"));
        assert_eq!(s.finish().len(), 2);
    }
    let s = Server::new(vec![Reply::code(401)]);
    assert_eq!(
        host(&s)
            .upload(&file.0, "test.gcode", false)
            .unwrap_err()
            .kind,
        ErrorKind::Unauthorised
    );
    assert_eq!(s.finish().len(), 1);
    let s = Server::new(vec![
        version(false),
        Reply {
            delay: Duration::from_millis(150),
            ..Reply::code(201)
        },
    ]);
    let h = PrusaConnect::new(
        s.config(PrinterKind::PrusaConnect),
        Timeouts {
            upload: Duration::from_millis(30),
            ..Timeouts::default()
        },
    )
    .unwrap();
    assert_eq!(
        h.upload(&file.0, "test.gcode", true).unwrap_err().kind,
        ErrorKind::Unreachable
    );
    assert_eq!(s.finish().len(), 2);
}

#[test]
fn connect_refuses_mismatched_hosts_invalid_names_and_secrets() {
    let file = Temp::new(b"G28\n");
    for v in [
        json!({"api":"0.1","text":"Moonraker"}),
        json!({"text":"PrusaLink"}),
    ] {
        let s = Server::new(vec![Reply::json(v)]);
        assert_eq!(
            host(&s)
                .upload(&file.0, "test.gcode", false)
                .unwrap_err()
                .kind,
            ErrorKind::Protocol
        );
        assert_eq!(s.finish().len(), 1);
    }
    let s = Server::new(vec![]);
    let h = host(&s);
    for name in ["../test.gcode", "bad\r\n.gcode", "bad\".gcode", "bad.stl"] {
        assert!(h.upload(&file.0, name, false).is_err());
    }
    let mut config = s.config(PrinterKind::PrusaConnect);
    config.address = "https://example.invalid".into();
    assert!(PrusaConnect::new(config, Timeouts::default()).is_err());
    let mut config = s.config(PrinterKind::PrusaConnect);
    config.credentials = Credentials::None;
    assert!(PrusaConnect::new(config, Timeouts::default()).is_err());
    assert!(s.finish().is_empty());
}
