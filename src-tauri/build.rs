fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") {
        cc::Build::new().file("src/fps_win.c").compile("fps_win");
        println!("cargo:rustc-link-lib=advapi32");
        println!("cargo:rerun-if-changed=src/fps_win.c");
    }
    tauri_build::build();
}
