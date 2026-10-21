// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "Found on your network": runs `mica_printhost::discovery` on a thread when the Add printer
//! window opens (and on Refresh) and hands the answers to the UI. Nothing runs while the
//! window is closed. Test and screenshot runs search a loopback fake responder only
//! (`target`), never the real network; without one they do not search at all.

use mica_printhost::discovery::{self, Discovered};
use std::{
    net::SocketAddr,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// How long a search listens for answers.
pub const WAIT: Duration = Duration::from_millis(1800);

pub enum Search {
    /// Not started (the window has not been opened yet).
    Idle,
    Running(mpsc::Receiver<Result<Vec<Discovered>, String>>),
    Done(Vec<Discovered>),
    Failed(String),
    /// This run does not search (tests, headless screenshots without a fake responder).
    Off(String),
}

pub struct Discovery {
    /// Where queries go: the mDNS group in the app, a loopback responder in screenshots.
    pub target: Option<SocketAddr>,
    pub state: Search,
    /// When the last search finished.
    pub finished: Option<Instant>,
}

impl Discovery {
    pub fn new(target: Option<SocketAddr>) -> Self {
        Self {
            target,
            state: Search::Idle,
            finished: None,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.state, Search::Running(_))
    }

    /// The printers found by the last finished search.
    pub fn found(&self) -> &[Discovered] {
        match &self.state {
            Search::Done(d) => d,
            _ => &[],
        }
    }

    /// Starts a search unless one is running.
    pub fn start(&mut self, wake: impl Fn() + Send + 'static) {
        if self.running() {
            return;
        }
        let loopback_only = super::loopback_only();
        let target = match self.target {
            None => {
                self.state = Search::Off("This run does not search the network.".into());
                return;
            }
            Some(t) if (loopback_only || cfg!(test)) && !t.ip().is_loopback() => {
                self.state = Search::Off(
                    "This run only talks to printers on this computer, so it does not search the network."
                        .into(),
                );
                return;
            }
            Some(t) => t,
        };
        let (tx, rx) = mpsc::channel();
        self.state = Search::Running(rx);
        thread::Builder::new()
            .name("mica-printer-search".into())
            .spawn(move || {
                let allow = move |address: &str| {
                    !(loopback_only || cfg!(test)) || super::is_loopback(address)
                };
                let result = discovery::discover(target, WAIT, allow).map_err(|e| {
                    format!("Mica could not search the network ({e}). Enter the printer's address instead.")
                });
                let _ = tx.send(result);
                wake();
            })
            .ok();
    }

    /// Takes a finished search; true when the state changed.
    pub fn poll(&mut self) -> bool {
        let Search::Running(rx) = &self.state else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(found)) => {
                self.state = Search::Done(found);
                self.finished = Some(Instant::now());
                true
            }
            Ok(Err(e)) => {
                self.state = Search::Failed(e);
                self.finished = Some(Instant::now());
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.state = Search::Failed("The network search stopped unexpectedly.".into());
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_printhost::discovery::FakeResponder;
    use std::net::UdpSocket;

    #[test]
    fn no_target_or_a_real_network_target_never_searches_in_tests() {
        let mut d = Discovery::new(None);
        d.start(|| {});
        assert!(matches!(d.state, Search::Off(_)));
        let mut d = Discovery::new(Some(discovery::MDNS_GROUP));
        d.start(|| {});
        assert!(matches!(d.state, Search::Off(_)));
    }

    #[test]
    fn a_loopback_responder_is_searched_and_its_printers_probed() {
        let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else {
            return;
        };
        // A closed port: the announced printer does not answer HTTP, so it is listed as
        // announced but offline (Moonraker announcements are trusted).
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .ok()
            .and_then(|l| l.local_addr().ok())
            .map_or(9, |a| a.port());
        let at = socket.local_addr().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        let server = thread::spawn(move || {
            FakeResponder {
                records: FakeResponder::instance(
                    "_moonraker._tcp.local",
                    "U1",
                    "snapmaker-u1",
                    closed,
                    &[],
                ),
            }
            .serve(&socket, || s2.load(std::sync::atomic::Ordering::Relaxed));
        });
        let mut d = Discovery::new(Some(at));
        d.start(|| {});
        let deadline = Instant::now() + Duration::from_secs(10);
        while d.running() && Instant::now() < deadline {
            d.poll();
            thread::sleep(Duration::from_millis(20));
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        server.join().unwrap();
        let found = d.found();
        assert_eq!(found.len(), 1, "{:?}", found);
        assert_eq!(found[0].name, "U1");
        assert_eq!(found[0].address, format!("127.0.0.1:{closed}"));
        assert_eq!(found[0].state, Some(mica_printhost::PrinterState::Offline));
    }
}
