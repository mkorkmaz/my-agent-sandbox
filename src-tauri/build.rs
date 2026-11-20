fn configure_development_identity() {
    if !tauri_build::is_dev() {
        return;
    }
    let mut config: serde_json::Map<String, serde_json::Value> =
        match std::env::var_os("TAURI_CONFIG") {
            Some(overrides) => serde_json::from_slice(overrides.as_encoded_bytes())
                .expect("TAURI_CONFIG must be a JSON object"),
            None => serde_json::Map::new(),
        };
    config.insert("productName".into(), "Tugboat Dev".into());
    config.insert("identifier".into(), "sh.brig.desktop.dev".into());
    let config = serde_json::Value::Object(config);
    // tauri-build and generate_context! run in separate processes.
    std::env::set_var("TAURI_CONFIG", config.to_string());
    println!("cargo:rustc-env=TAURI_CONFIG={config}");
}

fn main() {
    let install = std::path::Path::new("../.build/libghostty/install");
    assert!(
        install.join("lib/libghostty-vt.a").exists(),
        "Run npm run native:prepare first"
    );
    cc::Build::new()
        .file("native/TerminalView.m")
        .file("native/TerminalDrawing.m")
        .file("native/TerminalInput.m")
        .file("native/WindowAppearance.m")
        .file("native/ApplicationQuit.m")
        .include(install.join("include"))
        .flag("-fobjc-arc")
        .flag("-mmacosx-version-min=15.0")
        .compile("brig_terminal_view");
    println!(
        "cargo:rustc-link-search=native={}",
        install.join("lib").canonicalize().unwrap().display()
    );
    // Keep a distinct archive name so Darwin's linker cannot prefer the sibling dylib.
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::copy(
        install.join("lib/libghostty-vt.a"),
        output.join("libbrig_ghostty_vt.a"),
    )
    .unwrap();
    println!("cargo:rustc-link-lib=static=brig_ghostty_vt");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=CoreText");
    // AppKit method categories live in separate archive members.
    println!("cargo:rustc-link-arg=-Wl,-ObjC");
    println!("cargo:rerun-if-changed=native/TerminalView.m");
    println!("cargo:rerun-if-changed=native/TerminalView.h");
    println!("cargo:rerun-if-changed=native/TerminalDrawing.m");
    println!("cargo:rerun-if-changed=native/TerminalInput.m");
    println!("cargo:rerun-if-changed=native/TerminalViewPrivate.h");
    println!("cargo:rerun-if-changed=native/TerminalTheme.h");
    println!("cargo:rerun-if-changed=native/WindowAppearance.m");
    println!("cargo:rerun-if-changed=native/WindowAppearance.h");
    println!("cargo:rerun-if-changed=native/ApplicationQuit.m");
    configure_development_identity();
    tauri_build::build();
}
