//! Pure decisions of the window module (unit-tested).

pub const DEVMODEA_SIZE: usize = 156;
pub const SPI_SETSCREENSAVERRUNNING: u32 = 0x61;
const WS_EX_TOPMOST: u32 = 0x8;
const HWND_TOP: isize = 0;
const HWND_TOPMOST: isize = -1;

/// Window classes of the game's main window (Em4.exe, or vBase71 when it
/// creates its own).
pub fn is_game_class(class: &str) -> bool {
    class.eq_ignore_ascii_case("BLWNDCLS") || class.eq_ignore_ascii_case("VVideoClass")
}

pub fn strip_topmost(ex_style: u32) -> u32 {
    ex_style & !WS_EX_TOPMOST
}

/// `HWND_TOPMOST` becomes `HWND_TOP`: in front while active, but other
/// windows can come in front after Alt+Tab.
pub fn insert_after(after: isize) -> isize {
    if after == HWND_TOPMOST {
        HWND_TOP
    } else {
        after
    }
}

/// SW_MINIMIZE, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE, SW_FORCEMINIMIZE.
pub fn is_minimize(cmd: i32) -> bool {
    matches!(cmd, 2 | 6 | 7 | 11)
}

/// dmPelsWidth/dmPelsHeight of a DEVMODEA.
pub fn devmode_size(dm: &[u8]) -> (u32, u32) {
    let rd = |o: usize| u32::from_le_bytes(dm[o..o + 4].try_into().unwrap());
    (rd(108), rd(112))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Cds {
    /// Pretend success, leave the desktop mode alone.
    Skip,
    /// Really change the mode (different resolution requested).
    PassThrough,
}

/// `requested` = None means "restore the registry mode" (CDS(NULL, 0)); that
/// is only skipped when we skipped the switch before (`faked`).
pub fn cds_decision(requested: Option<(u32, u32)>, desktop: (u32, u32), faked: bool) -> Cds {
    match requested {
        None if faked => Cds::Skip,
        None => Cds::PassThrough,
        Some(r) if desktop.0 != 0 && r == desktop => Cds::Skip,
        Some(_) => Cds::PassThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert!(is_game_class("BLWNDCLS"));
        assert!(is_game_class("VVideoClass"));
        assert!(!is_game_class("Edit"));
    }

    #[test]
    fn topmost_removed() {
        // Em4.exe creates its window with ex-style 8 (WS_EX_TOPMOST).
        assert_eq!(strip_topmost(0x8), 0);
        assert_eq!(strip_topmost(0x8 | 0x100), 0x100);
        assert_eq!(insert_after(-1), 0);
        assert_eq!(insert_after(1), 1); // HWND_BOTTOM unchanged
        assert_eq!(insert_after(-2), -2); // HWND_NOTOPMOST unchanged
    }

    #[test]
    fn minimize_commands() {
        assert!(is_minimize(6)); // what Em4.exe uses at 0x5CF40B
        assert!(is_minimize(2) && is_minimize(7) && is_minimize(11));
        assert!(!is_minimize(5) && !is_minimize(1) && !is_minimize(9));
    }

    #[test]
    fn devmode_fields() {
        let mut dm = [0u8; DEVMODEA_SIZE];
        dm[108..112].copy_from_slice(&1920u32.to_le_bytes());
        dm[112..116].copy_from_slice(&1080u32.to_le_bytes());
        assert_eq!(devmode_size(&dm), (1920, 1080));
    }

    #[test]
    fn cds() {
        let d = (1920, 1080);
        assert_eq!(cds_decision(Some(d), d, false), Cds::Skip);
        assert_eq!(cds_decision(None, d, true), Cds::Skip);
        // Nothing was skipped (windowed / exclusive): restoring is real.
        assert_eq!(cds_decision(None, d, false), Cds::PassThrough);
        assert_eq!(cds_decision(Some((1024, 768)), d, false), Cds::PassThrough);
        // Unknown desktop: never pretend.
        assert_eq!(cds_decision(Some(d), (0, 0), false), Cds::PassThrough);
    }

    #[test]
    fn devmode_layout_matches_windows() {
        // Real EnumDisplaySettingsA fills dmPelsWidth at offset 108.
        #[link(name = "user32")]
        extern "system" {
            fn EnumDisplaySettingsA(device: *const u8, mode: u32, dm: *mut u8) -> i32;
            fn GetSystemMetrics(i: i32) -> i32;
        }
        let mut dm = [0u8; DEVMODEA_SIZE];
        dm[36..38].copy_from_slice(&(DEVMODEA_SIZE as u16).to_le_bytes());
        let ok = unsafe { EnumDisplaySettingsA(core::ptr::null(), 0xFFFF_FFFF, dm.as_mut_ptr()) };
        assert_ne!(ok, 0);
        let (w, h) = devmode_size(&dm);
        assert!(w >= 640 && h >= 480, "{w}x{h}");
        let _ = unsafe { GetSystemMetrics(0) };
    }
}
