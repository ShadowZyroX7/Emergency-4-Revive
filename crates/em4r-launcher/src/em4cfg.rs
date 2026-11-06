//! Reading and carefully editing the game's own `em4.cfg`.
//!
//! Format: `<var name="r_xres" value="1920" />` lines inside `<config>`.
//! Only the `value` attributes of known variables are replaced; everything
//! else stays byte-for-byte identical. A backup is made before the first
//! change, and the result is re-parsed before it replaces the original.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Em4Cfg {
    path: PathBuf,
    text: String,
    encoding: Encoding,
    pub vars: BTreeMap<String, String>,
}

/// EM4 itself writes em4.cfg as UTF-16 LE with BOM when it saves; the
/// shipped file is plain ASCII. We write back in the encoding we found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf16Le,
    Utf8Bom,
    Plain,
}

pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    match bytes {
        [0xFF, 0xFE, ..] => (crate::mods::decode_text(bytes), Encoding::Utf16Le),
        [0xEF, 0xBB, 0xBF, ..] => (crate::mods::decode_text(bytes), Encoding::Utf8Bom),
        _ => (crate::mods::decode_text(bytes), Encoding::Plain),
    }
}

pub fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }
        Encoding::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            v
        }
        Encoding::Plain => text.as_bytes().to_vec(),
    }
}

/// All `name -> value` pairs.
pub fn parse_vars(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = text;
    while let Some(i) = rest.find("<var ") {
        rest = &rest[i + 5..];
        let end = rest.find("/>").or_else(|| rest.find('>')).unwrap_or(rest.len());
        let tag = &rest[..end];
        if let (Some(n), Some(v)) = (attr(tag, "name"), attr(tag, "value")) {
            out.insert(n.to_string(), v.to_string());
        }
        rest = &rest[end..];
    }
    out
}

fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}=\"");
    let start = tag.find(&pat)? + pat.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

/// Replace the value of `name` in the raw text. None if the var is missing.
pub fn replace_value(text: &str, name: &str, new_value: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    let pos = text.find(&needle)?;
    let line_end = text[pos..].find('>').map(|e| pos + e)?;
    let vpat = "value=\"";
    let vstart = text[pos..line_end].find(vpat)? + pos + vpat.len();
    let vend = text[vstart..].find('"')? + vstart;
    let mut out = String::with_capacity(text.len() + new_value.len());
    out.push_str(&text[..vstart]);
    out.push_str(new_value);
    out.push_str(&text[vend..]);
    Some(out)
}

impl Em4Cfg {
    pub fn load(game_dir: &Path) -> Result<Self, String> {
        let path = game_dir.join("em4.cfg");
        let bytes = std::fs::read(&path).map_err(|e| format!("em4.cfg nicht lesbar: {e}"))?;
        let (text, encoding) = decode(&bytes);
        let vars = parse_vars(&text);
        if vars.is_empty() {
            return Err("em4.cfg enthält keine Einstellungen".into());
        }
        Ok(Self { path, text, encoding, vars })
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    pub fn get_bool(&self, name: &str) -> bool {
        self.get(name).is_some_and(|v| v.trim() != "0")
    }

    /// Write the given changes (only vars that already exist).
    pub fn save(&mut self, changes: &[(&str, String)], backup_dir: &Path) -> Result<usize, String> {
        let mut text = self.text.clone();
        let mut changed = 0;
        for (name, value) in changes {
            if self.vars.get(*name) == Some(value) {
                continue;
            }
            text = replace_value(&text, name, value).ok_or_else(|| format!("{name} fehlt in em4.cfg"))?;
            changed += 1;
        }
        if changed == 0 {
            return Ok(0);
        }
        // Validate: same set of variables, the new values in place.
        let check = parse_vars(&text);
        if check.len() != self.vars.len() || changes.iter().any(|(n, v)| check.get(*n) != Some(v)) {
            return Err("Prüfung nach dem Ändern fehlgeschlagen – nichts gespeichert".into());
        }
        // One-time backup of the state before the launcher ever changed it.
        let _ = std::fs::create_dir_all(backup_dir);
        let backup = backup_dir.join("em4.cfg.vor_launcher");
        if !backup.exists() {
            std::fs::write(&backup, encode(&self.text, self.encoding))
                .map_err(|e| format!("Sicherung fehlgeschlagen: {e}"))?;
        }
        std::fs::write(&self.path, encode(&text, self.encoding)).map_err(|e| e.to_string())?;
        self.text = text;
        self.vars = check;
        Ok(changed)
    }
}

/// Display modes of the primary monitor (32-bit, >= 1024x768), unique,
/// sorted ascending.
pub fn display_modes() -> Vec<(u32, u32)> {
    let mut modes = Vec::new();
    let mut i = 0u32;
    loop {
        let mut dm = DevModeW::new();
        // SAFETY: valid struct with dmSize set; null device = primary.
        if unsafe { EnumDisplaySettingsW(core::ptr::null(), i, &mut dm) } == 0 {
            break;
        }
        if dm.bits_per_pel == 32 && dm.pels_width >= 1024 && dm.pels_height >= 720 {
            modes.push((dm.pels_width, dm.pels_height));
        }
        i += 1;
        if i > 2000 {
            break;
        }
    }
    modes.sort();
    modes.dedup();
    modes
}

/// Current resolution of the primary monitor.
pub fn desktop_mode() -> Option<(u32, u32)> {
    let mut dm = DevModeW::new();
    // SAFETY: valid struct with dmSize set; ENUM_CURRENT_SETTINGS.
    let ok = unsafe { EnumDisplaySettingsW(core::ptr::null(), 0xFFFF_FFFF, &mut dm) } != 0;
    (ok && dm.pels_width > 0).then_some((dm.pels_width, dm.pels_height))
}

/// `DEVMODEW` (220 bytes); only the fields we read are named.
#[repr(C)]
struct DevModeW {
    device_name: [u16; 32],
    spec_version: u16,
    driver_version: u16,
    size: u16,
    driver_extra: u16,
    fields: u32,
    union_part: [u8; 16],
    color: i16,
    duplex: i16,
    y_resolution: i16,
    tt_option: i16,
    collate: i16,
    form_name: [u16; 32],
    log_pixels: u16,
    bits_per_pel: u32,
    pels_width: u32,
    pels_height: u32,
    display_flags: u32,
    display_frequency: u32,
    icm: [u32; 8],
}

impl DevModeW {
    fn new() -> Self {
        // SAFETY: plain-old-data struct; all-zero is valid.
        let mut d: Self = unsafe { core::mem::zeroed() };
        d.size = core::mem::size_of::<Self>() as u16;
        d
    }
}

#[link(name = "user32")]
extern "system" {
    fn EnumDisplaySettingsW(device: *const u16, mode: u32, dm: *mut DevModeW) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<?xml version=\"1.0\" ?>\r\n<config>\r\n    <var name=\"e4_bloom\" value=\"1\" />\r\n    <var name=\"r_xres\" value=\"1920\" />\r\n    <var name=\"r_yres\" value=\"1080\" />\r\n</config>\r\n";

    #[test]
    fn devmode_has_win32_size() {
        assert_eq!(core::mem::size_of::<DevModeW>(), 220);
    }

    #[test]
    fn parse_and_replace_keep_everything_else() {
        let v = parse_vars(SAMPLE);
        assert_eq!(v["r_xres"], "1920");
        assert_eq!(v.len(), 3);
        let t = replace_value(SAMPLE, "r_xres", "2560").unwrap();
        assert_eq!(t, SAMPLE.replace("value=\"1920\"", "value=\"2560\""));
        assert!(replace_value(SAMPLE, "nope", "1").is_none());
        // "r_yres" must not be matched when replacing "r_xres" etc.
        let t2 = replace_value(SAMPLE, "e4_bloom", "0").unwrap();
        assert!(t2.contains("name=\"r_xres\" value=\"1920\""));
    }

    #[test]
    fn save_backs_up_once_and_validates() {
        let dir = std::env::temp_dir().join(format!("em4r_em4cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("em4.cfg"), SAMPLE).unwrap();
        let backup = dir.join("backup");
        let mut c = Em4Cfg::load(&dir).unwrap();
        assert_eq!(c.save(&[("r_xres", "1280".into()), ("r_yres", "720".into())], &backup).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(backup.join("em4.cfg.vor_launcher")).unwrap(), SAMPLE);
        let c2 = Em4Cfg::load(&dir).unwrap();
        assert_eq!(c2.get("r_xres"), Some("1280"));
        // Second change keeps the *original* backup.
        let mut c3 = Em4Cfg::load(&dir).unwrap();
        c3.save(&[("e4_bloom", "0".into())], &backup).unwrap();
        assert_eq!(std::fs::read_to_string(backup.join("em4.cfg.vor_launcher")).unwrap(), SAMPLE);
        // Unknown variables are rejected without writing.
        assert!(c3.save(&[("gibtsnicht", "1".into())], &backup).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn utf16_file_roundtrip_keeps_encoding() {
        let dir = std::env::temp_dir().join(format!("em4r_em4cfg16_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = encode(SAMPLE, Encoding::Utf16Le);
        std::fs::write(dir.join("em4.cfg"), &original).unwrap();
        let mut c = Em4Cfg::load(&dir).unwrap();
        assert_eq!(c.get("r_yres"), Some("1080"));
        c.save(&[("r_yres", "1440".into())], &dir.join("b")).unwrap();
        let written = std::fs::read(dir.join("em4.cfg")).unwrap();
        assert_eq!(&written[..2], &[0xFF, 0xFE]);
        assert_eq!(written, encode(&SAMPLE.replace("\"1080\"", "\"1440\""), Encoding::Utf16Le));
        // Backup is byte-identical to the original file.
        assert_eq!(std::fs::read(dir.join("b").join("em4.cfg.vor_launcher")).unwrap(), original);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Works on a temp copy of the real file, never on the file itself.
    #[test]
    fn bloom_change_survives_reload_on_copy_of_real_cfg() {
        let real = Path::new(r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe\em4.cfg");
        let Ok(bytes) = std::fs::read(real) else { return };
        let dir = std::env::temp_dir().join(format!("em4r_em4cfg_real_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("em4.cfg"), &bytes).unwrap();
        let mut c = Em4Cfg::load(&dir).unwrap();
        let flipped = if c.get_bool("e4_bloom") { "0" } else { "1" };
        assert_eq!(c.save(&[("e4_bloom", flipped.into())], &dir.join("b")).unwrap(), 1);
        let reloaded = Em4Cfg::load(&dir).unwrap();
        assert_eq!(reloaded.get("e4_bloom"), Some(flipped));
        assert_eq!(reloaded.vars.len(), c.vars.len());
        assert_eq!(std::fs::read(dir.join("em4.cfg")).unwrap().len(), bytes.len());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn real_display_modes_and_real_cfg() {
        let modes = display_modes();
        assert!(!modes.is_empty(), "keine Bildschirmmodi");
        let game = Path::new(r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe");
        if game.join("em4.cfg").exists() {
            let c = Em4Cfg::load(game).unwrap();
            assert!(c.get("r_xres").is_some() && c.get("e4_shadow").is_some());
        }
    }
}
