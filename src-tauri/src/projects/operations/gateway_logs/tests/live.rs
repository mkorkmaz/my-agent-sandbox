use super::*;
use std::{env, os::unix::fs::MetadataExt, path::Path};

fn proof_directory() -> PathBuf {
    let root = PathBuf::from(
        env::var_os("TUGBOAT_GATEWAY_PROOF_ROOT").expect("Set the private proof directory."),
    );
    let temporary = Path::new("/tmp").canonicalize().unwrap();
    let metadata = fs::symlink_metadata(&root).expect("The proof directory must exist.");
    assert!(
        root != temporary && root.starts_with(&temporary) && root.canonicalize().unwrap() == root
    );
    assert!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
    );
    root
}

#[test]
#[ignore = "requires the active disposable gateway proof guest"]
fn reads_real_scratch_project_logs() {
    let root = proof_directory();
    let label = env::var("TUGBOAT_GATEWAY_PROOF_PROJECT").expect("Select proof project a or b.");
    assert!(matches!(label.as_str(), "a" | "b"));
    let folder = root.join(format!("project-{label}"));
    assert!(folder.is_dir() && folder.canonicalize().unwrap() == folder);
    let mut registry = Registry::default();
    let project = registry.register(&folder).unwrap();
    let agent_views = registry.agent_views(&project.id).unwrap();
    assert_eq!(agent_views[0].reference(), agent_views[1].reference());
    assert_eq!(agent_views[0].environment_id, agent_views[1].environment_id);
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/brig-runtime/bin");
    let mut brig = Brig {
        executable: runtime.join("brig"),
        environment: [
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", runtime.display()),
            ),
            (
                "BRIG_RUNTIME_BIN".into(),
                runtime.join("hull").to_string_lossy().into_owned(),
            ),
            ("BRIG_RUNTIME".into(), "hull".into()),
            ("BRIG_HYPERVISOR".into(), "hvi".into()),
            ("DO_NOT_TRACK".into(), "1".into()),
            ("TERM".into(), "dumb".into()),
        ]
        .into(),
    };
    for (key, name) in [
        ("HOME", "home"),
        ("BRIG_STATE_DIR", "state"),
        ("BRIG_PROFILE_DIR", "profiles"),
        ("BRIG_POLICY_DIR", "policies"),
        ("XDG_CONFIG_HOME", "config"),
        ("TMPDIR", "tmp"),
        ("DOCKER_CONFIG", "registry"),
    ] {
        let path = root.join(name);
        assert!(
            path.is_dir() && path.canonicalize().unwrap() == path,
            "Invalid proof directory {name}."
        );
        brig.environment
            .insert(key.into(), path.to_string_lossy().into_owned());
    }
    for selected in agent_views {
        assert_gateway_events(&brig, &selected);
    }
}

fn assert_gateway_events(brig: &Brig, project: &Project) {
    let GatewayLogs::Available { text } = read_registered(brig.clone(), project) else {
        panic!("Brig did not return a gateway log for the registered proof project.");
    };
    assert!(summary::can_copy(&text));
    for (key, expected) in [
        ("TUGBOAT_GATEWAY_PROOF_PORT", true),
        ("TUGBOAT_GATEWAY_PROOF_EXCLUDED_PORT", false),
    ] {
        if let Ok(port) = env::var(key) {
            let port = port.parse::<u16>().expect("Use a valid loopback port.");
            assert_ne!(port, 0);
            assert_eq!(
                text.contains(&format!("127.0.0.1:{port}=")),
                expected,
                "Unexpected loopback event for {key}."
            );
        }
    }
    println!(
        "Verified {} filtered lines for {} using {:?}.",
        text.lines().count(),
        project.reference(),
        project.agent_kind
    );
}
