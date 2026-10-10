//! Network doctor (lite): finds the usual reasons an iPhone can't see this PC and explains them in plain
//! language. It never changes anything by itself; fixes run only when the user clicks.

use serde::{Deserialize, Serialize};

/// What the probe saw. Collected by `probe()` on Windows; tests build it by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The network the primary interface is on, e.g. "Foundations".
    pub network_name: Option<String>,
    /// "Public", "Private" or "DomainAuthenticated".
    pub network_category: Option<String>,
    /// Inbound firewall rules that name the engine's exe.
    pub engine_rules: Vec<FirewallRule>,
    /// Other AirPlay receivers running on this PC (process names).
    pub other_receivers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FirewallRule {
    pub enabled: bool,
    /// "Allow" or "Block".
    pub action: String,
    /// Profiles the rule applies to, e.g. "Any" or "Private, Public".
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// Windows treats this network as untrusted and may block discovery.
    PublicNetwork { network: String },
    /// No enabled firewall rule lets iPhones reach the receiver on this network (or one blocks it).
    FirewallBlocks,
    /// The network looks like a guest network; those often stop devices from seeing each other
    /// (AP / client isolation). Can't be fixed from this PC, only explained.
    GuestNetwork { network: String },
    /// Another AirPlay receiver is running and may take the connection.
    OtherReceiver { name: String },
}

pub fn diagnose(s: &Snapshot) -> Vec<Finding> {
    let mut findings = Vec::new();
    let category = s.network_category.as_deref();
    if category == Some("Public") {
        findings.push(Finding::PublicNetwork { network: s.network_name.clone().unwrap_or_else(|| "this network".into()) });
    }
    // Firewall profile names: Domain / Private / Public, "Any", or a comma-separated mix.
    let profile = match category {
        Some("DomainAuthenticated") => Some("Domain"),
        other => other,
    };
    let applies = |r: &&FirewallRule| r.enabled && (r.profile == "Any" || profile.is_none_or(|p| r.profile.contains(p)));
    let blocked = s.engine_rules.iter().filter(applies).any(|r| r.action == "Block");
    let allowed = s.engine_rules.iter().filter(applies).any(|r| r.action == "Allow");
    if blocked || !allowed {
        findings.push(Finding::FirewallBlocks);
    }
    if let Some(name) = s.network_name.as_deref().filter(|n| n.to_lowercase().contains("guest")) {
        findings.push(Finding::GuestNetwork { network: name.to_string() });
    }
    findings.extend(s.other_receivers.iter().map(|name| Finding::OtherReceiver { name: name.clone() }));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(enabled: bool, action: &str, profile: &str) -> FirewallRule {
        FirewallRule { enabled, action: action.into(), profile: profile.into() }
    }

    fn healthy() -> Snapshot {
        Snapshot {
            network_name: Some("Home".into()),
            network_category: Some("Private".into()),
            engine_rules: vec![rule(true, "Allow", "Private")],
            other_receivers: vec![],
        }
    }

    #[test]
    fn a_healthy_setup_has_no_findings() {
        assert_eq!(diagnose(&healthy()), vec![]);
    }

    #[test]
    fn a_public_network_is_reported_by_name() {
        let s = Snapshot { network_category: Some("Public".into()), engine_rules: vec![rule(true, "Allow", "Any")], ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::PublicNetwork { network: "Home".into() }]);
    }

    #[test]
    fn no_rule_for_the_engine_means_the_firewall_blocks_it() {
        let s = Snapshot { engine_rules: vec![], ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::FirewallBlocks]);
    }

    #[test]
    fn an_allow_rule_for_another_profile_does_not_count() {
        // Allowed on Private only, but the PC is on a Public network.
        let s = Snapshot { network_category: Some("Public".into()), engine_rules: vec![rule(true, "Allow", "Private")], ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::PublicNetwork { network: "Home".into() }, Finding::FirewallBlocks]);
    }

    #[test]
    fn a_disabled_allow_rule_does_not_count() {
        let s = Snapshot { engine_rules: vec![rule(false, "Allow", "Any")], ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::FirewallBlocks]);
    }

    #[test]
    fn a_block_rule_wins_over_an_allow_rule() {
        // Clicking Cancel on Windows' firewall prompt creates a Block rule for the exe.
        let s = Snapshot { engine_rules: vec![rule(true, "Allow", "Any"), rule(true, "Block", "Private, Public")], ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::FirewallBlocks]);
    }

    #[test]
    fn a_guest_network_gets_an_isolation_hint() {
        let s = Snapshot { network_name: Some("Foundations Guest".into()), ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::GuestNetwork { network: "Foundations Guest".into() }]);
    }

    #[test]
    fn other_receivers_are_named() {
        let s = Snapshot { other_receivers: vec!["AirServer".into()], ..healthy() };
        assert_eq!(diagnose(&s), vec![Finding::OtherReceiver { name: "AirServer".into() }]);
    }

    #[test]
    fn an_unknown_network_category_is_not_guessed() {
        let s = Snapshot { network_category: None, network_name: None, ..healthy() };
        assert_eq!(diagnose(&s), vec![]);
    }
}

/// Known AirPlay receivers that compete for iPhones (matched by process name).
const OTHER_RECEIVERS: &[&str] =
    &["AirServer", "LonelyScreen", "Reflector", "5KPlayer", "AirPlayServer", "uxplay", "uxplay-windows", "AirParrot"];

/// PowerShell wants single quotes doubled inside a single-quoted string.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The probe script. `engine` is the receiver's exe, `ip` the primary address, `own_pid` our engine (not "another" receiver).
pub fn probe_script(engine: &str, ip: Option<&str>, own_pid: Option<u32>) -> String {
    let names = OTHER_RECEIVERS.iter().map(|n| ps_quote(n)).collect::<Vec<_>>().join(",");
    format!(
        r#"$ErrorActionPreference = 'SilentlyContinue'
$out = [ordered]@{{ network_name = $null; network_category = $null; engine_rules = @(); other_receivers = @() }}
$ip = {ip}
if ($ip) {{
  $alias = (Get-NetIPAddress -IPAddress $ip).InterfaceAlias | Select-Object -First 1
  if ($alias) {{
    $p = Get-NetConnectionProfile -InterfaceAlias $alias | Select-Object -First 1
    if ($p) {{ $out.network_name = [string]$p.Name; $out.network_category = [string]$p.NetworkCategory }}
  }}
}}
foreach ($f in @(Get-NetFirewallApplicationFilter -Program {engine})) {{
  $r = $f | Get-NetFirewallRule
  if ($r -and [string]$r.Direction -eq 'Inbound') {{
    $out.engine_rules += [ordered]@{{ enabled = ([string]$r.Enabled -eq 'True'); action = [string]$r.Action; profile = [string]$r.Profile }}
  }}
}}
$out.other_receivers = @(Get-Process -Name {names} | Where-Object {{ $_.Id -ne {pid} }} | ForEach-Object {{ $_.ProcessName }} | Sort-Object -Unique)
$out | ConvertTo-Json -Compress -Depth 4"#,
        ip = ip.map(ps_quote).unwrap_or_else(|| "$null".into()),
        engine = ps_quote(engine),
        names = names,
        pid = own_pid.unwrap_or(0),
    )
}

/// Runs the probe (Windows only). Returns None if PowerShell isn't available or the output can't be read.
pub fn probe(engine: &str, ip: Option<&str>, own_pid: Option<u32>) -> Option<Snapshot> {
    if !cfg!(windows) {
        return None;
    }
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &probe_script(engine, ip, own_pid)]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    serde_json::from_slice(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).ok()
}

/// The elevated command for the "Allow through firewall" fix: drop Block rules for the engine, add one Allow rule.
pub fn firewall_fix_script(engine: &str) -> String {
    format!(
        "Get-NetFirewallApplicationFilter -Program {e} | Get-NetFirewallRule | Where-Object {{ $_.Action -eq 'Block' }} | Remove-NetFirewallRule; \
         Remove-NetFirewallRule -DisplayName 'open-airplay receiver' -ErrorAction SilentlyContinue; \
         New-NetFirewallRule -DisplayName 'open-airplay receiver' -Direction Inbound -Program {e} -Action Allow -Profile Any | Out-Null",
        e = ps_quote(engine)
    )
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    #[test]
    fn paths_with_quotes_are_escaped_for_powershell() {
        assert_eq!(ps_quote(r"C:\Users\O'Brien\uxplay.exe"), r"'C:\Users\O''Brien\uxplay.exe'");
        assert!(probe_script(r"C:\Users\O'Brien\uxplay.exe", None, None).contains(r"-Program 'C:\Users\O''Brien\uxplay.exe'"));
    }

    #[test]
    fn probe_output_parses_into_a_snapshot() {
        let json = r#"{"network_name":"Foundations","network_category":"Public","engine_rules":[{"enabled":true,"action":"Allow","profile":"Private, Public"}],"other_receivers":[]}"#;
        let s: Snapshot = serde_json::from_str(json).unwrap();
        assert_eq!(s.network_category.as_deref(), Some("Public"));
        assert_eq!(diagnose(&s), vec![Finding::PublicNetwork { network: "Foundations".into() }]);
    }

    #[test]
    #[cfg(windows)]
    fn the_real_probe_runs_on_this_machine() {
        // Smoke test against the live system: a nonexistent exe has no firewall rules.
        let s = probe(r"C:\definitely\not\here\uxplay.exe", None, None).expect("probe output");
        assert!(s.engine_rules.is_empty());
    }
}
