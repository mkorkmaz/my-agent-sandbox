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
    tauri_build::build();
}
