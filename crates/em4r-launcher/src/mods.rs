//! Discovering installed EM4 mods (`Mods\<Ordner>\e4mod.info`).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    /// Folder name – what `Em4.exe -game -mod <folder>` expects.
    pub folder: String,
    pub name: String,
    pub author: String,
    /// Human version, e.g. "Version 1.1" (from `comment`) or "1.3".
    pub version: String,
    /// Map names without extension, e.g. `freeplay`, `multiplayer`.
    pub maps: Vec<String>,
    pub preview: Option<PathBuf>,
    pub has_campaign: bool,
}

/// Decode a text file that may be UTF-16 (with BOM), UTF-8 or ANSI.
pub fn decode_text(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        [0xFE, 0xFF, rest @ ..] => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            // Old mods use the Windows ANSI codepage; Latin-1 covers umlauts.
            Err(_) => bytes.iter().map(|&b| b as char).collect(),
        },
    }
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Attributes of the first `<mod .../>` element as (key, value) pairs.
pub fn mod_attributes(xml: &str) -> Vec<(String, String)> {
    let Some(start) = xml.find("<mod") else { return Vec::new() };
    let rest = &xml[start + 4..];
    let end = rest.find('>').unwrap_or(rest.len());
    let mut tag = &rest[..end];
    let mut out = Vec::new();
    while let Some(eq) = tag.find('=') {
        let key = tag[..eq].trim().trim_start_matches('/').trim().to_string();
        let after = tag[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let body = &after[1..];
        let Some(close) = body.find(quote) else { break };
        out.push((key, unescape(&body[..close])));
        tag = &body[close + 1..];
    }
    out
}

pub fn parse_info(folder: &str, bytes: &[u8]) -> ModInfo {
    let attrs = mod_attributes(&decode_text(bytes));
    let get = |k: &str| attrs.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string());
    let version = get("comment").filter(|c| !c.is_empty()).unwrap_or_else(|| match (get("major"), get("minor")) {
        (Some(a), Some(b)) => format!("{a}.{b}"),
        _ => String::new(),
    });
    ModInfo {
        folder: folder.to_string(),
        name: get("name").filter(|n| !n.is_empty()).unwrap_or_else(|| folder.to_string()),
        author: get("author").unwrap_or_default(),
        version,
        maps: Vec::new(),
        preview: None,
        has_campaign: false,
    }
}

/// All mods in `<game>\Mods`, sorted by display name.
pub fn scan(game_dir: &Path) -> Vec<ModInfo> {
    let Ok(entries) = std::fs::read_dir(game_dir.join("Mods")) else { return Vec::new() };
    let mut mods: Vec<ModInfo> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let dir = e.path();
            let folder = e.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(dir.join("e4mod.info")).ok()?;
            let mut m = parse_info(&folder, &bytes);
            m.maps = list_maps(&dir.join("Maps"));
            m.preview = ["e4mod.jpg", "e4mod.png", "e4mod.dds"].iter().map(|f| dir.join(f)).find(|p| p.is_file());
            m.has_campaign = dir.join("Specs").join("campaign.xml").is_file();
            Some(m)
        })
        .collect();
    mods.sort_by_key(|m| m.name.to_lowercase());
    mods
}

fn list_maps(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut maps: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let is_map = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("e4m"));
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            (is_map && !stem.eq_ignore_ascii_case("autosave")).then_some(stem)
        })
        .collect();
    maps.sort_by_key(|m| m.to_lowercase());
    maps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_bom(s: &str) -> Vec<u8> {
        let mut v = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_aubmp_style_info() {
        let xml = "<e4mod>\r\n    <mod name=\"Auenburg Modifikation\" author=\"OG Experience\" major=\"1\" minor=\"3\" comment=\"Version 1.1\" />\r\n</e4mod>\r\n";
        let m = parse_info("AUBMP", &utf16_bom(xml));
        assert_eq!(m.name, "Auenburg Modifikation");
        assert_eq!(m.author, "OG Experience");
        assert_eq!(m.version, "Version 1.1");
        assert_eq!(m.folder, "AUBMP");
    }

    #[test]
    fn falls_back_without_comment_or_name() {
        let m = parse_info("KREMP", b"<e4mod><mod author='A &amp; B' major=\"1\" minor=\"3\"/></e4mod>");
        assert_eq!(m.name, "KREMP");
        assert_eq!(m.author, "A & B");
        assert_eq!(m.version, "1.3");
    }

    #[test]
    fn decodes_ansi_umlauts() {
        assert_eq!(decode_text(&[b'G', 0xFC, b'r']), "Gür");
        assert_eq!(decode_text("Grün".as_bytes()), "Grün");
    }

    #[test]
    fn broken_info_does_not_panic() {
        let m = parse_info("X", b"<mod name=\"unterminated");
        assert_eq!(m.name, "X");
        let m = parse_info("Y", b"garbage");
        assert_eq!(m.name, "Y");
    }

    /// Against the real installation, if present.
    #[test]
    fn scans_installed_mods() {
        let game = Path::new(r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe");
        if !game.join("Mods").is_dir() {
            return;
        }
        let mods = scan(game);
        let aubmp = mods.iter().find(|m| m.folder == "AUBMP").expect("AUBMP");
        assert_eq!(aubmp.name, "Auenburg Modifikation");
        assert!(aubmp.maps.contains(&"freeplay".to_string()));
        assert!(aubmp.preview.is_some());
        let wilk = mods.iter().find(|m| m.folder == "Wilkorzec").expect("Wilkorzec");
        assert!(!wilk.maps.iter().any(|m| m.eq_ignore_ascii_case("autosave")));
        assert!(mods.len() >= 5);
    }
}
