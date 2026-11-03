// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica's polling budget: visible active jobs update within two seconds; idle cards need
//! only a ten-second refresh. Hidden active jobs retain a ten-second notification watch.
//! Failed requests wait 15, 30, then 60 seconds to limit traffic to an offline printer.
//! Minimising suspends all status requests; resuming starts a fresh deadline. These rates
//! are Mica product choices, independent of the host protocol. Camera budgets are separate.

use mica_printhost::PrinterState;
use std::time::{Duration, Instant};

pub const PRINTING: Duration = Duration::from_secs(2);
pub const IDLE: Duration = Duration::from_secs(10);
pub const FAILED: Duration = Duration::from_secs(15);
/// The longest wait after repeated failures (an offline printer).
pub const FAILED_MAX: Duration = Duration::from_secs(60);

/// Camera frame rates: dashboard thumbnails, the focused Machine view (also while the window
/// is in the background; snapshots are capped at `mica_camera::SNAPSHOT_MAX_FPS`), and
/// thumbnails while the window is in the background.
pub const THUMBNAIL_FPS: f32 = 1.;
pub const FOCUSED_FPS: f32 = 15.;
pub const BACKGROUND_FPS: f32 = 0.5;
/// My printers' cards: a fresh snapshot every three seconds while the window is open.
pub const CARD_FPS: f32 = 1. / 3.;

/// The polling interval for a printer in `state` (`None`: not known yet, treated as idle).
pub fn interval(state: Option<PrinterState>, failed: bool) -> Duration {
    match (failed, state) {
        (true, _) => FAILED,
        (false, Some(PrinterState::Printing | PrinterState::Paused)) => PRINTING,
        (false, _) => IDLE,
    }
}

/// The wait after `failures` failures in a row (1: the first).
pub fn backoff(failures: u32) -> Duration {
    match failures {
        0 | 1 => FAILED,
        2 => FAILED * 2,
        _ => FAILED_MAX,
    }
}

/// `visible`: the dashboard or the Machine view shows this printer; `window`: the app's
/// window is on screen (not minimised). `None` pauses polling.
pub fn wanted(
    visible: bool,
    window: bool,
    state: Option<PrinterState>,
    failed: bool,
) -> Option<Duration> {
    match (window, visible, state) {
        (false, _, _) => None,
        (true, true, _) => Some(interval(state, failed)),
        (true, false, Some(PrinterState::Printing | PrinterState::Paused)) => {
            Some(if failed { FAILED } else { IDLE })
        }
        (true, false, _) => None,
    }
}

/// Decides, frame by frame, whether a poll is due. Becoming visible polls at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ticker {
    pub next: Option<Instant>,
}
impl Ticker {
    pub fn due(&mut self, now: Instant, wanted: Option<Duration>) -> bool {
        match wanted {
            None => {
                self.next.take();
                false
            }
            Some(period) => {
                if self.next.is_some_and(|deadline| deadline > now) {
                    return false;
                }
                self.next = Some(now + period);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_follow_the_state_and_visibility() {
        assert_eq!(interval(Some(PrinterState::Printing), false), PRINTING);
        assert_eq!(interval(Some(PrinterState::Paused), false), PRINTING);
        assert_eq!(interval(Some(PrinterState::Idle), false), IDLE);
        assert_eq!(interval(Some(PrinterState::Finished), false), IDLE);
        assert_eq!(interval(None, false), IDLE);
        assert_eq!(interval(Some(PrinterState::Printing), true), FAILED);
        assert_eq!(
            wanted(true, true, Some(PrinterState::Printing), false),
            Some(PRINTING)
        );
        assert_eq!(
            wanted(true, true, Some(PrinterState::Idle), false),
            Some(IDLE)
        );
        // Not shown: a running job is still watched slowly (notifications); an idle printer
        // is not polled at all.
        assert_eq!(
            wanted(false, true, Some(PrinterState::Printing), false),
            Some(IDLE)
        );
        assert_eq!(wanted(false, true, Some(PrinterState::Idle), false), None);
        assert_eq!(wanted(false, true, None, false), None);
        // A minimised window: nothing.
        assert_eq!(
            wanted(true, false, Some(PrinterState::Printing), false),
            None
        );
    }

    #[test]
    fn failures_back_off_to_a_minute() {
        assert_eq!(backoff(1), FAILED);
        assert_eq!(backoff(2), Duration::from_secs(30));
        assert_eq!(backoff(3), FAILED_MAX);
        assert_eq!(backoff(40), FAILED_MAX);
    }

    #[test]
    fn ticker_polls_at_once_when_shown_then_at_the_interval() {
        let t0 = Instant::now();
        let mut t = Ticker::default();
        assert!(t.due(t0, Some(PRINTING)));
        assert!(!t.due(t0 + Duration::from_millis(1900), Some(PRINTING)));
        assert!(t.due(t0 + Duration::from_millis(2000), Some(PRINTING)));
        // Hidden: nothing is due, and showing again polls immediately.
        assert!(!t.due(t0 + Duration::from_secs(3), None));
        assert_eq!(t.next, None);
        assert!(t.due(t0 + Duration::from_secs(3), Some(IDLE)));
        assert!(!t.due(t0 + Duration::from_secs(12), Some(IDLE)));
        assert!(t.due(t0 + Duration::from_secs(13), Some(IDLE)));
    }

    #[test]
    fn pending_deadline_survives_rate_changes_and_minimising_resets_it() {
        let now = Instant::now();
        let mut ticker = Ticker::default();
        assert!(ticker.due(now, Some(IDLE)));
        assert!(!ticker.due(now + PRINTING, Some(PRINTING)));
        assert!(ticker.due(now + IDLE, Some(PRINTING)));
        assert_eq!(ticker.next, Some(now + IDLE + PRINTING));
        assert!(!ticker.due(
            now + IDLE,
            wanted(true, false, Some(PrinterState::Printing), false)
        ));
        assert!(ticker.due(now + IDLE, Some(PRINTING)));
        assert_eq!(backoff(0), FAILED);
        assert_eq!(backoff(u32::MAX), FAILED_MAX);
    }
}
