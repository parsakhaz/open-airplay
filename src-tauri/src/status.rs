//! The receiver status the UI shows, how engine events change it, and when a change is worth publishing.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::engine::{EngineEvent, ReceiverSettings};
use crate::supervisor::SupervisorEvent;

/// What the receiver is doing, as the UI shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Stopped,
    Starting,
    Ready,
    Connecting,
    Mirroring,
    Pin,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReceiverStatus {
    /// Rises with every published change; the UI keeps the snapshot with the highest number.
    pub seq: u64,
    pub phase: Phase,
    /// The user wants the receiver on (switch on, or auto-start); false after turning it off.
    pub wanted: bool,
    pub running: bool,
    pub engine_path: Option<String>,
    pub settings: Option<ReceiverSettings>,
    pub advertised: bool,
    pub connections: u32,
    pub last_device: Option<String>,
    /// Size of the mirrored video once frames arrive (reported by engine patch 0002).
    pub video: Option<(u32, u32)>,
    /// Unix time in ms when the current mirroring session's video started.
    pub mirroring_since_ms: Option<u64>,
    pub pin: Option<String>,
    pub last_error: Option<String>,
}

pub fn phase(s: &ReceiverStatus) -> Phase {
    if !s.running {
        return match (s.wanted, s.last_error.is_some()) {
            (false, _) => Phase::Stopped,
            (true, true) => Phase::Error,
            (true, false) => Phase::Starting,
        };
    }
    if s.pin.is_some() {
        Phase::Pin
    } else if s.video.is_some() {
        Phase::Mirroring
    } else if s.last_device.is_some() && s.connections > 0 {
        Phase::Connecting
    } else if s.advertised {
        Phase::Ready
    } else {
        Phase::Starting
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn end_session(status: &mut ReceiverStatus) {
    status.connections = 0;
    status.last_device = None;
    status.video = None;
    status.mirroring_since_ms = None;
    status.pin = None;
}

pub fn apply_event(status: &mut ReceiverStatus, event: &SupervisorEvent) {
    match event {
        SupervisorEvent::Started { .. } => {
            status.running = true;
            status.last_error = None;
        }
        SupervisorEvent::Exited { code } => {
            status.running = false;
            status.advertised = false;
            end_session(status);
            if status.wanted {
                status.last_error = Some(format!("The receiver stopped unexpectedly (exit code {code:?}). Restarting…"));
            }
        }
        SupervisorEvent::SpawnFailed { message } => {
            status.running = false;
            status.last_error = Some(format!("Could not start the receiver: {message}"));
        }
        SupervisorEvent::Engine { event } => match event {
            EngineEvent::Advertised => status.advertised = true,
            EngineEvent::ConnectionRequest { name, model, .. } => status.last_device = Some(format!("{name} ({model})")),
            EngineEvent::OpenConnections { count } => {
                if *count == 0 {
                    end_session(status);
                } else {
                    status.connections = *count;
                }
            }
            EngineEvent::Video { width, height } => {
                status.video = Some((*width, *height));
                status.pin = None;
                status.mirroring_since_ms.get_or_insert_with(now_ms);
            }
            EngineEvent::Pin { code } => status.pin = Some(code.clone()),
            EngineEvent::ClientLost => status.last_error = Some("Lost the connection to the sender.".into()),
            EngineEvent::Error { message } => status.last_error = Some(message.clone()),
        },
        SupervisorEvent::Log { .. } => {}
    }
    status.phase = phase(status);
}

/// The one copy of the receiver status, plus what was last published.
///
/// Every change goes through `update`, which says whether the snapshot differs from the last one
/// published; only those are worth sending to the UI. Log lines and repeated events change nothing,
/// so they publish nothing (a re-render per engine log line used to swallow clicks).
#[derive(Default)]
pub struct StatusHub {
    inner: Mutex<HubInner>,
}

#[derive(Default)]
struct HubInner {
    status: ReceiverStatus,
    published: Option<ReceiverStatus>,
}

impl StatusHub {
    /// The current status, numbered like the last published one (a command reply never outranks a push).
    pub fn snapshot(&self) -> ReceiverStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Changes the status. Returns the new snapshot when it differs from the last published one
    /// (with a higher `seq`), or None when there's nothing new to publish.
    pub fn update(&self, change: impl FnOnce(&mut ReceiverStatus)) -> Option<ReceiverStatus> {
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.status.seq;
        change(&mut inner.status);
        inner.status.seq = seq;
        inner.status.phase = phase(&inner.status);
        if inner.published.as_ref() == Some(&inner.status) {
            return None;
        }
        inner.status.seq = seq + 1;
        let snapshot = inner.status.clone();
        inner.published = Some(snapshot.clone());
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(advertised: bool) -> ReceiverStatus {
        ReceiverStatus { wanted: true, running: true, advertised, ..Default::default() }
    }

    fn engine(event: EngineEvent) -> SupervisorEvent {
        SupervisorEvent::Engine { event }
    }

    #[test]
    fn phase_follows_the_receiver_lifecycle() {
        assert_eq!(phase(&ReceiverStatus::default()), Phase::Stopped);
        assert_eq!(phase(&ReceiverStatus { wanted: true, ..Default::default() }), Phase::Starting);
        assert_eq!(phase(&running(false)), Phase::Starting);
        assert_eq!(phase(&running(true)), Phase::Ready);
        let error = ReceiverStatus { wanted: true, last_error: Some("boom".into()), ..Default::default() };
        assert_eq!(phase(&error), Phase::Error);
    }

    #[test]
    fn a_probe_connection_is_not_a_session() {
        // Other Apple devices open short connections to query the receiver; that's still "ready".
        let mut s = running(true);
        apply_event(&mut s, &engine(EngineEvent::OpenConnections { count: 1 }));
        assert_eq!(s.phase, Phase::Ready);
    }

    #[test]
    fn a_session_goes_connecting_then_mirroring_then_back_to_ready() {
        let mut s = running(true);
        apply_event(&mut s, &engine(EngineEvent::OpenConnections { count: 1 }));
        apply_event(
            &mut s,
            &engine(EngineEvent::ConnectionRequest { name: "Parsa's iPhone".into(), model: "iPhone16,1".into(), device_id: "x".into() }),
        );
        assert_eq!(s.phase, Phase::Connecting);
        apply_event(&mut s, &engine(EngineEvent::Video { width: 886, height: 1920 }));
        assert_eq!(s.phase, Phase::Mirroring);
        assert!(s.mirroring_since_ms.is_some());
        apply_event(&mut s, &engine(EngineEvent::OpenConnections { count: 0 }));
        assert_eq!(s.phase, Phase::Ready);
        assert_eq!((s.last_device.clone(), s.video), (None, None));
    }

    #[test]
    fn pin_shows_until_video_starts() {
        let mut s = running(true);
        apply_event(&mut s, &engine(EngineEvent::Pin { code: "4821".into() }));
        assert_eq!(s.phase, Phase::Pin);
        apply_event(&mut s, &engine(EngineEvent::Video { width: 1920, height: 886 }));
        assert_eq!(s.phase, Phase::Mirroring);
    }

    #[test]
    fn an_unexpected_exit_is_an_error_but_a_requested_stop_is_not() {
        let mut s = running(true);
        apply_event(&mut s, &SupervisorEvent::Exited { code: Some(1) });
        assert_eq!(s.phase, Phase::Error);
        let mut stopped = ReceiverStatus { wanted: false, running: true, ..Default::default() };
        apply_event(&mut stopped, &SupervisorEvent::Exited { code: Some(1) });
        assert_eq!(stopped.phase, Phase::Stopped);
    }

    /// A hub that has published one snapshot: a receiver that is starting.
    fn starting_hub() -> (StatusHub, ReceiverStatus) {
        let hub = StatusHub::default();
        let first = hub.update(|s| *s = running(false)).expect("first change publishes");
        (hub, first)
    }

    #[test]
    fn log_lines_publish_nothing() {
        let (hub, _) = starting_hub();
        for n in 0..200 {
            let log = SupervisorEvent::Log { line: format!("raop_rtp_mirror: frame {n}") };
            assert_eq!(hub.update(|s| apply_event(s, &log)), None);
        }
    }

    #[test]
    fn an_event_that_changes_nothing_publishes_nothing() {
        let (hub, _) = starting_hub();
        assert!(hub.update(|s| apply_event(s, &engine(EngineEvent::Advertised))).is_some());
        assert_eq!(hub.update(|s| apply_event(s, &engine(EngineEvent::Advertised))), None);
        // A probe connection (count only, no device) doesn't change what the UI shows either.
        assert!(hub.update(|s| apply_event(s, &engine(EngineEvent::OpenConnections { count: 1 }))).is_some());
        assert_eq!(hub.update(|s| apply_event(s, &engine(EngineEvent::OpenConnections { count: 1 }))), None);
    }

    #[test]
    fn each_published_snapshot_has_a_higher_number_and_replies_share_it() {
        let (hub, first) = starting_hub();
        assert_eq!(first.seq, 1);
        let second = hub.update(|s| apply_event(s, &engine(EngineEvent::Advertised))).unwrap();
        assert_eq!((second.seq, second.phase), (2, Phase::Ready));
        assert_eq!(hub.update(|s| apply_event(s, &SupervisorEvent::Log { line: "x".into() })), None);
        // A command reply reads the same numbered snapshot, so it can never outrank a later push.
        assert_eq!(hub.snapshot(), second);
        let third = hub.update(|s| apply_event(s, &engine(EngineEvent::Pin { code: "1234".into() }))).unwrap();
        assert_eq!(third.seq, 3);
    }
}
