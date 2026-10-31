fn main() {
    // On 32-bit Windows, stdcall functions are decorated (`_Name@20`).
    // Em4.exe imports the *undecorated* name, so export it explicitly.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("i686-") && target.contains("windows-msvc") {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:DirectInput8Create=_DirectInput8Create@20");
    } else if target.contains("windows-msvc") {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:DirectInput8Create");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
