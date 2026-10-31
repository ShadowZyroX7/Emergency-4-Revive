//! File access statistics: how many files the game opens, how many lookups
//! fail, how much it reads and how long all that takes – per phase and per
//! file extension.

use core::ffi::c_void;
use em4r_core::counters::{phase_now, Phase};
use em4r_core::ffi::*;
use em4r_core::fscache::{self, MissingCache};
use std::sync::atomic::AtomicBool;
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
    /// Paths that were looked up but do not exist: path -> count.
    missing: HashMap<String, u32>,
}

const MISSING_CAP: usize = 50_000;

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
static ORIG_ATTR_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_FIND_A: AtomicUsize = AtomicUsize::new(0);

type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, HANDLE) -> HANDLE;
type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, HANDLE) -> HANDLE;
type ReadFile = unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut c_void) -> BOOL;
type GetFileAttributesA = unsafe extern "system" fn(*const u8) -> u32;
type GetFileAttributesW = unsafe extern "system" fn(*const u16) -> u32;
type FindFirstFileA = unsafe extern "system" fn(*const u8, *mut c_void) -> HANDLE;

pub fn init() {
    if let Ok(mut g) = SHARED.lock() {
        *g = Some(Shared { handles: HashMap::new(), per_ext: HashMap::new(), slowest: Vec::new(), missing: HashMap::new() });
    }
}

/// Em4.exe calls CreateFileW through an MSLU (unicows) lazy-binding slot,
/// not through its import table. The slot initially holds a resolver stub;
/// we put our detour there instead (the detour calls the real CreateFileW,
/// exactly what the stub would end up doing on Windows NT).
const MSLU_CREATEFILEW_SLOT: usize = 0xA934D8;
const MSLU_CREATEFILEW_STUB: usize = 0x9551D8;

pub fn patch_em4_createfilew() {
    use em4r_core::patch::{swap_pointer, PatchResult};
    let Some(real) = em4r_core::util::loaded_proc("kernel32.dll", "CreateFileW") else { return };
    let _ = ORIG_CREATE_W.compare_exchange(0, real, Ordering::AcqRel, Ordering::Acquire);
    if ORIG_CREATE_W.load(Ordering::Acquire) != real {
        em4r_core::warn!("Datei-Hooks: CreateFileW ist bereits umgeleitet – Em4.exe-Zugriffe nicht erfasst");
        return;
    }
    let detour = hk_create_file_w as *const () as usize;
    // SAFETY: fixed-base Em4.exe .data slot; verified value before writing.
    let r = unsafe {
        match swap_pointer(MSLU_CREATEFILEW_SLOT, MSLU_CREATEFILEW_STUB, detour) {
            PatchResult::Mismatch(_) => swap_pointer(MSLU_CREATEFILEW_SLOT, real, detour),
            other => other,
        }
    };
    match r {
        PatchResult::Applied | PatchResult::AlreadyApplied => {
            em4r_core::info!("Datei-Hooks: auch Em4.exe-Dateizugriffe (unicows CreateFileW) erfasst")
        }
        other => em4r_core::warn!("Datei-Hooks: Em4.exe-CreateFileW nicht umleitbar ({other:?})"),
    }
}

/// CreateFile hooks always; the remaining ones only collect statistics.
pub fn hooks(stats: bool) -> Vec<IatHook> {
    let mut v = vec![
        iat_hook!("kernel32.dll", "CreateFileA", hk_create_file_a, ORIG_CREATE_A),
        iat_hook!("kernel32.dll", "CreateFileW", hk_create_file_w, ORIG_CREATE_W),
    ];
    if stats {
        v.extend(stats_hooks());
    }
    v
}

fn stats_hooks() -> Vec<IatHook> {
    vec![
        iat_hook!("kernel32.dll", "ReadFile", hk_read_file, ORIG_READ),
        iat_hook!("kernel32.dll", "GetFileAttributesA", hk_get_file_attributes_a, ORIG_ATTR_A),
        iat_hook!("kernel32.dll", "GetFileAttributesW", hk_get_file_attributes_w, ORIG_ATTR_W),
        iat_hook!("kernel32.dll", "FindFirstFileA", hk_find_first_file_a, ORIG_FIND_A),
        // Handle values are reused by Windows: forget closed handles so reads
        // are never attributed to an old file's extension.
        iat_hook!("kernel32.dll", "CloseHandle", hk_close_handle, ORIG_CLOSE),
    ]
}

static ORIG_CLOSE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn hk_close_handle(h: HANDLE) -> BOOL {
    if let Ok(mut g) = SHARED.lock() {
        if let Some(sh) = g.as_mut() {
            sh.handles.remove(&(h as usize));
        }
    }
    let f: unsafe extern "system" fn(HANDLE) -> BOOL = core::mem::transmute(ORIG_CLOSE.load(Ordering::Acquire));
    f(h)
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
        if sh.missing.len() < MISSING_CAP || sh.missing.contains_key(&path) {
            *sh.missing.entry(path).or_default() += 1;
        }
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

/// Statistics (profiler) and the missing-file cache are independent features
/// sharing the same CreateFile detours.
static STATS_ON: AtomicBool = AtomicBool::new(false);
static CACHE_ON: AtomicBool = AtomicBool::new(false);
static MISSING: MissingCache = MissingCache::new();

pub fn configure(stats: bool, cache: bool) {
    if stats {
        init();
    }
    STATS_ON.store(stats, Ordering::Release);
    CACHE_ON.store(cache, Ordering::Release);
}

pub fn cache_entries() -> usize {
    MISSING.len()
}

/// Shared logic of both CreateFile detours.
unsafe fn create_common(
    path: String,
    access: u32,
    disp: u32,
    real: impl FnOnce() -> HANDLE,
) -> HANDLE {
    let cache = CACHE_ON.load(Ordering::Relaxed);
    let cacheable = cache && fscache::is_read_only_open(access, disp);
    let key = if cache { fscache::normalize(&path) } else { String::new() };
    if cacheable {
        if let Some(err) = MISSING.lookup(&key) {
            em4r_core::counters::FS_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            SetLastError(err);
            return INVALID_HANDLE_VALUE;
        }
    }
    let t0 = qpc_now();
    let h = real();
    let err = GetLastError();
    let ticks = (qpc_now() - t0) as u64;
    if cache {
        if h == INVALID_HANDLE_VALUE {
            if cacheable && fscache::is_not_found(err) {
                MISSING.remember(key, err);
            }
        } else if !cacheable {
            // The file may have been created or changed: never hide it.
            MISSING.forget(&key);
        }
    }
    if STATS_ON.load(Ordering::Relaxed) {
        record_open(path, h, ticks);
    }
    SetLastError(err);
    h
}

unsafe extern "system" fn hk_create_file_a(
    name: *const u8, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: HANDLE,
) -> HANDLE {
    let f: CreateFileA = core::mem::transmute(ORIG_CREATE_A.load(Ordering::Acquire));
    create_common(ansi_to_string(name), access, disp, || f(name, access, share, sa, disp, flags, template))
}

unsafe extern "system" fn hk_create_file_w(
    name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: HANDLE,
) -> HANDLE {
    let f: CreateFileW = core::mem::transmute(ORIG_CREATE_W.load(Ordering::Acquire));
    create_common(wide_to_string(name), access, disp, || f(name, access, share, sa, disp, flags, template))
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

unsafe extern "system" fn hk_get_file_attributes_w(name: *const u16) -> u32 {
    let f: GetFileAttributesW = core::mem::transmute(ORIG_ATTR_W.load(Ordering::Acquire));
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

/// Every missing path with its count, for offline analysis.
pub fn write_missing(path: &std::path::Path) {
    let Ok(g) = SHARED.lock() else { return };
    let Some(sh) = g.as_ref() else { return };
    let mut miss: Vec<_> = sh.missing.iter().collect();
    miss.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = String::with_capacity(miss.len() * 80);
    for (p, n) in miss {
        let _ = writeln!(out, "{n}\t{p}");
    }
    let _ = std::fs::write(path, out);
}

/// Report section for file access.
pub fn render(out: &mut String) {
    let _ = writeln!(out, "\n=== Dateizugriffe ===");
    let _ = writeln!(
        out,
        "  Vom Fehlende-Dateien-Speicher sofort beantwortet: {} (gemerkte Pfade: {})",
        em4r_core::counters::FS_CACHE_HITS.load(Ordering::Relaxed),
        MISSING.len()
    );
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
    for phase in [Phase::Loading, Phase::Playing] {
        let _ = writeln!(out, "\n  Nach Dateiendung ({}), sortiert nach Gesamtzeit:", phase.label());
        let _ = writeln!(
            out,
            "    {:<10} {:>8} {:>8} {:>10} {:>10} {:>10}",
            "Endung", "geöffnet", "fehlt", "MB", "Öffnen ms", "Lesen ms"
        );
        let mut rows: Vec<_> = sh.per_ext.iter().filter(|((p, _), _)| *p == phase).collect();
        rows.sort_by(|a, b| (b.1.open_ticks + b.1.read_ticks).cmp(&(a.1.open_ticks + a.1.read_ticks)));
        for ((_, ext), e) in rows.into_iter().take(15) {
            let _ = writeln!(
                out,
                "    {:<10} {:>8} {:>8} {:>10.1} {:>10.0} {:>10.0}",
                ext, e.opens, e.fails, e.bytes as f64 / 1048576.0, ms(e.open_ticks), ms(e.read_ticks)
            );
        }
    }
    let _ = writeln!(out, "\n  Am häufigsten vergeblich gesuchte Dateien (Anzahl Versuche):");
    let mut miss: Vec<_> = sh.missing.iter().collect();
    miss.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (p, n) in miss.iter().take(25) {
        let _ = writeln!(out, "    {n:>7}x  {p}");
    }
    let _ = writeln!(out, "    (insgesamt {} verschiedene fehlende Pfade)", sh.missing.len());
    let _ = writeln!(out, "\n  Langsamste einzelne Datei-Öffnungen:");
    for (t, p) in &sh.slowest {
        let _ = writeln!(out, "    {:8.1} ms  {p}", ms(*t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end through the real detour and the real CreateFileW.
    #[test]
    fn missing_cache_answers_repeats_and_never_hides_created_files() {
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const CREATE_ALWAYS: u32 = 2;
        let real = em4r_core::util::loaded_proc("kernel32.dll", "CreateFileW").unwrap();
        let _ = ORIG_CREATE_W.compare_exchange(0, real, Ordering::AcqRel, Ordering::Acquire);
        configure(false, true);

        let dir = std::env::temp_dir().join(format!("em4r_fscache_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("spaeter_angelegt.dds");
        let w = em4r_core::util::wide(&file);
        let open = |access: u32, disp: u32| unsafe {
            hk_create_file_w(w.as_ptr(), access, 1, core::ptr::null_mut(), disp, 0x80, core::ptr::null_mut())
        };

        let hits0 = em4r_core::counters::FS_CACHE_HITS.load(Ordering::Relaxed);
        assert_eq!(open(GENERIC_READ, OPEN_EXISTING), INVALID_HANDLE_VALUE);
        assert_eq!(unsafe { GetLastError() }, ERROR_FILE_NOT_FOUND);
        // Second lookup: answered by the cache, same error code.
        assert_eq!(open(GENERIC_READ, OPEN_EXISTING), INVALID_HANDLE_VALUE);
        assert_eq!(unsafe { GetLastError() }, ERROR_FILE_NOT_FOUND);
        assert!(em4r_core::counters::FS_CACHE_HITS.load(Ordering::Relaxed) > hits0);

        // The game creates the file -> must be visible immediately.
        let h = open(GENERIC_WRITE, CREATE_ALWAYS);
        assert_ne!(h, INVALID_HANDLE_VALUE);
        unsafe { CloseHandle(h) };
        let h = open(GENERIC_READ, OPEN_EXISTING);
        assert_ne!(h, INVALID_HANDLE_VALUE, "angelegte Datei wurde versteckt");
        unsafe { CloseHandle(h) };
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_of(r"C:\Spiel\Mods\AUBMP\Models\car.V3O"), ".v3o");
        assert_eq!(extension_of("mod:/Textures/a.b/road.DDS"), ".dds");
        assert_eq!(extension_of(r"C:\Spiel\README"), "(ohne)");
        assert_eq!(extension_of(r"C:\Spiel\ordner.mit.punkt\datei"), "(ohne)");
        assert_eq!(extension_of("x."), "(ohne)");
    }
}
