//! Mirroring to several TVs at once: one `MirrorSession` per target, keyed by the target string the
//! sender echoes back as `device_ip` (docs/control-protocol.md). Pure: no process, no clock.

use crate::mirror::{FollowUp, MirrorPhase, MirrorSession, MirrorStatus, SenderState};

#[derive(Debug, Default)]
pub struct MirrorTargets {
    /// In the order the user added them, so the list doesn't jump around.
    sessions: Vec<(String, MirrorSession)>,
}

impl MirrorTargets {
    /// Adds a TV to the mirror (or retries one that failed or ended). Refused while that TV is live.
    pub fn begin(&mut self, target: &str, name: &str) -> Result<(), String> {
        if let Some(session) = self.get_mut(target) {
            if session.is_active() {
                let name = session.status().device.clone().unwrap_or_else(|| name.to_string());
                return Err(format!("Already mirroring to {name}."));
            }
            session.begin(target, name);
            return Ok(());
        }
        let mut session = MirrorSession::default();
        session.begin(target, name);
        self.sessions.push((target.to_string(), session));
        Ok(())
    }

    pub fn get_mut(&mut self, target: &str) -> Option<&mut MirrorSession> {
        self.sessions.iter_mut().find(|(t, _)| t == target).map(|(_, s)| s)
    }

    pub fn connect_replied(&mut self, target: &str, ok: bool, error_kind: &str) {
        if let Some(session) = self.get_mut(target) {
            session.connect_replied(ok, error_kind);
        }
    }

    /// Hands a state line to every session (each reads only its own target). Returns the targets
    /// whose code was wrong, which must be connected again so the TV shows a new code.
    pub fn on_state(&mut self, line: &SenderState, now_ms: u64) -> Vec<String> {
        let mut reconnect = Vec::new();
        for (target, session) in &mut self.sessions {
            if session.on_state(line, now_ms) == FollowUp::Reconnect {
                reconnect.push(target.clone());
            }
        }
        reconnect
    }

    /// The TV a code goes to: `target` if it is asking, or without one, the only TV asking.
    pub fn asking_for_code(&self, target: Option<&str>) -> Option<String> {
        let mut asking = self.sessions.iter().filter(|(t, s)| s.status().phase == MirrorPhase::Code && target.is_none_or(|want| want == t));
        let (first, _) = asking.next()?;
        asking.next().is_none().then(|| first.clone())
    }

    /// Connecting, waiting for a code, or mirroring.
    pub fn is_active(&self, target: &str) -> bool {
        self.sessions.iter().any(|(t, s)| t == target && s.is_active())
    }

    pub fn any_active(&self) -> bool {
        self.sessions.iter().any(|(_, s)| s.is_active())
    }

    /// The user removed this TV (and the sender confirmed, if it was live).
    pub fn remove(&mut self, target: &str) {
        self.sessions.retain(|(t, _)| t != target);
    }

    /// Stop all.
    pub fn remove_all(&mut self) {
        self.sessions.clear();
    }

    /// The sender process ended without being asked to.
    pub fn sender_exited(&mut self) {
        for (_, session) in &mut self.sessions {
            session.sender_exited();
        }
    }

    /// One status per TV, in the order they were added. A TV whose session ended keeps its row
    /// (with the note) until it is removed or started again.
    pub fn statuses(&self) -> Vec<MirrorStatus> {
        self.sessions
            .iter()
            .map(|(target, session)| {
                let mut status = session.status().clone();
                status.target = Some(target.clone());
                status
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::SenderStream;

    const A: &str = "192.168.1.20";
    const B: &str = "192.168.1.21";

    fn line(streams: &[(&str, &str)], errors: &[(&str, &str)]) -> SenderState {
        SenderState {
            state: String::new(),
            streams: streams
                .iter()
                .map(|(ip, st)| SenderStream { device: String::new(), device_ip: (*ip).into(), state: (*st).into(), credential_kind: if *st == "pin_required" { "pin".into() } else { String::new() } })
                .collect(),
            errors: errors.iter().map(|(t, k)| crate::mirror::SenderError { target: (*t).into(), kind: (*k).into(), message: "detail".into() }).collect(),
        }
    }

    fn phases(t: &MirrorTargets) -> Vec<(String, MirrorPhase)> {
        t.statuses().into_iter().map(|s| (s.target.unwrap_or_default(), s.phase)).collect()
    }

    #[test]
    fn each_target_follows_its_own_stream() {
        let mut t = MirrorTargets::default();
        t.begin(A, "Living Room").unwrap();
        t.connect_replied(A, true, "");
        t.begin(B, "Bedroom").unwrap();
        t.connect_replied(B, true, "");
        t.on_state(&line(&[(A, "streaming"), (B, "pin_required")], &[]), 5);
        assert_eq!(phases(&t), vec![(A.into(), MirrorPhase::Mirroring), (B.into(), MirrorPhase::Code)]);
    }

    fn two_live() -> MirrorTargets {
        let mut t = MirrorTargets::default();
        for (ip, name) in [(A, "Living Room"), (B, "Bedroom")] {
            t.begin(ip, name).unwrap();
            t.connect_replied(ip, true, "");
        }
        t.on_state(&line(&[(A, "streaming"), (B, "streaming")], &[]), 5);
        t
    }

    #[test]
    fn a_wrong_code_on_one_tv_reconnects_only_that_tv() {
        let mut t = two_live();
        t.on_state(&line(&[(A, "streaming"), (B, "pin_required")], &[]), 6);
        t.get_mut(B).unwrap().code_sent();
        let again = t.on_state(&line(&[(A, "streaming")], &[(B, "wrong_code")]), 7);
        assert_eq!(again, vec![B.to_string()]);
        assert_eq!(phases(&t), vec![(A.into(), MirrorPhase::Mirroring), (B.into(), MirrorPhase::Connecting)]);
    }

    #[test]
    fn the_same_tv_twice_is_refused_while_live() {
        let mut t = two_live();
        assert_eq!(t.begin(A, "Living Room").unwrap_err(), "Already mirroring to Living Room.");
        assert_eq!(t.statuses().len(), 2);
    }

    #[test]
    fn removing_one_tv_leaves_the_other_and_stop_all_clears_the_list() {
        let mut t = two_live();
        t.remove(A);
        assert_eq!(phases(&t), vec![(B.into(), MirrorPhase::Mirroring)]);
        t.remove_all();
        assert!(t.statuses().is_empty());
    }

    #[test]
    fn a_failed_or_ended_tv_keeps_its_note_until_removed_or_retried() {
        let mut t = two_live();
        t.on_state(&line(&[(B, "streaming")], &[(A, "ended")]), 8);
        let a = &t.statuses()[0];
        assert_eq!((a.target.as_deref(), a.phase, a.message.as_deref()), (Some(A), MirrorPhase::Idle, Some("Mirroring to Living Room ended.")));
        assert!(!t.is_active(A));
        t.begin(A, "Living Room").unwrap();
        assert_eq!(phases(&t)[0], (A.into(), MirrorPhase::Connecting));
        assert_eq!(t.statuses()[0].message, None);
    }

    #[test]
    fn a_code_goes_to_the_named_tv_or_to_the_only_one_asking() {
        let mut t = two_live();
        assert_eq!(t.asking_for_code(None), None);
        t.on_state(&line(&[(A, "pin_required"), (B, "streaming")], &[]), 6);
        assert_eq!(t.asking_for_code(None).as_deref(), Some(A));
        assert_eq!(t.asking_for_code(Some(A)).as_deref(), Some(A));
        assert_eq!(t.asking_for_code(Some(B)), None, "B isn't asking");
        t.on_state(&line(&[(A, "pin_required"), (B, "pin_required")], &[]), 7);
        assert_eq!(t.asking_for_code(None), None, "two are asking: the code must name one");
        assert_eq!(t.asking_for_code(Some(B)).as_deref(), Some(B));
    }

    #[test]
    fn a_sender_crash_fails_every_live_tv() {
        let mut t = two_live();
        t.sender_exited();
        assert_eq!(phases(&t), vec![(A.into(), MirrorPhase::Error), (B.into(), MirrorPhase::Error)]);
        assert!(!t.any_active());
    }
}
