//! Notices when the PC wakes from sleep or moves to another network, so the receiver can restart and
//! announce itself again (UxPlay only advertises on the addresses it saw at startup).

use std::net::{IpAddr, UdpSocket};
use std::time::Duration;

/// Why the receiver should restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restart {
    Resumed,
    NetworkChanged { from: Option<IpAddr>, to: Option<IpAddr> },
}

/// One observation by the watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Wall-clock time that passed since the previous sample (the watcher ticks every few seconds;
    /// a much larger gap means the machine was asleep).
    pub wall_gap: Duration,
    pub primary_ip: Option<IpAddr>,
}

/// A wall-clock gap longer than this between ticks means the PC was asleep.
pub const SLEEP_GAP: Duration = Duration::from_secs(20);

pub fn decide(previous_ip: Option<IpAddr>, now: Sample) -> Option<Restart> {
    if now.wall_gap >= SLEEP_GAP {
        return Some(Restart::Resumed);
    }
    match (previous_ip, now.primary_ip) {
        (_, None) => None, // offline: nothing to announce on yet
        (from, to) if from != to => Some(Restart::NetworkChanged { from, to }),
        _ => None,
    }
}

/// The local address the OS would use to reach the internet (no packets are sent).
pub fn primary_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?; // TEST-NET-1; UDP connect only picks a route
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const HOME: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 199)));
    const CAFE: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 23)));
    const TICK: Duration = Duration::from_secs(3);

    #[test]
    fn nothing_happens_on_a_normal_tick() {
        assert_eq!(decide(HOME, Sample { wall_gap: TICK, primary_ip: HOME }), None);
    }

    #[test]
    fn a_long_wall_clock_gap_means_the_pc_slept() {
        assert_eq!(decide(HOME, Sample { wall_gap: Duration::from_secs(600), primary_ip: HOME }), Some(Restart::Resumed));
    }

    #[test]
    fn a_new_primary_address_means_a_new_network() {
        assert_eq!(
            decide(HOME, Sample { wall_gap: TICK, primary_ip: CAFE }),
            Some(Restart::NetworkChanged { from: HOME, to: CAFE })
        );
    }

    #[test]
    fn coming_back_online_restarts_but_going_offline_waits() {
        // Offline: nothing useful to announce on, so wait for a network rather than restart into nothing.
        assert_eq!(decide(HOME, Sample { wall_gap: TICK, primary_ip: None }), None);
        assert_eq!(
            decide(None, Sample { wall_gap: TICK, primary_ip: CAFE }),
            Some(Restart::NetworkChanged { from: None, to: CAFE })
        );
    }

    #[test]
    fn waking_on_a_different_network_is_reported_as_a_resume() {
        assert_eq!(decide(HOME, Sample { wall_gap: Duration::from_secs(3600), primary_ip: CAFE }), Some(Restart::Resumed));
    }

    #[test]
    fn primary_ip_is_a_real_local_address_when_online() {
        // Can't assume a network in every environment; only check what we get is sane.
        if let Some(ip) = primary_ip() {
            assert!(!ip.is_unspecified() && !ip.is_multicast());
        }
    }
}
