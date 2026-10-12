//! Devices the user added by IP address or hostname, for networks where discovery is blocked.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A checked address: a lowercase hostname or an IP address, and the port if one was typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Address {
    pub host: String,
    pub port: Option<u16>,
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.port, self.host.contains(':')) {
            (None, _) => f.write_str(&self.host),
            (Some(port), true) => write!(f, "[{}]:{port}", self.host),
            (Some(port), false) => write!(f, "{}:{port}", self.host),
        }
    }
}

/// Why an address was refused. `Display` is the sentence shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    NotJustAddress,
    BadIp(String),
    Spaces,
    BadName(String),
    BadPort,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::Empty => f.write_str("Type the device's IP address or name."),
            AddressError::NotJustAddress => f.write_str("Type just the address, like 192.168.1.20."),
            AddressError::BadIp(ip) => write!(f, "{ip} isn't a valid IP address."),
            AddressError::Spaces => f.write_str("A device name can't contain spaces."),
            AddressError::BadName(name) => write!(f, "{name} isn't a valid device name."),
            AddressError::BadPort => f.write_str("The port must be a number from 1 to 65535."),
        }
    }
}

/// Checks what the user typed: `192.168.1.20`, `tv.local:7100`, `fe80::1` or `[fe80::1]:7000`.
pub fn parse_address(input: &str) -> Result<Address, AddressError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AddressError::Empty);
    }
    if input.contains("://") || input.contains('/') {
        return Err(AddressError::NotJustAddress);
    }
    if let Some(rest) = input.strip_prefix('[') {
        let (ip, after) = rest.split_once(']').ok_or_else(|| AddressError::BadIp(rest.to_string()))?;
        let ip: Ipv6Addr = ip.parse().map_err(|_| AddressError::BadIp(ip.to_string()))?;
        let port = match after {
            "" => None,
            _ => Some(parse_port(after.strip_prefix(':').ok_or(AddressError::BadPort)?)?),
        };
        return Ok(Address { host: ip.to_string(), port });
    }
    if input.matches(':').count() > 1 {
        let ip: Ipv6Addr = input.parse().map_err(|_| AddressError::BadIp(input.to_string()))?;
        return Ok(Address { host: ip.to_string(), port: None });
    }
    let (host, port) = match input.split_once(':') {
        Some((host, port)) => (host, Some(parse_port(port)?)),
        None => (input, None),
    };
    if host.chars().any(char::is_whitespace) {
        return Err(AddressError::Spaces);
    }
    if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        let ip: Ipv4Addr = host.parse().map_err(|_| AddressError::BadIp(host.to_string()))?;
        return Ok(Address { host: ip.to_string(), port });
    }
    if !is_hostname(host) {
        return Err(AddressError::BadName(host.to_string()));
    }
    Ok(Address { host: host.trim_end_matches('.').to_ascii_lowercase(), port })
}

/// The AirPlay port, used when the user typed none (the mirror sender's default too).
pub const AIRPLAY_PORT: u16 = 7000;

/// Whether something answers at the address: the name resolves and a TCP connection opens.
/// Blocks for up to `timeout` per resolved address. The error is a sentence for the user.
pub fn check_reachable(addr: &Address, timeout: Duration) -> Result<(), String> {
    let port = addr.port.unwrap_or(AIRPLAY_PORT);
    let not_found = || format!("Couldn't find {} on this network. Check the name, or use its IP address.", addr.host);
    let sockets: Vec<_> = (addr.host.as_str(), port).to_socket_addrs().map_err(|_| not_found())?.collect();
    if sockets.is_empty() {
        return Err(not_found());
    }
    if sockets.iter().any(|s| TcpStream::connect_timeout(s, timeout).is_ok()) {
        return Ok(());
    }
    Err(format!("Nothing answered at {}. Check the address and that the device is on.", addr.host))
}

/// A remembered device, as saved in prefs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddedDevice {
    /// As `parse_address` normalised it; the key, and the `target` the mirror sender is given.
    pub host: String,
    /// The mirror port, if the user typed one (the sender's default otherwise).
    pub port: Option<u16>,
    /// The device's own name, once it has told us.
    pub name: Option<String>,
    /// The cast sidecar's identifier, if the device answered a cast scan.
    pub cast_id: Option<String>,
}

/// Adds a device, or updates the one already saved at that address (keeping its place and a known name).
pub fn remember(list: &mut Vec<AddedDevice>, device: AddedDevice) {
    match list.iter_mut().find(|d| d.host == device.host) {
        Some(saved) => {
            let name = device.name.clone().or_else(|| saved.name.take());
            let cast_id = device.cast_id.clone().or_else(|| saved.cast_id.take());
            *saved = AddedDevice { name, cast_id, ..device };
        }
        None => list.push(device),
    }
}

/// Forgets the device at `host`. Returns whether there was one.
pub fn forget(list: &mut Vec<AddedDevice>, host: &str) -> bool {
    let before = list.len();
    list.retain(|d| d.host != host);
    list.len() != before
}

/// `parse_address`, with the port from the form's own Port field (which wins) when there is one.
pub fn parse_address_with_port(input: &str, port: Option<u16>) -> Result<Address, AddressError> {
    let addr = parse_address(input)?;
    match port {
        Some(0) => Err(AddressError::BadPort),
        Some(port) => Ok(Address { port: Some(port), ..addr }),
        None => Ok(addr),
    }
}

fn parse_port(text: &str) -> Result<u16, AddressError> {
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(AddressError::BadPort),
    }
}

/// RFC 1123 hostname: dot-separated labels of letters, digits and inner hyphens.
fn is_hostname(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> (String, Option<u16>) {
        let a = parse_address(input).unwrap_or_else(|e| panic!("{input:?} was refused: {e}"));
        (a.host, a.port)
    }

    #[test]
    fn accepts_ip_addresses_and_hostnames_with_or_without_a_port() {
        assert_eq!(ok("192.168.1.20"), ("192.168.1.20".into(), None));
        assert_eq!(ok("  192.168.1.20:7100 "), ("192.168.1.20".into(), Some(7100)));
        assert_eq!(ok("Living-Room.local"), ("living-room.local".into(), None));
        assert_eq!(ok("tv:7000"), ("tv".into(), Some(7000)));
        assert_eq!(ok("fe80::1"), ("fe80::1".into(), None));
        assert_eq!(ok("[fe80::1]:7000"), ("fe80::1".into(), Some(7000)));
    }

    #[test]
    fn refuses_bad_input_with_a_plain_sentence() {
        let refused = |input: &str| parse_address(input).unwrap_err().to_string();
        assert_eq!(refused("   "), "Type the device's IP address or name.");
        assert_eq!(refused("http://192.168.1.20"), "Type just the address, like 192.168.1.20.");
        assert_eq!(refused("192.168.1.20/tv"), "Type just the address, like 192.168.1.20.");
        assert_eq!(refused("192.168.1.300"), "192.168.1.300 isn't a valid IP address.");
        assert_eq!(refused("192.168.1"), "192.168.1 isn't a valid IP address.");
        assert_eq!(refused("living room"), "A device name can't contain spaces.");
        assert_eq!(refused("tv_den!"), "tv_den! isn't a valid device name.");
        assert_eq!(refused("-tv"), "-tv isn't a valid device name.");
        assert_eq!(refused("tv:0"), "The port must be a number from 1 to 65535.");
        assert_eq!(refused("tv:70000"), "The port must be a number from 1 to 65535.");
        assert_eq!(refused("tv:abc"), "The port must be a number from 1 to 65535.");
        assert_eq!(refused("[fe80::1"), "fe80::1 isn't a valid IP address.");
    }

    fn device(host: &str, name: Option<&str>) -> AddedDevice {
        AddedDevice { host: host.into(), port: None, name: name.map(str::to_string), cast_id: None }
    }

    #[test]
    fn adding_the_same_address_again_updates_it_in_place() {
        let mut list = vec![device("192.168.1.20", None), device("tv", None)];
        remember(&mut list, AddedDevice { port: Some(7100), ..device("192.168.1.20", Some("Den")) });
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], AddedDevice { port: Some(7100), ..device("192.168.1.20", Some("Den")) });
        remember(&mut list, device("office.local", None));
        assert_eq!(list.iter().map(|d| d.host.as_str()).collect::<Vec<_>>(), ["192.168.1.20", "tv", "office.local"]);
    }

    #[test]
    fn a_name_learned_once_is_kept_when_a_later_add_finds_none() {
        let mut list = vec![device("tv", Some("Den"))];
        remember(&mut list, device("tv", None));
        assert_eq!(list[0].name.as_deref(), Some("Den"));
    }

    #[test]
    fn removing_forgets_only_that_address() {
        let mut list = vec![device("192.168.1.20", None), device("tv", None)];
        assert!(forget(&mut list, "tv"));
        assert!(!forget(&mut list, "tv"), "already gone");
        assert_eq!(list, [device("192.168.1.20", None)]);
    }

    #[test]
    fn a_device_that_answers_on_its_port_is_reachable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr = parse_address(&format!("127.0.0.1:{port}")).unwrap();
        assert_eq!(check_reachable(&addr, Duration::from_secs(2)), Ok(()));
    }

    #[test]
    fn nothing_listening_and_unknown_names_get_plain_sentences() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let closed = parse_address(&format!("127.0.0.1:{free}")).unwrap();
        assert_eq!(
            check_reachable(&closed, Duration::from_secs(2)).unwrap_err(),
            "Nothing answered at 127.0.0.1. Check the address and that the device is on."
        );
        let unknown = parse_address("no-such-device.invalid").unwrap();
        assert_eq!(
            check_reachable(&unknown, Duration::from_secs(2)).unwrap_err(),
            "Couldn't find no-such-device.invalid on this network. Check the name, or use its IP address."
        );
    }

    #[test]
    fn a_port_from_its_own_field_wins_over_one_typed_in_the_address() {
        assert_eq!(parse_address_with_port("tv:7100", Some(7200)).unwrap().port, Some(7200));
        assert_eq!(parse_address_with_port("tv:7100", None).unwrap().port, Some(7100));
        assert_eq!(parse_address_with_port("tv", Some(0)).unwrap_err(), AddressError::BadPort);
    }

    #[test]
    fn the_target_is_what_the_sender_is_given() {
        assert_eq!(parse_address("192.168.1.20:7100").unwrap().to_string(), "192.168.1.20:7100");
        assert_eq!(parse_address("[fe80::1]:7000").unwrap().to_string(), "[fe80::1]:7000");
        assert_eq!(parse_address("tv").unwrap().to_string(), "tv");
    }
}
