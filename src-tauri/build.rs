use std::path::Path;

fn main() {
    // tauri.windows.conf.json bundles ../engine/runtime/, which engine/stage-runtime.sh produces
    // after the MSYS2 engine build. tauri_build rejects a missing resource path, so a fresh clone
    // could not even `cargo test`. An empty directory bundles nothing; the app then falls back to
    // OAP_ENGINE or engine/dist (see find_engine in src/lib.rs).
    let runtime = Path::new("../engine/runtime");
    println!("cargo:rerun-if-changed=../engine/runtime");
    if !runtime.exists() {
        std::fs::create_dir_all(runtime).expect("create engine/runtime");
    }
    let engine = runtime.join("bin").join("uxplay.exe");
    if cfg!(windows) && std::env::var("PROFILE").as_deref() == Ok("release") && !engine.is_file() {
        println!("cargo:warning=engine/runtime has no bin/uxplay.exe: the installer will ship without the receiver engine. Run engine/build-engine.sh first.");
    }
    tauri_build::build()
}
