// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica MCP live mode: the link between `mica mcp` and an open Mica window, so the AI works
//! on the window's project and the user watches every edit appear (and can undo it there).
//!
//! **Transport**: a Unix domain socket in Mica's settings folder (`mcp.sock`, mode 0600),
//! created by the window when the Mica MCP feature is on and removed when it closes. Nothing
//! listens on the network. On Windows live mode is not supported yet: `mica mcp` stays
//! headless there.
//!
//! **Protocol** (version [`PROTOCOL`]): one JSON object per line each way. The client sends
//! `{"v":1,"id":7,"tool":"paint_at","args":{…}}`; the window answers
//! `{"v":1,"id":7,"ok":true,"content":[{"type":"text","text":"…"}]}` or
//! `{"v":1,"id":7,"ok":false,"error":"…"}`. `hello` is answered by the connection itself
//! (protocol version, app version, process id); every other tool runs on the window's UI
//! thread between frames. A request with another protocol version is answered with an error.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The protocol version both sides speak.
pub const PROTOCOL: u32 = 1;
/// The socket's file name in Mica's settings folder.
pub const SOCKET_NAME: &str = "mcp.sock";

/// Mica's settings folder (`MICA_CONFIG_DIR` when set).
pub fn config_dir() -> PathBuf {
    mica_core::user_data::config_dir()
}

/// Where the window listens: `<settings folder>/mcp.sock`.
pub fn socket_path() -> PathBuf {
    config_dir().join(SOCKET_NAME)
}

/// One block of a tool's result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    /// `data` is base64.
    Image {
        data: String,
        mime: String,
    },
}

impl Content {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Image { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: u64,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Content>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn new(id: u64, result: Result<Vec<Content>, String>) -> Self {
        match result {
            Ok(content) => Self {
                v: PROTOCOL,
                id,
                ok: true,
                content,
                error: None,
            },
            Err(e) => Self {
                v: PROTOCOL,
                id,
                ok: false,
                content: vec![],
                error: Some(e),
            },
        }
    }

    pub fn result(self) -> Result<Vec<Content>, String> {
        if self.ok {
            Ok(self.content)
        } else {
            Err(self
                .error
                .unwrap_or_else(|| "the Mica window reported an error without a message".into()))
        }
    }
}

/// One line of the protocol (no newline inside: JSON escapes them).
pub fn encode<T: Serialize>(message: &T) -> String {
    let mut line = serde_json::to_string(message).unwrap_or_else(|e| {
        format!("{{\"v\":{PROTOCOL},\"id\":0,\"ok\":false,\"error\":\"encoding: {e}\"}}")
    });
    line.push('\n');
    line
}

/// What the window answers to `hello`.
pub fn hello(pid: u32) -> Value {
    serde_json::json!({
        "protocol": PROTOCOL,
        "app": "Mica Slicer",
        "version": mica_core::VERSION,
        "pid": pid,
    })
}

/// Why no window could be reached.
#[derive(Debug)]
pub enum ConnectError {
    /// No socket, or a stale one nobody listens on: no Mica window serves Mica MCP.
    NoWindow(String),
    /// Something answered but not as a Mica window of this protocol version.
    Incompatible(String),
    /// Live mode does not exist on this platform yet.
    Unsupported,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWindow(m) | Self::Incompatible(m) => f.write_str(m),
            Self::Unsupported => {
                f.write_str("live mode (the open Mica window) is not supported on Windows yet")
            }
        }
    }
}

#[cfg(unix)]
pub use unix::{Client, Listener, bind, serve_connection};

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::Duration;

    /// A connection to the window.
    pub struct Client {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
        next: u64,
        /// The window's `hello` answer.
        pub hello: Value,
        pub path: PathBuf,
    }

    impl Client {
        /// Connects and says hello. A missing socket, a refused connection (a stale socket
        /// left by a crashed window) or no answer is [`ConnectError::NoWindow`].
        pub fn connect(path: &Path) -> Result<Self, ConnectError> {
            let no_window = |why: String| {
                ConnectError::NoWindow(format!(
                    "no Mica window answers on {} ({why}); start Mica Slicer with the Mica MCP \
                     feature on (Settings › Features)",
                    path.display()
                ))
            };
            let stream = UnixStream::connect(path).map_err(|e| no_window(e.to_string()))?;
            // The hello must come back quickly; tool calls may take as long as a slice.
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| no_window(e.to_string()))?;
            let writer = stream.try_clone().map_err(|e| no_window(e.to_string()))?;
            let mut client = Self {
                reader: BufReader::new(stream),
                writer,
                next: 1,
                hello: Value::Null,
                path: path.to_path_buf(),
            };
            let hello = match client.call("hello", Value::Null) {
                Ok(Ok(content)) => content
                    .first()
                    .and_then(Content::as_text)
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
                    .unwrap_or(Value::Null),
                Ok(Err(e)) => return Err(ConnectError::Incompatible(e)),
                Err(e) => return Err(no_window(e)),
            };
            if hello.get("protocol").and_then(Value::as_u64) != Some(u64::from(PROTOCOL)) {
                return Err(ConnectError::Incompatible(format!(
                    "the Mica window on {} speaks another live protocol ({hello}); update Mica \
                     and the mica command to the same version",
                    path.display()
                )));
            }
            client.hello = hello;
            let _ = client.reader.get_ref().set_read_timeout(None);
            Ok(client)
        }

        /// One tool call. The outer error is the connection (the window closed); the inner
        /// one is the tool's error message.
        pub fn call(
            &mut self,
            tool: &str,
            args: Value,
        ) -> Result<Result<Vec<Content>, String>, String> {
            let id = self.next;
            self.next += 1;
            let request = Request {
                v: PROTOCOL,
                id,
                tool: tool.into(),
                args,
            };
            self.writer
                .write_all(encode(&request).as_bytes())
                .and_then(|()| self.writer.flush())
                .map_err(|e| format!("sending to the Mica window: {e}"))?;
            let mut line = String::new();
            let n = self
                .reader
                .read_line(&mut line)
                .map_err(|e| format!("reading from the Mica window: {e}"))?;
            if n == 0 {
                return Err("the Mica window closed the connection".into());
            }
            let response: Response = serde_json::from_str(line.trim_end())
                .map_err(|e| format!("the Mica window sent an unreadable answer: {e}"))?;
            if response.id != id {
                return Err(format!(
                    "the Mica window answered request {} instead of {id}",
                    response.id
                ));
            }
            Ok(response.result())
        }
    }

    /// The window's listening socket; the file is removed when it is dropped (if it is still
    /// this listener's file).
    pub struct Listener {
        pub listener: UnixListener,
        pub path: PathBuf,
        inode: u64,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            use std::os::unix::fs::MetadataExt;
            if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    /// Listens on `path` (mode 0600). A socket file nobody answers on (left by a window that
    /// crashed) is replaced; one another window answers on is left alone (`Err`).
    pub fn bind(path: &Path) -> Result<Listener, String> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if !meta.file_type().is_socket() {
                return Err(format!(
                    "{} exists and is not a socket; Mica MCP live mode is off",
                    path.display()
                ));
            }
            if UnixStream::connect(path).is_ok() {
                return Err(format!(
                    "another Mica window already serves Mica MCP on {}",
                    path.display()
                ));
            }
            std::fs::remove_file(path)
                .map_err(|e| format!("removing the stale {}: {e}", path.display()))?;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // Created with the process umask, then narrowed before anyone is accepted.
        let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let inode = std::fs::symlink_metadata(path)
            .map(|m| m.ino())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Listener {
            listener,
            path: path.to_path_buf(),
            inode,
        })
    }

    /// Serves one connection until it closes: `hello` and protocol errors are answered here,
    /// every other request goes to `handle` (which may wait for the window's UI thread).
    pub fn serve_connection(stream: UnixStream, mut handle: impl FnMut(Request) -> Response) {
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let Ok(line) = line else {
                return;
            };
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Err(e) => Response::new(0, Err(format!("unreadable request: {e}"))),
                Ok(r) if r.v != PROTOCOL => Response::new(
                    r.id,
                    Err(format!(
                        "this Mica window speaks live protocol {PROTOCOL}, the request {}; \
                         update Mica and the mica command to the same version",
                        r.v
                    )),
                ),
                Ok(r) if r.tool == "hello" => Response::new(
                    r.id,
                    Ok(vec![Content::Text {
                        text: hello(std::process::id()).to_string(),
                    }]),
                ),
                Ok(r) => handle(r),
            };
            if writer.write_all(encode(&response).as_bytes()).is_err() {
                return;
            }
        }
    }
}

/// Windows: no live mode yet; the server stays headless.
#[cfg(not(unix))]
pub struct Client {
    pub hello: Value,
    pub path: PathBuf,
}

#[cfg(not(unix))]
impl Client {
    pub fn connect(_path: &Path) -> Result<Self, ConnectError> {
        Err(ConnectError::Unsupported)
    }

    pub fn call(
        &mut self,
        _tool: &str,
        _args: Value,
    ) -> Result<Result<Vec<Content>, String>, String> {
        Err(ConnectError::Unsupported.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_one_line_each() {
        let r = Request {
            v: PROTOCOL,
            id: 3,
            tool: "paint_at".into(),
            args: serde_json::json!({"view_id": 1, "x": 10, "y": 20, "tool": 2}),
        };
        let line = encode(&r);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        assert_eq!(serde_json::from_str::<Request>(line.trim()).unwrap(), r);
        let ok = Response::new(
            3,
            Ok(vec![
                Content::Text {
                    text: "multi\nline".into(),
                },
                Content::Image {
                    data: "iVBOR".into(),
                    mime: "image/png".into(),
                },
            ]),
        );
        let line = encode(&ok);
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        let back: Response = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(back, ok);
        assert_eq!(back.result().unwrap().len(), 2);
        let err = Response::new(4, Err("no model".into()));
        let back: Response = serde_json::from_str(encode(&err).trim()).unwrap();
        assert_eq!(back.result(), Err("no model".to_string()));
        // A request without args is fine.
        let r: Request = serde_json::from_str(r#"{"v":1,"id":1,"tool":"status"}"#).unwrap();
        assert!(r.args.is_null());
    }

    #[cfg(unix)]
    #[test]
    fn stale_sockets_are_replaced_and_live_ones_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mica-live-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SOCKET_NAME);
        // No window: a clear message.
        let e = Client::connect(&path).err().expect("nothing listens");
        assert!(matches!(e, ConnectError::NoWindow(_)), "{e}");
        // A stale socket (its listener gone, the file left behind as after a crash).
        {
            let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
            drop(l);
        }
        assert!(path.exists());
        let e = Client::connect(&path).err().expect("stale");
        assert!(matches!(e, ConnectError::NoWindow(_)), "{e}");
        let listener = bind(&path).expect("a stale socket is replaced");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // A second window leaves the live one alone.
        let second = bind(&path).err().expect("in use");
        assert!(second.contains("already serves"), "{second}");
        // That check connected once; the window sees a connection that closes at once.
        drop(listener.listener.accept().unwrap());
        // Serve: hello, a tool, a wrong version.
        let l = listener.listener.try_clone().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = l.accept().unwrap();
            serve_connection(stream, |r| {
                Response::new(
                    r.id,
                    if r.tool == "echo" {
                        Ok(vec![Content::Text {
                            text: r.args.to_string(),
                        }])
                    } else {
                        Err(format!("unknown tool {}", r.tool))
                    },
                )
            });
        });
        let mut c = Client::connect(&path).expect("connects");
        assert_eq!(c.hello["protocol"], PROTOCOL);
        assert_eq!(c.hello["pid"], std::process::id());
        let out = c
            .call("echo", serde_json::json!({"a": 1}))
            .unwrap()
            .unwrap();
        assert_eq!(out[0].as_text(), Some(r#"{"a":1}"#));
        assert_eq!(
            c.call("nope", Value::Null).unwrap(),
            Err("unknown tool nope".to_string())
        );
        {
            use std::io::{BufRead, Write};
            let mut raw = std::os::unix::net::UnixStream::connect(&path).unwrap();
            drop(c);
            server.join().unwrap();
            // The listener still accepts; a request of another version is refused.
            let l = listener.listener.try_clone().unwrap();
            let server = std::thread::spawn(move || {
                let (stream, _) = l.accept().unwrap();
                serve_connection(stream, |r| Response::new(r.id, Ok(vec![])));
            });
            raw.write_all(b"{\"v\":99,\"id\":5,\"tool\":\"status\"}\n")
                .unwrap();
            let mut line = String::new();
            std::io::BufReader::new(raw.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let r: Response = serde_json::from_str(line.trim()).unwrap();
            assert!(!r.ok && r.error.unwrap().contains("protocol 1"));
            drop(raw);
            server.join().unwrap();
        }
        // Dropping the listener removes its socket file.
        drop(listener);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
