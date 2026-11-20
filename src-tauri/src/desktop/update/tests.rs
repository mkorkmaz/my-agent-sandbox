use super::*;
use serde_json::json;

#[test]
fn only_production_identity_can_enable_the_updater() {
    assert!(production_identity(false, "sh.brig.desktop"));
    assert!(!production_identity(false, "sh.brig.desktop.dev"));
    assert!(!production_identity(true, "sh.brig.desktop"));
    assert!(!production_identity(true, "sh.brig.desktop.dev"));
}

#[test]
fn stable_precedence_rejects_equal_older_and_prerelease_versions() {
    let current = "0.4.1".parse().unwrap();
    for rejected in ["0.4.1", "0.4.0", "0.4.1+build.17", "0.4.2-build.18"] {
        assert!(
            !is_newer(&current, &rejected.parse().unwrap()),
            "{rejected}"
        );
    }
    assert!(is_newer(&current, &"0.4.2".parse().unwrap()));
}

#[test]
fn feed_requires_release_notes_and_the_arm64_artifact_at_the_expected_origin() {
    let endpoint = Url::parse(STABLE_FEED).unwrap();
    let manifest = json!({
        "version": "0.4.2",
        "notes": "Release notes",
        "platforms": { "darwin-aarch64": {
            "url": "https://github.com/reformo/tugboat/releases/download/v0.4.2-build.18/Tugboat.app.tar.gz",
            "signature": "fixture-signature"
        }}
    });
    assert!(read_manifest(&manifest, &endpoint).is_ok());
    for field in ["version", "notes", "platforms"] {
        let mut missing = manifest.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(read_manifest(&missing, &endpoint).is_err(), "{field}");
    }
    for field in ["url", "signature"] {
        let mut missing = manifest.clone();
        missing["platforms"][TARGET]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(read_manifest(&missing, &endpoint).is_err(), "{field}");
    }
    let mut other = manifest;
    other["platforms"][TARGET]["url"] = json!("https://example.org/Tugboat.app.tar.gz");
    assert!(read_manifest(&other, &endpoint).is_err());
}

#[cfg(target_arch = "aarch64")]
mod controlled_feed;
