// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Finding printers on the local network: a one-shot mDNS / DNS-SD browse (RFC 6762, RFC
//! 6763) for the service types printer hosts announce, then a read-only look at each answer's
//! HTTP API to tell what it is.
//!
//! * **Service types.** The Snapmaker U1 announces `_snapmaker._tcp` (seen on a real LAN:
//!   instance "U1", the MQTT port 1884, TXT `version`, `machine_type=Snapmaker U1`,
//!   `device_name`, `sn`, `link_mode`, `userid`, `ip`, `region`); its Moonraker is asked on
//!   7125 at the TXT `ip` (else the resolved host), and `sn` / `userid` are dropped at once.
//!   Moonraker's `zeroconf` component announces `_moonraker._tcp` (off on the U1). Prusa-Link
//!   (the Raspberry Pi daemon) uses `_octoprint._tcp` (what PrusaSlicer's Bonjour browser looks
//!   for, TXT `version` / `model`); `_prusa-link._tcp` is browsed in case a firmware uses its
//!   own type. A CORE One and an XL on Buddy firmware announced nothing in that check:
//!   "Enter an address" is their way in. Plain `_http._tcp` answers are only looked at when
//!   their name or TXT hints at a printer, and only an API answer makes them one: smart plugs
//!   named "PrusaXL-…" or "U1-Plug…" answering 401 or 404 are left out.
//! * **Transport.** The query is sent from an ephemeral port, which makes it a "legacy
//!   unicast" query (RFC 6762 §6.7): responders answer straight to that port, so Mica does not
//!   join the multicast group or compete with the system's responder for port 5353.
//!   Answers missing their SRV / TXT / address records get one follow-up query.
//! * **Identification.** Every request is a GET of a status or version endpoint, sent without
//!   credentials (`/api/version`, `/api/v1/info`, `/api/v1/status` for PrusaLink;
//!   `/server/info`, `/printer/info`, `/printer/objects/list`, `/printer/objects/query` for
//!   Moonraker). A 401 means "a printer that wants its API key", which the Add printer flow
//!   then asks for. Nothing here uploads, starts or changes anything.
//!
//! The packet code is plain Rust over `std::net::UdpSocket` (no mDNS crate): it only has to
//! build a PTR query and read the five record types DNS-SD answers carry.

use crate::{
    Credentials, ErrorKind, Moonraker, PrinterConfig, PrinterHost, PrinterKind, PrinterState,
    PrusaLink, Timeouts, string,
};
use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

/// The mDNS multicast group and port.
pub const MDNS_GROUP: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 251)), 5353);

/// The DNS-SD service types browsed, most specific first.
pub const SERVICES: [&str; 5] = [
    "_snapmaker._tcp.local",
    "_moonraker._tcp.local",
    "_octoprint._tcp.local",
    "_prusa-link._tcp.local",
    "_http._tcp.local",
];

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_AAAA: u16 = 28;
const TYPE_SRV: u16 = 33;
const CLASS_IN: u16 = 1;

/// A domain name as its labels (`["Prusa XL", "_octoprint", "_tcp", "local"]`). Labels may
/// contain dots and spaces; compare with `key`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name(pub Vec<String>);
impl Name {
    pub fn parse(dotted: &str) -> Self {
        Self(
            dotted
                .trim_end_matches('.')
                .split('.')
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }
    /// Case-insensitive identity (DNS names compare without case).
    pub fn key(&self) -> String {
        self.0
            .iter()
            .map(|l| l.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\u{1}")
    }
    /// The name without its first label (an instance's service type).
    pub fn parent(&self) -> Name {
        Name(self.0.iter().skip(1).cloned().collect())
    }
    pub fn first(&self) -> &str {
        self.0.first().map_or("", String::as_str)
    }
}
impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    Ptr(Name),
    Srv { port: u16, target: Name },
    Txt(Vec<String>),
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Other(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub name: Name,
    pub data: Data,
}

/// A parsed DNS message: its id, whether it is a response, and every resource record of the
/// answer, authority and additional sections (questions are skipped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: u16,
    pub response: bool,
    pub records: Vec<Record>,
}

fn push_name(out: &mut Vec<u8>, name: &Name) {
    for label in &name.0 {
        let bytes = label.as_bytes();
        let n = bytes.len().min(63);
        out.push(n as u8);
        out.extend_from_slice(&bytes[..n]);
    }
    out.push(0);
}

/// A query for `questions` (`(name, type)`), class IN, unicast-response bit clear.
pub fn query(id: u16, questions: &[(Name, u16)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // standard query
    out.extend_from_slice(&(questions.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for (name, kind) in questions {
        push_name(&mut out, name);
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
    }
    out
}

/// A response carrying `records` as answers (fake responders: tests and the mock printers).
pub fn response(id: u16, records: &[Record]) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x8400u16.to_be_bytes()); // response, authoritative
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(records.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    for r in records {
        push_name(&mut out, &r.name);
        let (kind, rdata) = match &r.data {
            Data::Ptr(n) => {
                let mut d = vec![];
                push_name(&mut d, n);
                (TYPE_PTR, d)
            }
            Data::Srv { port, target } => {
                let mut d = vec![0, 0, 0, 0];
                d.extend_from_slice(&port.to_be_bytes());
                push_name(&mut d, target);
                (TYPE_SRV, d)
            }
            Data::Txt(entries) => {
                let mut d = vec![];
                for e in entries {
                    let b = e.as_bytes();
                    let n = b.len().min(255);
                    d.push(n as u8);
                    d.extend_from_slice(&b[..n]);
                }
                if d.is_empty() {
                    d.push(0);
                }
                (TYPE_TXT, d)
            }
            Data::A(ip) => (TYPE_A, ip.octets().to_vec()),
            Data::Aaaa(ip) => (TYPE_AAAA, ip.octets().to_vec()),
            Data::Other(k) => (*k, vec![]),
        };
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&120u32.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(&rdata);
    }
    out
}

/// The questions of a query: `(name, type)`.
pub fn questions(packet: &[u8]) -> Result<(u16, Vec<(Name, u16)>), String> {
    let mut r = Reader {
        data: packet,
        at: 0,
    };
    let id = r.u16()?;
    let _flags = r.u16()?;
    let qd = r.u16()?;
    r.skip(6)?;
    let mut out = vec![];
    for _ in 0..qd {
        let name = r.name()?;
        let kind = r.u16()?;
        let _class = r.u16()?;
        out.push((name, kind));
    }
    Ok((id, out))
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.at).ok_or("truncated packet")?;
        self.at += 1;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes([
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
        ]))
    }
    fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.at + n > self.data.len() {
            return Err("truncated packet".into());
        }
        self.at += n;
        Ok(())
    }
    /// A possibly compressed name starting at the cursor.
    fn name(&mut self) -> Result<Name, String> {
        let (name, end) = read_name(self.data, self.at)?;
        self.at = end;
        Ok(name)
    }
}

/// Reads a name at `at`; returns it and the offset just past it in the original stream.
fn read_name(data: &[u8], mut at: usize) -> Result<(Name, usize), String> {
    let mut labels = vec![];
    let mut end = None;
    let mut jumps = 0;
    let mut total = 0;
    loop {
        let len = *data.get(at).ok_or("truncated name")? as usize;
        match len & 0xc0 {
            0x00 => {
                if len == 0 {
                    at += 1;
                    break;
                }
                let bytes = data.get(at + 1..at + 1 + len).ok_or("truncated label")?;
                total += len + 1;
                if total > 255 {
                    return Err("name too long".into());
                }
                labels.push(String::from_utf8_lossy(bytes).into_owned());
                at += 1 + len;
            }
            0xc0 => {
                let low = *data.get(at + 1).ok_or("truncated pointer")? as usize;
                let target = ((len & 0x3f) << 8) | low;
                if end.is_none() {
                    end = Some(at + 2);
                }
                jumps += 1;
                if jumps > 16 || target >= data.len() {
                    return Err("bad name pointer".into());
                }
                at = target;
            }
            _ => return Err("unsupported label type".into()),
        }
    }
    Ok((Name(labels), end.unwrap_or(at)))
}

/// Parses a DNS message (questions skipped, every record section read).
pub fn parse(packet: &[u8]) -> Result<Message, String> {
    let mut r = Reader {
        data: packet,
        at: 0,
    };
    let id = r.u16()?;
    let flags = r.u16()?;
    let qd = r.u16()?;
    let an = r.u16()?;
    let ns = r.u16()?;
    let ar = r.u16()?;
    for _ in 0..qd {
        r.name()?;
        r.skip(4)?;
    }
    let mut records = vec![];
    for _ in 0..(an as usize + ns as usize + ar as usize) {
        let name = r.name()?;
        let kind = r.u16()?;
        let _class = r.u16()?;
        let _ttl = r.u32()?;
        let len = r.u16()? as usize;
        let start = r.at;
        let rdata = packet.get(start..start + len).ok_or("truncated record")?;
        let data = match kind {
            TYPE_PTR => Data::Ptr(read_name(packet, start)?.0),
            TYPE_SRV if len >= 7 => {
                let port = u16::from_be_bytes([rdata[4], rdata[5]]);
                Data::Srv {
                    port,
                    target: read_name(packet, start + 6)?.0,
                }
            }
            TYPE_TXT => {
                let mut entries = vec![];
                let mut i = 0;
                while i < rdata.len() {
                    let n = rdata[i] as usize;
                    let e = rdata.get(i + 1..i + 1 + n).ok_or("truncated TXT")?;
                    if !e.is_empty() {
                        entries.push(String::from_utf8_lossy(e).into_owned());
                    }
                    i += 1 + n;
                }
                Data::Txt(entries)
            }
            TYPE_A if len == 4 => Data::A(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3])),
            TYPE_AAAA if len == 16 => {
                let mut b = [0u8; 16];
                b.copy_from_slice(rdata);
                Data::Aaaa(Ipv6Addr::from(b))
            }
            other => Data::Other(other),
        };
        r.at = start + len;
        records.push(Record { name, data });
    }
    Ok(Message {
        id,
        response: flags & 0x8000 != 0,
        records,
    })
}

/// One announced service instance, assembled from its PTR, SRV, TXT and address records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    pub instance: Name,
    /// The service type (`_moonraker._tcp.local`), lower case.
    pub service: String,
    pub host: Option<Name>,
    pub port: Option<u16>,
    pub addresses: Vec<IpAddr>,
    /// TXT `key=value` pairs, keys lower case.
    pub txt: BTreeMap<String, String>,
}
impl Service {
    /// The instance's own label ("Prusa XL"), the name a person gave the printer.
    pub fn label(&self) -> &str {
        self.instance.first()
    }
    /// Whether SRV, TXT-independent address and port are all known.
    pub fn complete(&self) -> bool {
        self.port.is_some() && !self.addresses.is_empty()
    }
}

/// Collects records from any number of answers and assembles the services of `types`.
#[derive(Clone, Debug, Default)]
pub struct Browse {
    records: Vec<Record>,
}
impl Browse {
    pub fn add(&mut self, message: &Message) {
        for r in &message.records {
            if !self.records.contains(r) {
                self.records.push(r.clone());
            }
        }
    }
    pub fn services(&self, types: &[&str]) -> Vec<Service> {
        let wanted: Vec<String> = types.iter().map(|t| Name::parse(t).key()).collect();
        let mut out: Vec<Service> = vec![];
        for r in &self.records {
            let Data::Ptr(instance) = &r.data else {
                continue;
            };
            if !wanted.contains(&r.name.key()) || instance.parent().key() != r.name.key() {
                continue;
            }
            if out.iter().any(|s| s.instance.key() == instance.key()) {
                continue;
            }
            let mut s = Service {
                instance: instance.clone(),
                service: r.name.to_string().to_ascii_lowercase(),
                ..Service::default()
            };
            for x in self
                .records
                .iter()
                .filter(|x| x.name.key() == instance.key())
            {
                match &x.data {
                    Data::Srv { port, target } => {
                        s.port = Some(*port);
                        s.host = Some(target.clone());
                    }
                    Data::Txt(entries) => {
                        for e in entries {
                            let (k, v) = e.split_once('=').unwrap_or((e.as_str(), ""));
                            s.txt.insert(k.to_ascii_lowercase(), v.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if let Some(host) = &s.host {
                for x in self.records.iter().filter(|x| x.name.key() == host.key()) {
                    let ip = match x.data {
                        Data::A(ip) => IpAddr::V4(ip),
                        Data::Aaaa(ip) => IpAddr::V6(ip),
                        _ => continue,
                    };
                    if !s.addresses.contains(&ip) {
                        s.addresses.push(ip);
                    }
                }
                // IPv4 first: printers' web servers are reliably there.
                s.addresses.sort_by_key(|a| (a.is_ipv6(), *a));
            }
            out.push(s);
        }
        out
    }
}

/// Browses `types` by sending a legacy-unicast query to `target` (the multicast group, or a
/// fake responder on loopback in tests) and collecting answers for `wait`. One follow-up asks
/// for the records incomplete answers lack.
pub fn browse(target: SocketAddr, types: &[&str], wait: Duration) -> io::Result<Vec<Service>> {
    let bind: SocketAddr = if target.ip().is_loopback() {
        (Ipv4Addr::LOCALHOST, 0).into()
    } else {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    };
    let socket = UdpSocket::bind(bind)?;
    if target.ip().is_multicast() {
        // Link-local only (RFC 6762 §11 asks for 255 so responders can verify it).
        let _ = socket.set_multicast_ttl_v4(255);
    }
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let id = (std::process::id() as u16) ^ 0x4d49;
    let ptrs: Vec<(Name, u16)> = types.iter().map(|t| (Name::parse(t), TYPE_PTR)).collect();
    let packet = query(id, &ptrs);
    let mut browse = Browse::default();
    let start = Instant::now();
    socket.send_to(&packet, target)?;
    let mut buf = vec![0u8; 9000];
    let collect = |browse: &mut Browse, until: Instant, socket: &UdpSocket, buf: &mut [u8]| {
        while Instant::now() < until {
            match socket.recv_from(buf) {
                Ok((n, _)) => {
                    if let Ok(m) = parse(&buf[..n])
                        && m.response
                    {
                        browse.add(&m);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    };
    let half = start + wait / 3;
    collect(&mut browse, half, &socket, &mut buf);
    // Packets get lost on busy Wi-Fi; one repeat, as Bonjour clients do.
    let _ = socket.send_to(&packet, target);
    collect(&mut browse, start + wait, &socket, &mut buf);
    let incomplete: Vec<Service> = browse
        .services(types)
        .into_iter()
        .filter(|s| !s.complete())
        .collect();
    if !incomplete.is_empty() {
        let mut follow = vec![];
        for s in &incomplete {
            match &s.host {
                None => {
                    follow.push((s.instance.clone(), TYPE_SRV));
                    follow.push((s.instance.clone(), TYPE_TXT));
                }
                Some(h) => {
                    follow.push((h.clone(), TYPE_A));
                }
            }
        }
        let _ = socket.send_to(&query(id.wrapping_add(1), &follow), target);
        collect(
            &mut browse,
            Instant::now() + Duration::from_millis(700),
            &socket,
            &mut buf,
        );
        // Hosts learned from the SRV answers just now.
        let hosts: Vec<(Name, u16)> = browse
            .services(types)
            .into_iter()
            .filter(|s| s.addresses.is_empty())
            .filter_map(|s| s.host.map(|h| (h, TYPE_A)))
            .collect();
        if !hosts.is_empty() {
            let _ = socket.send_to(&query(id.wrapping_add(2), &hosts), target);
            collect(
                &mut browse,
                Instant::now() + Duration::from_millis(500),
                &socket,
                &mut buf,
            );
        }
    }
    Ok(browse.services(types))
}

/// A service worth asking: where it listens and what its announcement suggests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The instance name ("Prusa XL"), else the host name.
    pub name: String,
    /// `ip:port` (IPv6 in brackets).
    pub address: String,
    pub hint: Option<PrinterKind>,
    pub txt: BTreeMap<String, String>,
    /// The host's mDNS name without `.local` ("prusa-xl").
    pub host: Option<String>,
}

fn printer_words(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "prusa",
        "printer",
        "klipper",
        "moonraker",
        "mainsail",
        "fluidd",
        "snapmaker",
        "octoprint",
        "voron",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// The services to probe, one per address and port: the specific service types win over
/// `_http._tcp`, which is only kept when its name or TXT record looks like a printer.
pub fn candidates(services: &[Service]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = vec![];
    for s in services {
        let hint = match s.service.as_str() {
            t if t.starts_with("_snapmaker.") => Some(PrinterKind::SnapmakerU1),
            t if t.starts_with("_moonraker.") => Some(PrinterKind::Moonraker),
            t if t.starts_with("_octoprint.") || t.starts_with("_prusa-link.") => {
                Some(PrinterKind::PrusaLink)
            }
            _ => None,
        };
        // Serial numbers and account ids never leave this function.
        let txt: BTreeMap<String, String> = s
            .txt
            .iter()
            .filter(|(k, _)| !SECRET_TXT.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let (ip, port) = if hint == Some(PrinterKind::SnapmakerU1) {
            // The U1 announces its MQTT port; Moonraker listens on 7125. Its TXT names the
            // address it is reachable at.
            let ip = txt
                .get("ip")
                .and_then(|i| i.parse::<IpAddr>().ok())
                .or_else(|| s.addresses.first().copied());
            (ip, Some(MOONRAKER_PORT))
        } else {
            (s.addresses.first().copied(), s.port)
        };
        let (Some(port), Some(ip)) = (port, ip) else {
            continue;
        };
        let host = s
            .host
            .as_ref()
            .map(|h| h.first().to_string())
            .filter(|h| !h.is_empty());
        if hint.is_none() {
            let words: String = txt.iter().map(|(k, v)| format!("{k}={v} ")).collect();
            if !printer_words(s.label())
                && !printer_words(&words)
                && !host.as_deref().is_some_and(printer_words)
            {
                continue;
            }
        }
        let address = match ip {
            IpAddr::V4(v4) => format!("{v4}:{port}"),
            IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        };
        let name = txt
            .get("device_name")
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .or_else(|| (!s.label().is_empty()).then(|| s.label().to_string()))
            .or_else(|| host.clone())
            .unwrap_or_else(|| address.clone());
        match out.iter_mut().find(|c| c.address == address) {
            Some(c) => {
                if c.hint.is_none() && hint.is_some() {
                    c.hint = hint;
                    c.name = name;
                }
                for (k, v) in &txt {
                    c.txt.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            None => out.push(Candidate {
                name,
                address,
                hint,
                txt,
                host,
            }),
        }
    }
    out
}

/// Moonraker's default port (the U1 announces another service's port).
pub const MOONRAKER_PORT: u16 = 7125;

/// TXT keys that identify a device or an account (the U1's `sn`, `userid`): dropped.
const SECRET_TXT: [&str; 4] = ["sn", "serial", "userid", "user_id"];

/// What a printer said about itself when asked without credentials.
#[derive(Clone, Debug, PartialEq)]
pub struct Discovered {
    pub name: String,
    pub address: String,
    pub kind: PrinterKind,
    /// The model as reported or guessed ("Original Prusa XL 5T", "Snapmaker U1").
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub nozzle: Option<f64>,
    /// Number of tools, when the printer says.
    pub tools: Option<u32>,
    pub state: Option<PrinterState>,
    pub progress: Option<f64>,
    /// The printer answered 401: it needs an API key (or a login) first.
    pub needs_key: bool,
    /// The host name the printer reports (or announces).
    pub hostname: Option<String>,
}

/// A Prusa model named by a host name, mDNS name or TXT `model` value ("prusa-xl",
/// "PrusaCoreOne", "MK4S"), for printers that do not report it without a key.
pub fn guess_model(text: &str) -> Option<&'static str> {
    let t: String = text
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let has = |w: &str| t.contains(w);
    if has("coreonel") {
        Some("Prusa CORE One L")
    } else if has("coreone") || has("c1") && has("prusa") {
        Some("Prusa CORE One")
    } else if has("xl") && (has("prusa") || t.len() <= 4) {
        Some("Original Prusa XL")
    } else if has("mk4s") {
        Some("Original Prusa MK4S")
    } else if has("mk4") {
        Some("Original Prusa MK4")
    } else if has("mk39") || has("mk3.9") {
        Some("Original Prusa MK3.9")
    } else if has("mk35") {
        Some("Original Prusa MK3.5")
    } else if has("mini") && has("prusa") {
        Some("Original Prusa MINI")
    } else if has("snapmakeru1") || has("u1") && has("snapmaker") {
        Some("Snapmaker U1")
    } else {
        None
    }
}

fn config(kind: PrinterKind, address: &str) -> PrinterConfig {
    PrinterConfig {
        name: "discovery".into(),
        kind,
        address: address.to_string(),
        credentials: Credentials::None,
    }
}

/// Asks a Moonraker for what it is. `None`: not a Moonraker.
fn probe_moonraker(c: &Candidate, timeouts: Timeouts) -> Option<Discovered> {
    let host = Moonraker::new(config(PrinterKind::Moonraker, &c.address), timeouts).ok()?;
    let mut d = Discovered {
        name: c.name.clone(),
        address: c.address.clone(),
        kind: PrinterKind::Moonraker,
        model: None,
        firmware: None,
        nozzle: None,
        tools: None,
        state: None,
        progress: None,
        needs_key: false,
        hostname: c.host.clone(),
    };
    // Only an announcement says "printer": a smart plug named "U1-Plug" answering 401 is
    // not one.
    let announced = matches!(
        c.hint,
        Some(PrinterKind::Moonraker | PrinterKind::SnapmakerU1)
    );
    let machine_type = c
        .txt
        .get("machine_type")
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let snapmaker_named = c.hint == Some(PrinterKind::SnapmakerU1)
        || machine_type
            .as_deref()
            .is_some_and(|m| guess_model(m) == Some("Snapmaker U1"))
        || announced
            && [c.name.as_str(), c.host.as_deref().unwrap_or("")]
                .iter()
                .any(|t| guess_model(t) == Some("Snapmaker U1"));
    if snapmaker_named {
        d.kind = PrinterKind::SnapmakerU1;
        d.model = Some(
            machine_type
                .clone()
                .unwrap_or_else(|| "Snapmaker U1".into()),
        );
        // The U1's own firmware version (its TXT), rather than Klipper's.
        d.firmware = c.txt.get("version").cloned().filter(|v| !v.is_empty());
    }
    match host.info() {
        Err(e) if e.kind == ErrorKind::Unauthorised => {
            if !announced {
                return None;
            }
            d.needs_key = true;
            return Some(d);
        }
        Err(_) => {
            // Not Moonraker at all unless it was announced as one.
            return announced.then(|| {
                d.state = Some(PrinterState::Offline);
                d
            });
        }
        Ok(info) => {
            if d.firmware.is_none() {
                d.firmware = info.firmware;
            }
            d.hostname = info.model.clone().or(d.hostname);
            d.tools = info.tools.as_ref().map(|t| t.len() as u32);
            d.state = Some(info.state);
        }
    }
    let objects = host.objects().unwrap_or_default();
    let snapmaker = objects
        .iter()
        .any(|o| o == "print_task_config" || o == "filament_detect")
        || snapmaker_named;
    if snapmaker {
        d.kind = PrinterKind::SnapmakerU1;
        if d.model.is_none() {
            d.model = Some("Snapmaker U1".into());
        }
    }
    if let Ok(s) = host.status() {
        d.state = Some(s.state);
        d.progress = s.progress;
    }
    if let Ok(v) = host
        .http
        .call("GET", "/printer/objects/query?configfile=settings", None)
    {
        d.nozzle = crate::number(
            &v["result"]["status"]["configfile"]["settings"]["extruder"]["nozzle_diameter"],
        );
    }
    Some(d)
}

/// Asks a PrusaLink for what it is. `None`: not a PrusaLink.
fn probe_prusalink(c: &Candidate, timeouts: Timeouts) -> Option<Discovered> {
    let host = PrusaLink::new(config(PrinterKind::PrusaLink, &c.address), timeouts).ok()?;
    let guessed = [
        c.txt.get("model").map(String::as_str),
        Some(c.name.as_str()),
        c.host.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(guess_model)
    .map(str::to_string);
    let mut d = Discovered {
        name: c.name.clone(),
        address: c.address.clone(),
        kind: PrinterKind::PrusaLink,
        model: guessed,
        firmware: c.txt.get("version").cloned().filter(|v| !v.is_empty()),
        nozzle: None,
        tools: None,
        state: None,
        progress: None,
        needs_key: false,
        hostname: c.host.clone(),
    };
    let version = match host.http.call("GET", "/api/version", None) {
        Ok(v) => v,
        Err(e) if e.kind == ErrorKind::Unauthorised => {
            d.needs_key = true;
            // A 401 from an unannounced HTTP server is not evidence of a printer.
            // (A smart plug named "PrusaXL-…" answering 401 is not a printer either.)
            return (c.hint == Some(PrinterKind::PrusaLink)).then_some(d);
        }
        Err(_) => return None,
    };
    if version.get("api").is_none() && version.get("text").is_none() {
        return None;
    }
    if let Some(h) = string(&version["hostname"]) {
        if d.model.is_none() {
            d.model = guess_model(&h).map(str::to_string);
        }
        d.hostname = Some(h);
    }
    d.nozzle = crate::number(&version["nozzle_diameter"]);
    match host.http.call("GET", "/api/v1/info", None) {
        Ok(v) => {
            if let Some(m) = string(&v["printer_model"]).or_else(|| string(&v["model"])) {
                d.model = Some(m);
            }
            d.firmware = string(&v["firmware"]).or(d.firmware);
            d.nozzle = crate::number(&v["nozzle_diameter"]).or(d.nozzle);
            if let Some(n) = v["tools"]
                .as_object()
                .map(|t| t.len() as u32)
                .filter(|n| *n > 0)
            {
                d.tools = Some(n);
            }
            if d.name == c.address
                && let Some(n) = string(&v["name"])
            {
                d.name = n;
            }
        }
        Err(e) if e.kind == ErrorKind::Unauthorised => d.needs_key = true,
        Err(_) => {}
    }
    match host.status() {
        Ok(s) => {
            d.state = Some(s.state);
            d.progress = s.progress;
        }
        Err(e) if e.kind == ErrorKind::Unauthorised => d.needs_key = true,
        Err(_) => {}
    }
    if d.firmware.is_none() {
        d.firmware = string(&version["firmware"]);
    }
    Some(d)
}

/// Identifies one candidate (read-only requests, no credentials). `None`: no printer API
/// answered.
pub fn identify(c: &Candidate, timeouts: Timeouts) -> Option<Discovered> {
    match c.hint {
        Some(PrinterKind::Moonraker | PrinterKind::SnapmakerU1) => probe_moonraker(c, timeouts),
        Some(PrinterKind::PrusaLink) => probe_prusalink(c, timeouts),
        Some(PrinterKind::PrusaConnect) => None,
        None => probe_prusalink(c, timeouts).or_else(|| probe_moonraker(c, timeouts)),
    }
}

/// Short timeouts for probing many devices at once.
pub fn probe_timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_millis(1500),
        request: Duration::from_secs(3),
        upload: Duration::from_secs(3),
    }
}

/// Browses and identifies, one thread per candidate. `allow` filters the addresses that may
/// be contacted (the GUI's loopback-only test runs).
pub fn discover(
    target: SocketAddr,
    wait: Duration,
    allow: impl Fn(&str) -> bool,
) -> io::Result<Vec<Discovered>> {
    let services = browse(target, &SERVICES, wait)?;
    let found: Vec<Candidate> = candidates(&services)
        .into_iter()
        .filter(|c| allow(&c.address))
        .collect();
    let handles: Vec<_> = found
        .into_iter()
        .map(|c| std::thread::spawn(move || identify(&c, probe_timeouts())))
        .collect();
    let mut out: Vec<Discovered> = handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect();
    out.sort_by_key(|d| d.name.to_lowercase());
    Ok(out)
}

/// A fake responder answering every query that names one of its records' owners (tests and
/// the mock printers). Records are matched by name and type; PTR answers carry the
/// instance's SRV, TXT and the host's A records as additional answers, like Avahi does.
pub struct FakeResponder {
    pub records: Vec<Record>,
}
impl FakeResponder {
    /// The records answering `questions`.
    pub fn answer(&self, questions: &[(Name, u16)]) -> Vec<Record> {
        let mut out: Vec<Record> = vec![];
        let push = |r: &Record, out: &mut Vec<Record>| {
            if !out.contains(r) {
                out.push(r.clone());
            }
        };
        let kind_of = |d: &Data| match d {
            Data::Ptr(_) => TYPE_PTR,
            Data::Srv { .. } => TYPE_SRV,
            Data::Txt(_) => TYPE_TXT,
            Data::A(_) => TYPE_A,
            Data::Aaaa(_) => TYPE_AAAA,
            Data::Other(k) => *k,
        };
        for (name, kind) in questions {
            for r in &self.records {
                if r.name.key() != name.key() || kind_of(&r.data) != *kind {
                    continue;
                }
                push(r, &mut out);
                if let Data::Ptr(instance) = &r.data {
                    for x in self
                        .records
                        .iter()
                        .filter(|x| x.name.key() == instance.key())
                    {
                        push(x, &mut out);
                        if let Data::Srv { target, .. } = &x.data {
                            for a in self.records.iter().filter(|a| a.name.key() == target.key()) {
                                push(a, &mut out);
                            }
                        }
                    }
                }
            }
        }
        out
    }
    /// Serves on `socket` until `stop` returns true (checked every 100 ms).
    pub fn serve(&self, socket: &UdpSocket, stop: impl Fn() -> bool) {
        let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
        let mut buf = vec![0u8; 2048];
        while !stop() {
            let Ok((n, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let Ok((id, qs)) = questions(&buf[..n]) else {
                continue;
            };
            let answers = self.answer(&qs);
            if !answers.is_empty() {
                let _ = socket.send_to(&response(id, &answers), from);
            }
        }
    }
    /// The records of one service instance on 127.0.0.1.
    pub fn instance(
        service: &str,
        label: &str,
        host: &str,
        port: u16,
        txt: &[&str],
    ) -> Vec<Record> {
        let service = Name::parse(service);
        let mut instance = vec![label.to_string()];
        instance.extend(service.0.iter().cloned());
        let instance = Name(instance);
        let host = Name::parse(&format!("{host}.local"));
        vec![
            Record {
                name: service,
                data: Data::Ptr(instance.clone()),
            },
            Record {
                name: instance.clone(),
                data: Data::Srv {
                    port,
                    target: host.clone(),
                },
            },
            Record {
                name: instance,
                data: Data::Txt(txt.iter().map(|t| t.to_string()).collect()),
            },
            Record {
                name: host,
                data: Data::A(Ipv4Addr::LOCALHOST),
            },
        ]
    }
}
