use super::*;
use serde_json::json;

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("tugboat-runtime-{name}-{}", std::process::id()));
    fs::create_dir_all(root.join("bin")).unwrap();
    let digest = crate::hash::hex(Sha256::digest(b"runtime fixture"));
    let mut tools = serde_json::Map::new();
    for name in TOOLS {
        let file = root.join("bin").join(name);
        fs::write(&file, "runtime fixture").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
        tools.insert(name.into(), json!({"sha256":digest}));
    }
    fs::write(root.join("mke2fs.conf"), "runtime fixture").unwrap();
    fs::create_dir_all(root.join("registry")).unwrap();
    fs::write(root.join("registry/config.json"), "runtime fixture").unwrap();
    let manifest = json!({
        "schemaVersion":1,"brigVersion":"v0.5.2","tools":tools,"libraries":[],
        "configuration":{"mke2fs.conf":{"sha256":digest},"registry/config.json":{"sha256":digest}}
    });
    fs::write(root.join("build.json"), manifest.to_string()).unwrap();
    root
}

#[test]
fn complete_bundle_rejects_missing_altered_and_nonexecutable_tools() {
    let root = fixture("tools");
    assert!(validate_at(&root.join("bin")).is_ok());
    fs::remove_file(root.join("bin/hvi")).unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("hvi"));
    fs::write(root.join("bin/hvi"), "runtime fixture").unwrap();
    fs::set_permissions(root.join("bin/hvi"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("bin/cosign"), "damaged fixture").unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("cosign SHA-256"));
    fs::write(root.join("bin/cosign"), "runtime fixture").unwrap();
    fs::set_permissions(root.join("bin/brig"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("brig executable"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn configuration_and_library_paths_are_checked_before_launch() {
    let root = fixture("configuration");
    fs::write(root.join("mke2fs.conf"), "damaged fixture").unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("mke2fs.conf SHA-256"));
    fs::write(root.join("mke2fs.conf"), "runtime fixture").unwrap();
    fs::write(root.join("registry/config.json"), "{\"credsStore\":\"missing-helper\"}").unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("config.json SHA-256"));
    fs::remove_file(root.join("registry/config.json")).unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("config.json"));
    fs::write(root.join("registry/config.json"), "runtime fixture").unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(root.join("build.json")).unwrap()).unwrap();
    manifest["libraries"] = json!([{"name":"../outside.dylib","sha256":"a".repeat(64)}]);
    fs::write(root.join("build.json"), manifest.to_string()).unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("invalid library name"));
    manifest["libraries"] = json!([{"name":"missing.dylib","sha256":"a".repeat(64)}]);
    fs::write(root.join("build.json"), manifest.to_string()).unwrap();
    assert!(validate_at(&root.join("bin")).unwrap_err().contains("missing.dylib"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bundle_paths_preserve_spaces_and_do_not_use_a_system_runtime() {
    let directory = directory_for(Path::new("/Applications/My Apps/Tugboat.app/Contents/MacOS/tugboat")).unwrap();
    assert_eq!(directory, Path::new("/Applications/My Apps/Tugboat.app/Contents/MacOS"));
    assert_eq!(data_directory(&directory).unwrap(), Path::new("/Applications/My Apps/Tugboat.app/Contents/Resources/runtime"));
    assert_eq!(data_directory(Path::new("/fixture/.build/brig-runtime/bin")).unwrap(), Path::new("/fixture/.build/brig-runtime"));
}
