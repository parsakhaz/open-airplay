//! The tray panel: where it opens, how big it is, and when it goes away.
//!
//! It behaves like Windows quick settings: it opens anchored to the tray icon, sizes itself to its
//! content, and closes when you click somewhere else. Windows the app itself brings up (the mirror
//! window, the UAC prompt, a settings page it opened) don't close it.

use std::time::{Duration, Instant};

/// Space between the panel and the taskbar (and the screen edges), in logical px.
pub const GAP: f64 = 10.0;
/// The panel's width in logical px; its height follows the content.
pub const WIDTH: f64 = 340.0;
/// A tray click this soon after the panel closed itself is the click that closed it, not a new open.
pub const TRAY_CLICK_AFTER_DISMISS: Duration = Duration::from_millis(500);
/// How long a window the app just opened (video window, network settings) may take focus without
/// closing the panel.
pub const OWN_WINDOW_GRACE: Duration = Duration::from_secs(4);
/// Focus changes right after the panel opens (the Start menu closing, a launcher handing over the
/// foreground) don't close it.
pub const OPEN_GRACE: Duration = Duration::from_millis(800);

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
    fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Where the panel's top-left corner goes, in physical px.
///
/// With the tray icon's rectangle: `gap` away from the taskbar on whichever side the taskbar is (the
/// side of the work area the icon lies outside of), clamped to the work area. Beside a bottom or top
/// taskbar it is centred on the icon; beside a side taskbar its bottom lines up with the icon's, so
/// the footer stays put when the content (and the panel) grows. Without an icon: the bottom-right
/// corner of the work area.
pub fn position(icon: Option<Rect>, width: f64, height: f64, work: Rect, gap: f64) -> (f64, f64) {
    let clamp_x = |x: f64| x.min(work.right() - width - gap).max(work.x + gap);
    let clamp_y = |y: f64| y.min(work.bottom() - height - gap).max(work.y + gap);
    let Some(icon) = icon else {
        return (clamp_x(work.right()), clamp_y(work.bottom()));
    };
    let (cx, cy) = icon.centre();
    if cy >= work.bottom() {
        (clamp_x(cx - width / 2.0), clamp_y(work.bottom()))
    } else if cy <= work.y {
        (clamp_x(cx - width / 2.0), clamp_y(work.y))
    } else if cx <= work.x {
        (clamp_x(work.x), clamp_y(icon.bottom() - height))
    } else if cx >= work.right() {
        (clamp_x(work.right()), clamp_y(icon.bottom() - height))
    } else {
        // An icon inside the work area (the tray overflow flyout): open above it if there's room.
        (clamp_x(cx - width / 2.0), clamp_y(icon.y - height - gap))
    }
}

/// The tallest the panel may be: the work area minus a gap at each end. Longer content scrolls.
pub fn max_height(work: Rect, gap: f64) -> f64 {
    (work.h - 2.0 * gap).max(0.0)
}

/// The window that just became the foreground window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreground {
    /// No window (for example while Windows switches to the secure desktop for a UAC prompt).
    pub none: bool,
    /// It belongs to this app's process (the panel itself).
    pub own_process: bool,
    /// Its window class name.
    pub class: String,
}

/// The engine's mirror window (engine patch 0002).
pub const MIRROR_WINDOW_CLASS: &str = "OpenAirplayMirrorWindow";
/// What Windows puts in the foreground on the normal desktop while a UAC prompt shows.
pub const UAC_DUMMY_CLASS: &str = "$$$Secure UAP Dummy Window Class For Interim Dialog";
/// The UAC prompt itself.
pub const UAC_CONSENT_CLASS: &str = "Credential Dialog Xaml Host";

/// Whether the open panel should close because `fg` took the foreground.
/// `grace` is true while a window the app just opened is allowed to take focus.
pub fn should_dismiss(fg: &Foreground, grace: bool) -> bool {
    if fg.none || fg.own_process || grace {
        return false;
    }
    !matches!(fg.class.as_str(), MIRROR_WINDOW_CLASS | UAC_DUMMY_CLASS | UAC_CONSENT_CLASS)
}

/// What a left click on the tray icon does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayClick {
    Open,
    Close,
    /// The click that already closed the panel (it took the focus from the panel first).
    Nothing,
}

pub fn tray_click(visible: bool, last_dismissed: Option<Instant>, now: Instant) -> TrayClick {
    if visible {
        TrayClick::Close
    } else if last_dismissed.is_some_and(|t| now.saturating_duration_since(t) < TRAY_CLICK_AFTER_DISMISS) {
        TrayClick::Nothing
    } else {
        TrayClick::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1920x1080 screen with a 48 px taskbar at the bottom.
    const WORK: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1032.0 };

    fn icon_at(x: f64, y: f64) -> Option<Rect> {
        Some(Rect { x, y, w: 24.0, h: 24.0 })
    }

    #[test]
    fn opens_centred_on_the_icon_just_above_a_bottom_taskbar() {
        let (x, y) = position(icon_at(1500.0, 1044.0), 340.0, 400.0, WORK, 10.0);
        assert_eq!((x, y), (1512.0 - 170.0, 1032.0 - 400.0 - 10.0));
    }

    #[test]
    fn stays_inside_the_work_area_near_the_screen_edge() {
        let (x, _) = position(icon_at(1890.0, 1044.0), 340.0, 400.0, WORK, 10.0);
        assert_eq!(x, 1920.0 - 340.0 - 10.0);
    }

    #[test]
    fn opens_below_a_top_taskbar() {
        let work = Rect { x: 0.0, y: 48.0, w: 1920.0, h: 1032.0 };
        let (x, y) = position(icon_at(1500.0, 12.0), 340.0, 400.0, work, 10.0);
        assert_eq!((x, y), (1342.0, 58.0));
    }

    #[test]
    fn opens_beside_a_side_taskbar() {
        let left = Rect { x: 60.0, y: 0.0, w: 1860.0, h: 1080.0 };
        assert_eq!(position(icon_at(18.0, 500.0), 340.0, 400.0, left, 10.0), (70.0, 524.0 - 400.0));
        let right = Rect { x: 0.0, y: 0.0, w: 1860.0, h: 1080.0 };
        assert_eq!(position(icon_at(1878.0, 900.0), 340.0, 400.0, right, 10.0), (1860.0 - 340.0 - 10.0, 924.0 - 400.0));
    }

    #[test]
    fn beside_a_side_taskbar_the_footer_stays_put_as_the_panel_grows() {
        let left = Rect { x: 60.0, y: 0.0, w: 1860.0, h: 1080.0 };
        let bottom = |h: f64| position(icon_at(18.0, 700.0), 340.0, h, left, 10.0).1 + h;
        assert_eq!(bottom(300.0), bottom(520.0));
    }

    #[test]
    fn works_on_a_second_monitor_at_150_percent() {
        // Monitor to the right of the primary, scaled 150 %: everything is in physical px.
        let work = Rect { x: 1920.0, y: 0.0, w: 2560.0, h: 1368.0 };
        let (x, y) = position(icon_at(4000.0, 1380.0), 510.0, 600.0, work, 15.0);
        assert_eq!((x, y), (4012.0 - 255.0, 1368.0 - 600.0 - 15.0));
    }

    #[test]
    fn without_an_icon_it_opens_bottom_right() {
        assert_eq!(position(None, 340.0, 400.0, WORK, 10.0), (1920.0 - 350.0, 1032.0 - 410.0));
    }

    #[test]
    fn content_taller_than_the_screen_is_capped_and_kept_on_screen() {
        assert_eq!(max_height(WORK, 10.0), 1012.0);
        let (_, y) = position(icon_at(1500.0, 1044.0), 340.0, 1012.0, WORK, 10.0);
        assert_eq!(y, 10.0);
    }

    fn fg(class: &str) -> Foreground {
        Foreground { class: class.into(), ..Default::default() }
    }

    #[test]
    fn clicking_the_desktop_another_app_or_the_taskbar_closes_it() {
        for class in ["Progman", "WorkerW", "Notepad", "Shell_TrayWnd", "Chrome_WidgetWin_1"] {
            assert!(should_dismiss(&fg(class), false), "{class}");
        }
    }

    #[test]
    fn the_apps_own_windows_and_uac_keep_it_open() {
        assert!(!should_dismiss(&Foreground { own_process: true, ..fg("Tauri Window") }, false));
        assert!(!should_dismiss(&fg(MIRROR_WINDOW_CLASS), false));
        assert!(!should_dismiss(&fg(UAC_DUMMY_CLASS), false));
        assert!(!should_dismiss(&fg(UAC_CONSENT_CLASS), false));
        assert!(!should_dismiss(&Foreground { none: true, ..Default::default() }, false));
        // The network settings page the app just opened.
        assert!(!should_dismiss(&fg("ApplicationFrameWindow"), true));
    }

    #[test]
    fn a_tray_click_toggles_and_the_closing_click_does_not_reopen() {
        let now = Instant::now();
        assert_eq!(tray_click(true, None, now), TrayClick::Close);
        assert_eq!(tray_click(false, None, now), TrayClick::Open);
        // The click on the tray icon took the focus first, which already closed the panel.
        assert_eq!(tray_click(false, Some(now - Duration::from_millis(80)), now), TrayClick::Nothing);
        assert_eq!(tray_click(false, Some(now - Duration::from_secs(2)), now), TrayClick::Open);
    }
}
