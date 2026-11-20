use super::*;

#[test]
fn app_bundle_can_be_renamed_but_must_contain_the_macos_executable() {
    let path = Path::new("/Applications/Tugboat Renamed.app/Contents/MacOS/tugboat");
    assert_eq!(
        bundle_directory(path),
        Some(Path::new("/Applications/Tugboat Renamed.app"))
    );
    assert!(bundle_directory(Path::new("/work/target/debug/tugboat")).is_none());
}

#[test]
fn read_only_volumes_require_the_signed_installer_and_applications() {
    let error = require_writable_volume(true).unwrap_err();
    assert!(error.contains("read-only volume"));
    assert!(error.contains("https://github.com/reformo/tugboat/releases/latest"));
    assert!(error.contains("Applications"));
    require_writable_volume(false).unwrap();
}

#[test]
fn recovery_can_use_the_updaters_extensionless_backup_directory() {
    let installed =
        InstallLocation::for_executable("/Applications/Tugboat.app/Contents/MacOS/tugboat".into())
            .unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temporary = std::env::temp_dir().join(format!(
        "tugboat-update-backup-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&temporary).unwrap();
    let backup = temporary.join("current_app");
    fs::create_dir(&backup).unwrap();
    let recovered = installed.preserved_at(&backup).unwrap();
    assert_eq!(
        recovered.executable,
        backup
            .canonicalize()
            .unwrap()
            .join("Contents/MacOS/tugboat")
    );
    let symlink = temporary.join("backup-link");
    std::os::unix::fs::symlink(&backup, &symlink).unwrap();
    assert!(installed.preserved_at(&symlink).is_err());
    fs::remove_file(symlink).unwrap();
    fs::remove_dir(backup).unwrap();
    fs::remove_dir(temporary).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn recovery_uses_the_version_of_the_bundle_on_disk() {
    let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
        <plist version="1.0"><dict>
        <key>CFBundleIdentifier</key><string>sh.brig.desktop</string>
        <key>CFBundleExecutable</key><string>tugboat</string>
        <key>CFBundleShortVersionString</key><string>0.5.0</string>
        </dict></plist>"#;
    assert_eq!(
        read_version(plist, "sh.brig.desktop", "tugboat").unwrap(),
        "0.5.0"
    );
    assert!(read_version(plist, "sh.brig.desktop.dev", "tugboat").is_err());
    assert!(read_version(plist, "sh.brig.desktop", "another-app").is_err());
    assert!(read_version(b"<broken", "sh.brig.desktop", "tugboat").is_err());
}
