//! em4r-re – static analysis helper for Em4.exe (developer tool, not shipped).
//!
//!   em4r-re <exe> func <addr>        disassemble the function containing addr
//!   em4r-re <exe> callers <addr>     direct callers of the function containing addr
//!   em4r-re <exe> xref <addr>        instructions referencing addr (imm, disp, branch)
//!   em4r-re <exe> str <text>         find a string and its references
//!   em4r-re <exe> dis <from> <to>    disassemble a raw range
//!
//! A linear sweep over .text builds the instruction list; call targets give
//! function starts. Good enough for MSVC 7.1 code.

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind};
use std::collections::{BTreeSet, HashMap};

struct Section {
    name: String,
    va: u32,
    vsize: u32,
    raw: u32,
    rawsize: u32,
}

struct Image {
    data: Vec<u8>,
    base: u32,
    sections: Vec<Section>,
    imports: HashMap<u32, String>, // IAT slot VA -> "dll!func"
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

impl Image {
    fn load(path: &str) -> Image {
        let data = std::fs::read(path).expect("read exe");
        let nt = u32_at(&data, 0x3C) as usize;
        let nsec = u16_at(&data, nt + 6) as usize;
        let opt = nt + 24;
        let optsize = u16_at(&data, nt + 20) as usize;
        let base = u32_at(&data, opt + 28);
        let mut sections = Vec::new();
        for i in 0..nsec {
            let s = opt + optsize + i * 40;
            sections.push(Section {
                name: String::from_utf8_lossy(&data[s..s + 8]).trim_end_matches('\0').to_string(),
                vsize: u32_at(&data, s + 8),
                va: u32_at(&data, s + 12),
                rawsize: u32_at(&data, s + 16),
                raw: u32_at(&data, s + 20),
            });
        }
        let mut img = Image { data, base, sections, imports: HashMap::new() };
        img.read_imports(opt);
        img
    }

    fn off(&self, rva: u32) -> Option<usize> {
        self.sections
            .iter()
            .find(|s| rva >= s.va && rva < s.va + s.vsize.max(s.rawsize))
            .filter(|s| rva - s.va < s.rawsize)
            .map(|s| (s.raw + rva - s.va) as usize)
    }

    fn cstr(&self, rva: u32) -> Option<String> {
        let o = self.off(rva)?;
        let end = self.data[o..].iter().position(|&b| b == 0)?;
        let s = &self.data[o..o + end];
        (s.len() >= 2 && s.len() < 200 && s.iter().all(|&b| (0x20..0x7f).contains(&b) || b == b'\t'))
            .then(|| String::from_utf8_lossy(s).into_owned())
    }

    fn read_imports(&mut self, opt: usize) {
        let dir_rva = u32_at(&self.data, opt + 96 + 8);
        let Some(mut d) = self.off(dir_rva) else { return };
        loop {
            let oft = u32_at(&self.data, d);
            let name = u32_at(&self.data, d + 12);
            let ft = u32_at(&self.data, d + 16);
            if name == 0 && ft == 0 {
                break;
            }
            let dll = self.cstr(name).unwrap_or_default();
            let thunks = if oft != 0 { oft } else { ft };
            let mut i = 0;
            while let Some(to) = self.off(thunks + i * 4) {
                let t = u32_at(&self.data, to);
                if t == 0 {
                    break;
                }
                let f = if t & 0x8000_0000 != 0 {
                    format!("#{}", t & 0xFFFF)
                } else {
                    self.cstr(t + 2).unwrap_or_default()
                };
                self.imports.insert(self.base + ft + i * 4, format!("{dll}!{f}"));
                i += 1;
            }
            d += 20;
        }
    }

    fn text(&self) -> &Section {
        self.sections.iter().find(|s| s.name == ".text").expect(".text")
    }

    /// Linear sweep of .text.
    fn sweep(&self) -> Vec<Instruction> {
        let t = self.text();
        let bytes = &self.data[t.raw as usize..(t.raw + t.rawsize) as usize];
        let ip = (self.base + t.va) as u64;
        let mut out = Vec::with_capacity(bytes.len() / 3);
        let mut pos = 0usize;
        while pos < bytes.len() {
            let mut dec = Decoder::with_ip(32, &bytes[pos..], ip + pos as u64, DecoderOptions::NONE);
            let ins = dec.decode();
            if ins.is_invalid() {
                pos += 1;
                continue;
            }
            pos += ins.len();
            out.push(ins);
        }
        out
    }

    fn annotate(&self, ins: &Instruction) -> String {
        let mut notes = Vec::new();
        let mut check = |v: u64| {
            let v = v as u32;
            if let Some(i) = self.imports.get(&v) {
                notes.push(i.clone());
            } else if v > self.base {
                if let Some(s) = self.cstr(v - self.base) {
                    notes.push(format!("\"{s}\""));
                }
            }
        };
        for i in 0..ins.op_count() {
            match ins.op_kind(i) {
                OpKind::Immediate32 | OpKind::Immediate8to32 => check(ins.immediate(i)),
                OpKind::Memory => check(ins.memory_displacement64()),
                _ => {}
            }
        }
        notes.join(" ")
    }
}

fn parse_addr(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex address")
}

fn print_ins(img: &Image, fmt: &mut IntelFormatter, ins: &Instruction) {
    let mut s = String::new();
    fmt.format(ins, &mut s);
    let note = img.annotate(ins);
    if note.is_empty() {
        println!("{:08X}  {s}", ins.ip());
    } else {
        println!("{:08X}  {s:<48} ; {note}", ins.ip());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("Aufruf: em4r-re <exe> func|callers|xref|str|dis <arg> [arg2]");
        std::process::exit(2);
    }
    let img = Image::load(&args[1]);
    let all = img.sweep();
    let mut fmt = IntelFormatter::new();
    fmt.options_mut().set_hex_prefix("0x");
    fmt.options_mut().set_hex_suffix("");

    // Function starts = direct call targets inside .text.
    let t = img.text();
    let (lo, hi) = ((img.base + t.va) as u64, (img.base + t.va + t.vsize) as u64);
    let starts: BTreeSet<u64> = all
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch32)
        .map(|i| i.near_branch_target())
        .filter(|&x| x >= lo && x < hi)
        .collect();
    let func_of = |a: u64| starts.range(..=a).next_back().copied();
    let index_of = |a: u64| all.partition_point(|i| i.ip() < a);

    match args[2].as_str() {
        "func" => {
            let a = parse_addr(&args[3]);
            let Some(start) = func_of(a) else { return eprintln!("keine Funktion gefunden") };
            let end = starts.range(start + 1..).next().copied().unwrap_or(hi);
            println!("; Funktion 0x{start:X} .. 0x{end:X}");
            for ins in &all[index_of(start)..index_of(end)] {
                print_ins(&img, &mut fmt, ins);
            }
        }
        "dis" => {
            let (a, b) = (parse_addr(&args[3]), parse_addr(&args[4]));
            for ins in &all[index_of(a)..index_of(b)] {
                print_ins(&img, &mut fmt, ins);
            }
        }
        "callers" => {
            let a = parse_addr(&args[3]);
            let start = func_of(a).unwrap_or(a);
            println!("; Aufrufer von 0x{start:X}");
            for ins in all.iter().filter(|i| {
                matches!(i.mnemonic(), Mnemonic::Call | Mnemonic::Jmp)
                    && i.op0_kind() == OpKind::NearBranch32
                    && i.near_branch_target() == start
            }) {
                let f = func_of(ins.ip()).unwrap_or(0);
                print!("  in Funktion 0x{f:X}: ");
                print_ins(&img, &mut fmt, ins);
            }
            // Also report the address appearing as data (vtables, callbacks).
            let needle = (start as u32).to_le_bytes();
            for s in img.sections.iter().filter(|s| s.name != ".text") {
                let d = &img.data[s.raw as usize..(s.raw + s.rawsize) as usize];
                for (i, w) in d.windows(4).enumerate().step_by(1) {
                    if w == needle && i % 4 == 0 {
                        println!("  als Zeiger in {} bei 0x{:X}", s.name, img.base + s.va + i as u32);
                    }
                }
            }
        }
        "xref" => {
            let a = parse_addr(&args[3]);
            for ins in &all {
                let hit = (0..ins.op_count()).any(|k| match ins.op_kind(k) {
                    OpKind::NearBranch32 => ins.near_branch_target() == a,
                    OpKind::Immediate32 | OpKind::Immediate8to32 => ins.immediate(k) == a,
                    OpKind::Memory => ins.memory_displacement64() == a,
                    _ => false,
                });
                if hit {
                    let f = func_of(ins.ip()).unwrap_or(0);
                    print!("  [f 0x{f:X}] ");
                    print_ins(&img, &mut fmt, ins);
                }
            }
        }
        "str" => {
            let needle = args[3].as_bytes();
            let d = &img.data;
            for i in 0..d.len().saturating_sub(needle.len()) {
                if &d[i..i + needle.len()] == needle && (i == 0 || d[i - 1] == 0) && d[i + needle.len()] == 0 {
                    let Some(sec) = img.sections.iter().find(|s| i as u32 >= s.raw && (i as u32) < s.raw + s.rawsize)
                    else {
                        continue;
                    };
                    let va = (img.base + sec.va + i as u32 - sec.raw) as u64;
                    println!("\"{}\" @ 0x{va:X}", args[3]);
                    for ins in all.iter().filter(|ins| {
                        (0..ins.op_count()).any(|k| match ins.op_kind(k) {
                            OpKind::Immediate32 => ins.immediate(k) == va,
                            OpKind::Memory => ins.memory_displacement64() == va,
                            _ => false,
                        })
                    }) {
                        let f = func_of(ins.ip()).unwrap_or(0);
                        print!("  [f 0x{f:X}] ");
                        print_ins(&img, &mut fmt, ins);
                    }
                }
            }
        }
        other => eprintln!("unbekannter Befehl {other}"),
    }
}
