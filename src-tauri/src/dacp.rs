//! Play/pause/skip on the phone that is playing music to this PC.
//!
//! The phone runs a small remote-control web service (DACP, the protocol Apple's Remote app used).
//! The engine reports its id and the session's Active-Remote token (engine patch 0005, `-dacp`);
//! the phone advertises the service over mDNS as `iTunes_Ctrl_<id>._dacp._tcp.local.`.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// The phone's remote-control service for the current session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub dacp_id: String,
    pub active_remote: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    PlayPause,
    Next,
    Previous,
}

impl Command {
    fn path(self) -> &'static str {
        match self {
            Command::PlayPause => "/ctrl-int/1/playpause",
            Command::Next => "/ctrl-int/1/nextitem",
            Command::Previous => "/ctrl-int/1/previtem",
        }
    }
}

const SERVICE: &str = "_dacp._tcp.local.";
const FIND_TIMEOUT: Duration = Duration::from_secs(3);
const SEND_TIMEOUT: Duration = Duration::from_secs(3);
const NOT_FOUND: &str = "Couldn't reach the phone to control playback. Use the controls on the phone.";

/// Sends `command` to the phone: finds its remote-control service, then asks it.
pub fn control(remote: &Remote, command: Command) -> Result<(), String> {
    let addr = find(&remote.dacp_id, FIND_TIMEOUT)?;
    send(addr, remote, command, SEND_TIMEOUT)
}

/// Finds the phone's remote-control service on the network.
pub fn find(dacp_id: &str, timeout: Duration) -> Result<SocketAddr, String> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let fullname = format!("iTunes_Ctrl_{dacp_id}.{SERVICE}");
    let mdns = ServiceDaemon::new().map_err(|e| {
        eprintln!("open-airplay: dacp: mDNS unavailable: {e}");
        NOT_FOUND.to_string()
    })?;
    let found = (|| {
        let events = mdns.browse(SERVICE).ok()?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            match events.recv_timeout(left).ok()? {
                ServiceEvent::ServiceResolved(s) if s.fullname.eq_ignore_ascii_case(&fullname) => {
                    // Prefer IPv4: a link-local IPv6 address needs its scope to connect.
                    let mut ips: Vec<IpAddr> = s.addresses.iter().map(|a| a.to_ip_addr()).collect();
                    ips.sort_by_key(|ip| !ip.is_ipv4());
                    return ips.first().map(|ip| SocketAddr::new(*ip, s.port));
                }
                _ => {}
            }
        }
    })();
    let _ = mdns.shutdown();
    found.ok_or_else(|| NOT_FOUND.to_string())
}

/// The HTTP request for one command.
fn request(command: Command, remote: &Remote, addr: SocketAddr) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {addr}\r\nActive-Remote: {}\r\nConnection: close\r\n\r\n",
        command.path(),
        remote.active_remote
    )
}

/// Asks the phone's remote-control service at `addr` to run `command`.
pub fn send(addr: SocketAddr, remote: &Remote, command: Command, timeout: Duration) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| {
        eprintln!("open-airplay: dacp {command:?} to {addr}: {e}");
        NOT_FOUND.to_string()
    };
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| failed(&e))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| failed(&e))?;
    stream.write_all(request(command, remote, addr).as_bytes()).map_err(|e| failed(&e))?;
    // Only the status line matters: "HTTP/1.1 204 No Content".
    let mut head = [0u8; 64];
    let n = stream.read(&mut head).map_err(|e| failed(&e))?;
    let status = String::from_utf8_lossy(&head[..n]);
    let code = status.split_whitespace().nth(1).unwrap_or("");
    if code.starts_with('2') {
        Ok(())
    } else {
        eprintln!("open-airplay: dacp {command:?}: the phone answered {:?}", status.lines().next().unwrap_or(""));
        Err("The phone didn't accept the command. Use the controls on the phone.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn remote() -> Remote {
        Remote { dacp_id: "1A2B3C4D5E6F7788".into(), active_remote: "1986535575".into() }
    }

    /// A fake phone remote: answers one request with `reply` and hands back what it received.
    fn fake_phone(reply: &'static str) -> (SocketAddr, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = conn.read(&mut buf).unwrap();
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = conn.write_all(reply.as_bytes());
        });
        (addr, rx)
    }

    #[test]
    fn each_button_asks_the_phone_with_the_session_token() {
        for (command, path) in [(Command::PlayPause, "playpause"), (Command::Next, "nextitem"), (Command::Previous, "previtem")] {
            let (addr, received) = fake_phone("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
            assert_eq!(send(addr, &remote(), command, Duration::from_secs(5)), Ok(()));
            let request = received.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(request.starts_with(&format!("GET /ctrl-int/1/{path} HTTP/1.1\r\n")), "{request}");
            assert!(request.contains("\r\nActive-Remote: 1986535575\r\n"), "{request}");
        }
    }

    #[test]
    fn a_refusal_is_a_plain_sentence() {
        let (addr, _received) = fake_phone("HTTP/1.1 403 Forbidden\r\n\r\n");
        let err = send(addr, &remote(), Command::Next, Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("phone") && !err.contains("403"), "{err}");
    }

    #[test]
    fn a_phone_that_is_gone_is_a_plain_sentence() {
        // Bind and drop: nothing listens on this port any more.
        let addr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let err = send(addr, &remote(), Command::PlayPause, Duration::from_secs(2)).unwrap_err();
        assert_eq!(err, NOT_FOUND);
    }

    #[test]
    fn button_names_from_the_ui() {
        let parse = |s: &str| serde_json::from_str::<Command>(&format!("\"{s}\"")).ok();
        assert_eq!(parse("play_pause"), Some(Command::PlayPause));
        assert_eq!(parse("next"), Some(Command::Next));
        assert_eq!(parse("previous"), Some(Command::Previous));
        assert_eq!(parse("volume_up"), None);
    }

    /// Finds a remote advertised on this machine the way a phone advertises it. Needs multicast on a
    /// network interface, which CI runners don't always have: `cargo test -- --ignored dacp`.
    #[test]
    #[ignore]
    fn finds_the_phone_remote_by_its_id() {
        let mdns = mdns_sd::ServiceDaemon::new().unwrap();
        let ip = crate::netwatch::primary_ip().expect("a network interface");
        let info = mdns_sd::ServiceInfo::new(SERVICE, "iTunes_Ctrl_00AA11BB22CC33DD", "oap-test-phone.local.", ip, 3689, None::<std::collections::HashMap<String, String>>).unwrap();
        mdns.register(info).unwrap();
        let found = find("00AA11BB22CC33DD", Duration::from_secs(10));
        let _ = mdns.shutdown();
        assert_eq!(found, Ok(SocketAddr::new(ip, 3689)));
    }
}
