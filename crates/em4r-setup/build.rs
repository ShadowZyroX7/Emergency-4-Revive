//! Embeds the mod files into the setup exe and adds an `asInvoker` manifest.
//!
//! `EM4R_PAYLOAD_DIR` (set by scripts/package.ps1) names the folder with the
//! freshly built files. Without it, the exe is built with an empty payload so
//! that plain workspace builds and tests keep working; such an exe refuses to
//! install anything.
//!
//! The manifest matters: Windows auto-elevates 32-bit programs with "setup" in
//! their name unless they declare an execution level. We must start
//! unelevated so that registry entries, shortcuts and the launcher restart
//! belong to the real user; only the file copy is elevated when necessary.

use std::{env, fs, path::Path};

const FILES: [(&str, &str); 3] =
    [("DINPUT8", "dinput8.dll"), ("LAUNCHER", "EM4Revive.exe"), ("DEFAULT_CONFIG", "em4revive.toml")];

fn main() {
    println!("cargo:rerun-if-env-changed=EM4R_PAYLOAD_DIR");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("payload.rs");
    let mut code = String::new();
    match env::var("EM4R_PAYLOAD_DIR") {
        Ok(dir) => {
            for (name, file) in FILES {
                let path = fs::canonicalize(Path::new(&dir).join(file))
                    .unwrap_or_else(|e| panic!("Payload-Datei {file} fehlt in {dir}: {e}"));
                println!("cargo:rerun-if-changed={}", path.display());
                code += &format!("pub const {name}: &[u8] = include_bytes!({:?});\n", path);
            }
        }
        Err(_) => {
            for (name, _) in FILES {
                code += &format!("pub const {name}: &[u8] = &[];\n");
            }
        }
    }
    fs::write(out, code).unwrap();

    if env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("EM4Revive.Setup"))
            .expect("Manifest konnte nicht eingebettet werden");
    }
}
