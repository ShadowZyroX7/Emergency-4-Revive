//! Minimal DDS decoder (first mip level only) for mod preview images:
//! uncompressed RGB(A) with bit masks, DXT1, DXT3 and DXT5.

/// Decoded image: width, height, RGBA8 pixels.
pub type Rgba = (u32, u32, Vec<u8>);

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

pub fn decode(b: &[u8]) -> Option<Rgba> {
    if b.get(0..4)? != b"DDS " {
        return None;
    }
    let height = u32_at(b, 12)?;
    let width = u32_at(b, 16)?;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return None;
    }
    let pf_flags = u32_at(b, 80)?;
    let data = b.get(128..)?;
    if pf_flags & DDPF_FOURCC != 0 {
        match b.get(84..88)? {
            b"DXT1" => decode_blocks(data, width, height, 8, |blk, out| decode_color(blk, out, true)),
            b"DXT3" => decode_blocks(data, width, height, 16, |blk, out| {
                decode_color(&blk[8..], out, false);
                for i in 0..16 {
                    let nibble = (blk[i / 2] >> ((i % 2) * 4)) & 0xF;
                    out[i][3] = nibble * 17;
                }
            }),
            b"DXT5" => decode_blocks(data, width, height, 16, |blk, out| {
                decode_color(&blk[8..], out, false);
                let alphas = dxt5_alpha_table(blk[0], blk[1]);
                let bits = blk[2..8].iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64);
                for (i, px) in out.iter_mut().enumerate() {
                    px[3] = alphas[((bits >> (3 * i)) & 7) as usize];
                }
            }),
            _ => None,
        }
    } else if pf_flags & DDPF_RGB != 0 {
        decode_masked(data, width, height, b, pf_flags & DDPF_ALPHAPIXELS != 0)
    } else {
        None
    }
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1F) as u32;
    let g = ((c >> 5) & 0x3F) as u32;
    let b = (c & 0x1F) as u32;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
}

/// Decode a DXT color block into 16 RGBA pixels (row-major in the block).
fn decode_color(blk: &[u8], out: &mut [[u8; 4]; 16], dxt1: bool) {
    let c0 = u16::from_le_bytes([blk[0], blk[1]]);
    let c1 = u16::from_le_bytes([blk[2], blk[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u32, wb: u32, d: u32| -> [u8; 4] {
        let f = |i: usize| ((a[i] as u32 * wa + b[i] as u32 * wb) / d) as u8;
        [f(0), f(1), f(2), 255]
    };
    let palette: [[u8; 4]; 4] = if !dxt1 || c0 > c1 {
        [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let idx = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = palette[((idx >> (2 * i)) & 3) as usize];
    }
}

fn dxt5_alpha_table(a0: u8, a1: u8) -> [u8; 8] {
    let (a0w, a1w) = (a0 as u32, a1 as u32);
    let mut t = [a0, a1, 0, 0, 0, 0, 0, 0];
    if a0 > a1 {
        for i in 1..7u32 {
            t[i as usize + 1] = (((7 - i) * a0w + i * a1w) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            t[i as usize + 1] = (((5 - i) * a0w + i * a1w) / 5) as u8;
        }
        t[6] = 0;
        t[7] = 255;
    }
    t
}

fn decode_blocks(
    data: &[u8],
    w: u32,
    h: u32,
    block_bytes: usize,
    decode_block: impl Fn(&[u8], &mut [[u8; 4]; 16]),
) -> Option<Rgba> {
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    if data.len() < bw * bh * block_bytes {
        return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    let mut px = [[0u8; 4]; 16];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * block_bytes;
            decode_block(&data[off..off + block_bytes], &mut px);
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w as usize && y < h as usize {
                    let o = (y * w as usize + x) * 4;
                    out[o..o + 4].copy_from_slice(p);
                }
            }
        }
    }
    Some((w, h, out))
}

fn decode_masked(data: &[u8], w: u32, h: u32, header: &[u8], has_alpha: bool) -> Option<Rgba> {
    let bits = u32_at(header, 88)?;
    let bpp = match bits {
        32 => 4,
        24 => 3,
        _ => return None,
    };
    let masks = [u32_at(header, 92)?, u32_at(header, 96)?, u32_at(header, 100)?, u32_at(header, 104)?];
    let n = (w * h) as usize;
    if data.len() < n * bpp {
        return None;
    }
    let extract = |v: u32, m: u32| -> u8 {
        if m == 0 {
            return 255;
        }
        let shifted = (v & m) >> m.trailing_zeros();
        let max = m >> m.trailing_zeros();
        (shifted * 255 / max) as u8
    };
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let p = &data[i * bpp..i * bpp + bpp];
        let v = p.iter().rev().fold(0u32, |acc, &x| (acc << 8) | x as u32);
        out.extend_from_slice(&[
            extract(v, masks[0]),
            extract(v, masks[1]),
            extract(v, masks[2]),
            if has_alpha { extract(v, masks[3]) } else { 255 },
        ]);
    }
    Some((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32, pf_flags: u32, fourcc: &[u8; 4], bits: u32, masks: [u32; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[80..84].copy_from_slice(&pf_flags.to_le_bytes());
        b[84..88].copy_from_slice(fourcc);
        b[88..92].copy_from_slice(&bits.to_le_bytes());
        for (i, m) in masks.iter().enumerate() {
            b[92 + i * 4..96 + i * 4].copy_from_slice(&m.to_le_bytes());
        }
        b
    }

    #[test]
    fn dxt1_solid_red() {
        let mut b = header(4, 4, DDPF_FOURCC, b"DXT1", 0, [0; 4]);
        // c0 = pure red (0xF800), c1 = black, all indices 0.
        b.extend_from_slice(&[0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0]);
        let (w, h, px) = decode(&b).unwrap();
        assert_eq!((w, h), (4, 4));
        assert!(px.chunks(4).all(|p| p == [255, 0, 0, 255]));
    }

    #[test]
    fn dxt1_transparent_mode() {
        let mut b = header(4, 4, DDPF_FOURCC, b"DXT1", 0, [0; 4]);
        // c0 <= c1 -> 3-color mode; index 3 = transparent.
        b.extend_from_slice(&[0x00, 0x00, 0x00, 0xF8, 0xFF, 0xFF, 0xFF, 0xFF]);
        let (_, _, px) = decode(&b).unwrap();
        assert!(px.chunks(4).all(|p| p[3] == 0));
    }

    #[test]
    fn dxt5_alpha_endpoints() {
        let mut b = header(4, 4, DDPF_FOURCC, b"DXT5", 0, [0; 4]);
        // alpha0=255, alpha1=0, all alpha indices 1 (=alpha1=0); color white.
        b.extend_from_slice(&[255, 0, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24]);
        b.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
        let (_, _, px) = decode(&b).unwrap();
        assert!(px.chunks(4).all(|p| p == [255, 255, 255, 0]), "{:?}", &px[..8]);
    }

    #[test]
    fn uncompressed_bgra() {
        let mut b = header(1, 1, DDPF_RGB | DDPF_ALPHAPIXELS, b"\0\0\0\0", 32, [0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000]);
        b.extend_from_slice(&[0x30, 0x20, 0x10, 0x80]); // B G R A in memory
        assert_eq!(decode(&b).unwrap().2, vec![0x10, 0x20, 0x30, 0x80]);
    }

    #[test]
    fn rejects_garbage_and_truncated() {
        assert!(decode(b"not a dds").is_none());
        let b = header(64, 64, DDPF_FOURCC, b"DXT1", 0, [0; 4]);
        assert!(decode(&b).is_none()); // no pixel data
    }

    /// All installed mods' DDS previews must decode.
    #[test]
    fn decodes_installed_previews() {
        let mods = std::path::Path::new(r"C:\SteamLibrary\steamapps\common\EMERGENCY 4 Deluxe\Mods");
        let Ok(entries) = std::fs::read_dir(mods) else { return };
        for e in entries.flatten() {
            let p = e.path().join("e4mod.dds");
            if let Ok(bytes) = std::fs::read(&p) {
                let (w, h, px) = decode(&bytes).unwrap_or_else(|| panic!("{} nicht lesbar", p.display()));
                assert_eq!(px.len(), (w * h * 4) as usize);
                // Not a blank image.
                assert!(px.chunks(4).any(|c| c[0] != px[0] || c[1] != px[1] || c[2] != px[2]), "{}", p.display());
            }
        }
    }
}
