//! What a launch does: a boot launch (from start at login) stays in the tray with receiving off;
//! a manual launch opens the panel and restores the user's receiving switch.

use crate::prefs::AppPrefs;

/// The argument start at login passes, so a boot launch can tell itself apart from a manual one.
pub const AUTOSTART_ARG: &str = "--autostart";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    pub show_panel: bool,
    pub start_receiving: bool,
}

pub fn is_autostart<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|a| a.as_ref() == AUTOSTART_ARG)
}

pub fn plan<S: AsRef<str>>(args: &[S], prefs: &AppPrefs) -> Launch {
    let boot = is_autostart(args);
    Launch { show_panel: !boot, start_receiving: !boot && prefs.receiving_on() }
}

/// The start-at-login Run entry as Windows reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEntry {
    /// No Run value, e.g. after an uninstall removed it.
    Missing,
    Enabled,
    /// Present but turned off in Task Manager.
    DisabledByUser,
}

/// Whether this launch should (re)write the Run entry so it exists and carries --autostart.
pub fn should_write_run_entry(start_at_login: bool, entry: RunEntry) -> bool {
    start_at_login && entry != RunEntry::DisabledByUser
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(receiving: Option<bool>) -> AppPrefs {
        AppPrefs { receiving, ..AppPrefs::default() }
    }

    #[test]
    fn start_at_login_restores_a_missing_entry_but_respects_task_manager() {
        assert!(should_write_run_entry(true, RunEntry::Missing), "a reinstall gets its entry back");
        assert!(should_write_run_entry(true, RunEntry::Enabled), "an old entry gains --autostart");
        assert!(!should_write_run_entry(true, RunEntry::DisabledByUser), "Task Manager's off stays off");
        for entry in [RunEntry::Missing, RunEntry::Enabled, RunEntry::DisabledByUser] {
            assert!(!should_write_run_entry(false, entry));
        }
    }

    #[test]
    fn a_boot_launch_stays_in_the_tray_with_receiving_off() {
        let args = [r"C:\Program Files\open-airplay\open-airplay.exe", "--autostart"];
        for remembered in [None, Some(true), Some(false)] {
            assert_eq!(plan(&args, &prefs(remembered)), Launch { show_panel: false, start_receiving: false });
        }
    }

    #[test]
    fn a_manual_launch_opens_the_panel_and_restores_the_switch() {
        let args = ["open-airplay.exe"];
        assert_eq!(plan(&args, &prefs(None)), Launch { show_panel: true, start_receiving: true });
        assert_eq!(plan(&args, &prefs(Some(true))), Launch { show_panel: true, start_receiving: true });
        assert_eq!(plan(&args, &prefs(Some(false))), Launch { show_panel: true, start_receiving: false });
    }
}
