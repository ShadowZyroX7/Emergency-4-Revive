//! File access statistics: how many files the game opens, how many lookups
//! fail, how much it reads and how long all that takes – per phase and per
//! file extension.

use core::ffi::c_void;
use em4r_core::counters::{phase_now, Phase};
use em4r_core::ffi::*;
use em4r_core::hook::IatHook;
use em4r_core::iat_hook;
use em4r_core::util::{qpc_now, qpc_to_ms};
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

struct PhaseStats {
    opens: AtomicU64,
    open_fail: AtomicU64,
    open_ticks: AtomicU64,
    reads: AtomicU64,
    read_bytes: AtomicU64,
    read_ticks: AtomicU64,
    attrs: AtomicU64,
    attr_fail: AtomicU64,
    attr_ticks: AtomicU64,
    finds: AtomicU64,
    find_ticks: AtomicU64,
}

#[derive(Default, Clone, Copy)]
struct ExtStats {
    opens: u64,
    fails: u64,
    open_ticks: u64,
    reads: u64,
    bytes: u64,
    read_ticks: u64,
}

struct Shared {
    /// Extension of each open file handle, to attribute reads.
    handles: HashMap<usize, String>,
    per_ext: HashMap<(Phase, String), ExtStats>,
    /// The slowest successful opens: (ticks, path).
    slowest: Vec<(u64, String)>,
}

impl PhaseStats {
    const fn new() -> Self {
        Self {
            opens: AtomicU64::new(0),
            open_fail: AtomicU64::new(0),
            open_ticks: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            read_bytes: AtomicU64::new(0),
            read_ticks: AtomicU64::new(0),
            attrs: AtomicU64::new(0),
            attr_fail: AtomicU64::new(0),
            attr_ticks: AtomicU64::new(0),
            finds: AtomicU64::new(0),
            find_ticks: AtomicU64::new(0),
        }
    }
}

/// Indexed by `Phase as usize`.
static STATS: [PhaseStats; 2] = [PhaseStats::new(), PhaseStats::new()];

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
const SLOWEST_KEPT: usize = 15;

static ORIG_CREATE_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_READ: AtomicUsize = AtomicUsize::new(0);
static ORIG_ATTR_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_FIND_A: AtomicUsize = AtomicUsize::new(0);

type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, HANDLE) -> HANDLE;
type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, HANDLE) -> HANDLE;
type ReadFile = unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut c_void) -> BOOL;
type GetFileAttributesA = unsafe extern "system" fn(*const u8) -> u32;
type FindFirstFileA = unsafe extern "system" fn(*const u8, *mut c_void) -> HANDLE;

pub fn init() {
    if let Ok(mut g) = SHARED.lock() {
        *g = Some(Shared { handles: HashMap::new(), per_ext: HashMap::new(), slowest: Vec::new() });
    }
}

pub fn hooks() -> Vec<IatHook> {
    vec![
        iat_hook!("kernel32.dll", "CreateFileA", hk_create_file_a, ORIG_CREATE_A),
        iat_hook!("kernel32.dll", "CreateFileW", hk_create_file_w, ORIG_CREATE_W),
        iat_hook!("kernel32.dll", "ReadFile", hk_read_file, ORIG_READ),
        iat_hook!("kernel32.dll", "GetFileAttributesA", hk_get_file_attributes_a, ORIG_ATTR_A),
        iat_hook!("kernel32.dll", "FindFirstFileA", hk_find_first_file_a, ORIG_FIND_A),
    ]
}

/// Lower-case extension of a path, e.g. `.dds`; `(ohne)` if none.
pub fn extension_of(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match file.rfind('.') {
        Some(i) if i + 1 < file.len() && file.len() - i <= 10 => file[i..].to_ascii_lowercase(),
        _ => "(ohne)".into(),
    }
}

unsafe fn ansi_to_string(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 && len < 1024 {
        len += 1;
    }
    // Paths are in the system ANSI codepage; Latin-1 is close enough for stats.
    std::slice::from_raw_parts(p, len).iter().map(|&b| b as char).collect()
}

unsafe fn wide_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 && len < 1024 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

fn record_open(path: String, handle: HANDLE, ticks: u64) {
    let phase = phase_now();
    let s = &STATS[phase as usize];
    s.opens.fetch_add(1, Ordering::Relaxed);
    s.open_ticks.fetch_add(ticks, Ordering::Relaxed);
    let failed = handle == INVALID_HANDLE_VALUE;
    if failed {
        s.open_fail.fetch_add(1, Ordering::Relaxed);
    }
    let ext = extension_of(&path);
    let Ok(mut g) = SHARED.lock() else { return };
    let Some(sh) = g.as_mut() else { return };
    let e = sh.per_ext.entry((phase, ext.clone())).or_default();
    e.opens += 1;
    e.open_ticks += ticks;
    if failed {
        e.fails += 1;
        return;
    }
    sh.handles.insert(handle as usize, ext);
    let slow_enough = sh.slowest.len() < SLOWEST_KEPT || sh.slowest.last().is_some_and(|(t, _)| ticks > *t);
    if slow_enough {
        sh.slowest.push((ticks, path));
        sh.slowest.sort_by(|a, b| b.0.cmp(&a.0));
        sh.slowest.truncate(SLOWEST_KEPT);
    }
}

unsafe extern "system" fn hk_create_file_a(
    name: *const u8, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: HANDLE,
) -> HANDLE {
    let f: CreateFileA = core::mem::transmute(ORIG_CREATE_A.load(Ordering::Acquire));
    let t0 = qpc_now();
    let h = f(name, access, share, sa, disp, flags, template);
    let ticks = (qpc_now() - t0) as u64;
    record_open(ansi_to_string(name), h, ticks);
    h
}

unsafe extern "system" fn hk_create_file_w(
    name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: HANDLE,
) -> HANDLE {
    let f: CreateFileW = core::mem::transmute(ORIG_CREATE_W.load(Ordering::Acquire));
    let t0 = qpc_now();
    let h = f(name, access, share, sa, disp, flags, template);
    let ticks = (qpc_now() - t0) as u64;
    record_open(wide_to_string(name), h, ticks);
    h
}

unsafe extern "system" fn hk_read_file(h: HANDLE, buf: *mut c_void, n: u32, read: *mut u32, ov: *mut c_void) -> BOOL {
    let f: ReadFile = core::mem::transmute(ORIG_READ.load(Ordering::Acquire));
    let t0 = qpc_now();
    let ok = f(h, buf, n, read, ov);
    let ticks = (qpc_now() - t0) as u64;
    let bytes = if !read.is_null() && ok != 0 { *read as u64 } else { 0 };
    let phase = phase_now();
    let s = &STATS[phase as usize];
    s.reads.fetch_add(1, Ordering::Relaxed);
    s.read_bytes.fetch_add(bytes, Ordering::Relaxed);
    s.read_ticks.fetch_add(ticks, Ordering::Relaxed);
    if let Ok(mut g) = SHARED.lock() {
        if let Some(sh) = g.as_mut() {
            if let Some(ext) = sh.handles.get(&(h as usize)).cloned() {
                let e = sh.per_ext.entry((phase, ext)).or_default();
                e.reads += 1;
                e.bytes += bytes;
                e.read_ticks += ticks;
            }
        }
    }
    ok
}

unsafe extern "system" fn hk_get_file_attributes_a(name: *const u8) -> u32 {
    let f: GetFileAttributesA = core::mem::transmute(ORIG_ATTR_A.load(Ordering::Acquire));
    let t0 = qpc_now();
    let r = f(name);
    let s = &STATS[phase_now() as usize];
    s.attrs.fetch_add(1, Ordering::Relaxed);
    s.attr_ticks.fetch_add((qpc_now() - t0) as u64, Ordering::Relaxed);
    if r == INVALID_FILE_ATTRIBUTES {
        s.attr_fail.fetch_add(1, Ordering::Relaxed);
    }
    r
}

unsafe extern "system" fn hk_find_first_file_a(name: *const u8, data: *mut c_void) -> HANDLE {
    let f: FindFirstFileA = core::mem::transmute(ORIG_FIND_A.load(Ordering::Acquire));
    let t0 = qpc_now();
    let h = f(name, data);
    let s = &STATS[phase_now() as usize];
    s.finds.fetch_add(1, Ordering::Relaxed);
    s.find_ticks.fetch_add((qpc_now() - t0) as u64, Ordering::Relaxed);
    h
}

fn ms(ticks: u64) -> f64 {
    qpc_to_ms(ticks as i64)
}

/// Report section for file access.
pub fn render(out: &mut String) {
    let _ = writeln!(out, "\n=== Dateizugriffe ===");
    for phase in Phase::ALL {
        let s = &STATS[phase as usize];
        let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let _ = writeln!(out, "\n  {}:", phase.label());
        let _ = writeln!(
            out,
            "    Dateien geöffnet: {} (davon nicht gefunden: {}) — Zeit: {:.0} ms",
            ld(&s.opens), ld(&s.open_fail), ms(ld(&s.open_ticks))
        );
        let _ = writeln!(
            out,
            "    Gelesen: {:.1} MB in {} Lesevorgängen — Zeit: {:.0} ms",
            ld(&s.read_bytes) as f64 / 1048576.0, ld(&s.reads), ms(ld(&s.read_ticks))
        );
        let _ = writeln!(
            out,
            "    Existenz-Prüfungen: {} (davon fehlend: {}) — Zeit: {:.0} ms | Ordnersuchen: {} — Zeit: {:.0} ms",
            ld(&s.attrs), ld(&s.attr_fail), ms(ld(&s.attr_ticks)), ld(&s.finds), ms(ld(&s.find_ticks))
        );
    }

    let Ok(g) = SHARED.lock() else { return };
    let Some(sh) = g.as_ref() else { return };
    let _ = writeln!(out, "\n  Nach Dateiendung (Laden), sortiert nach Gesamtzeit:");
    let _ = writeln!(out, "    {:<10} {:>8} {:>8} {:>10} {:>10} {:>10}", "Endung", "geöffnet", "fehlt", "MB", "Öffnen ms", "Lesen ms");
    let mut rows: Vec<_> = sh.per_ext.iter().filter(|((p, _), _)| *p == Phase::Loading).collect();
    rows.sort_by(|a, b| (b.1.open_ticks + b.1.read_ticks).cmp(&(a.1.open_ticks + a.1.read_ticks)));
    for ((_, ext), e) in rows.into_iter().take(15) {
        let _ = writeln!(
            out,
            "    {:<10} {:>8} {:>8} {:>10.1} {:>10.0} {:>10.0}",
            ext, e.opens, e.fails, e.bytes as f64 / 1048576.0, ms(e.open_ticks), ms(e.read_ticks)
        );
    }
    let _ = writeln!(out, "\n  Langsamste einzelne Datei-Öffnungen:");
    for (t, p) in &sh.slowest {
        let _ = writeln!(out, "    {:8.1} ms  {p}", ms(*t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(extension_of(r"C:\Spiel\Mods\AUBMP\Models\car.V3O"), ".v3o");
        assert_eq!(extension_of("mod:/Textures/a.b/road.DDS"), ".dds");
        assert_eq!(extension_of(r"C:\Spiel\README"), "(ohne)");
        assert_eq!(extension_of(r"C:\Spiel\ordner.mit.punkt\datei"), "(ohne)");
        assert_eq!(extension_of("x."), "(ohne)");
    }
}
