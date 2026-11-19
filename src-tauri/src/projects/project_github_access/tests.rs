use super::*;
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

mod live;

const PROJECT: &str = "0123456789abcdef0123456789abcdef";
const SIBLING: &str = "1123456789abcdef0123456789abcdef";
const TOKEN: &str = "github_pat_SYNTHETIC_credential_12345678";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            "/private/tmp/tgb-gh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_saved_scope_belongs_to_one_project_and_denies_extra_paths() {
    let grant = Grant::new(PROJECT, state::repository("demo/private").unwrap()).unwrap();
    assert!(grant.validate(PROJECT).is_ok());
    assert!(grant.validate(SIBLING).is_err());
    assert_ne!(
        Grant::new(PROJECT, "demo/private".into()).unwrap().name,
        grant.name
    );
    for invalid in [
        "github.com/demo/private",
        "demo/private.git?service=git-receive-pack",
        "demo/../private",
        "demo/repo\n",
        "repo",
        "-bad/repo",
    ] {
        assert!(state::repository(invalid).is_err());
    }
    let spec = grant.source();
    let route = &spec["spec"]["routes"][0];
    assert_eq!(
        route["request"]["allow"],
        json!({ "methods": ["GET", "POST"], "paths": ["/demo/private.git/info/refs", "/demo/private.git/git-upload-pack"], "repos": ["demo/private"] })
    );
    assert_eq!(route["credential"]["secretRef"]["name"], grant.name);
    assert!(!spec.to_string().contains(TOKEN));
}

#[test]
fn keychain_operations_send_the_token_only_to_stdin_and_mask_errors() {
    let fixture = Fixture::new();
    let executable = fixture.0.join("brig");
    fs::write(
        &executable,
        r#"#!/bin/sh
set -eu
root=${0%/*}
printf '%s\n' "$*" >> "$root/calls"
case "$1:$2" in
    --json:secret)
        test "$3" = ls
        names='[]'
        if test -f "$root/name"; then names="[{\"name\":\"$(/bin/cat "$root/name")\"}]"; fi
        printf '{"apiVersion":"brig.sh/v1alpha1","kind":"SecretList","data":%s}\n' "$names"
        ;;
    secret:create|secret:update)
        IFS= read -r token
        case "$token" in github_pat_SYNTHETIC_credential_*) ;; *) exit 8 ;; esac
        if test -f "$root/fail"; then printf '%s\n' "$token" >&2; exit 1; fi
        printf '%s' "$3" > "$root/name"
        ;;
    secret:delete)
        test "$4" = -y
        /bin/rm -f "$root/name"
        ;;
    *) exit 9 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let grant = Grant::new(PROJECT, "demo/private".into()).unwrap();
    let keychain = Keychain {
        executable,
        environment: Default::default(),
    };
    assert!(!keychain.contains(&grant.name).unwrap());
    fs::write(fixture.0.join("fail"), "").unwrap();
    let error = keychain
        .write(&grant.name, keychain::token(TOKEN.into()).unwrap(), false)
        .unwrap_err();
    assert!(error.contains("Mac Keychain"));
    assert!(!error.contains(TOKEN));
    fs::remove_file(fixture.0.join("fail")).unwrap();
    keychain
        .write(&grant.name, keychain::token(TOKEN.into()).unwrap(), false)
        .unwrap();
    keychain
        .write(&grant.name, keychain::token(TOKEN.into()).unwrap(), true)
        .unwrap();
    keychain.delete(&grant.name).unwrap();
    keychain.delete(&grant.name).unwrap();
    let calls = fs::read_to_string(fixture.0.join("calls")).unwrap();
    assert!(!calls.contains(TOKEN));
    assert!(calls
        .lines()
        .any(|line| line == format!("secret update {}", grant.name)));
    assert!(keychain::token("value with whitespace".into()).is_err());
}

#[test]
fn revokes_one_broker_sandbox_and_verifies_the_sibling_is_untouched() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let fixture = Fixture::new();
    let directory = fixture.0.join("host/broker");
    fs::create_dir_all(&directory).unwrap();
    let socket = directory.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let worker = std::thread::spawn(move || {
        for (request, response) in [
            (
                "GET /status HTTP/1.0",
                json!({"sandboxes":["brig-primary", "brig-sibling"]}),
            ),
            (
                "DELETE /sandboxes/brig-primary HTTP/1.0",
                json!({"removed":true,"remaining":1}),
            ),
            (
                "GET /status HTTP/1.0",
                json!({"sandboxes":["brig-sibling"]}),
            ),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&mut stream).read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), request);
            write!(
                stream,
                "HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n{response}"
            )
            .unwrap();
        }
    });
    let brig = Brig {
        executable: PathBuf::new(),
        environment: [(
            "BRIG_STATE_DIR".into(),
            fixture.0.to_string_lossy().into_owned(),
        )]
        .into(),
    };
    broker::revoke(&brig, "brig-primary").unwrap();
    worker.join().unwrap();
    fs::remove_file(socket).unwrap();
    broker::revoke(&brig, "brig-primary").unwrap();
    assert!(broker::revoke(&brig, "brig-primary/../brig-sibling").is_err());
}

#[test]
#[ignore = "requires the pinned Brig runtime and a writable host login Keychain"]
fn uses_mac_keychain_for_one_synthetic_grant_and_removes_it() {
    struct Cleanup<'a>(&'a Keychain, &'a str);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            assert!(
                self.0.delete(self.1).is_ok(),
                "Could not remove the synthetic test grant."
            );
        }
    }
    let keychain = Keychain::host().unwrap();
    let grant = Grant::new(PROJECT, "demo/private".into()).unwrap();
    assert!(!keychain.contains(&grant.name).unwrap());
    let _cleanup = Cleanup(&keychain, &grant.name);
    keychain
        .write(&grant.name, keychain::token(TOKEN.into()).unwrap(), false)
        .unwrap();
    keychain
        .write(
            &grant.name,
            keychain::token(format!("{TOKEN}second")).unwrap(),
            true,
        )
        .unwrap();
    keychain.delete(&grant.name).unwrap();
    assert!(!keychain.contains(&grant.name).unwrap());
}

#[test]
fn project_removal_retries_keychain_revocation_before_erasing_only_its_registration() {
    let fixture = Fixture::new();
    let mut registry = Registry::default();
    for name in ["a", "b"] {
        fs::create_dir(fixture.0.join(name)).unwrap();
    }
    let first = registry.register(&fixture.0.join("a")).unwrap();
    let sibling = registry.register(&fixture.0.join("b")).unwrap();
    let file = fixture.0.join("projects.json");
    let mut first_grant = Grant::new(&first.id, "demo/first".into()).unwrap();
    first_grant.phase = Phase::Saved;
    let mut sibling_grant = Grant::new(&sibling.id, "demo/sibling".into()).unwrap();
    sibling_grant.phase = Phase::Saved;
    persist(&mut registry, &file, &first, Some(&first_grant)).unwrap();
    persist(&mut registry, &file, &sibling, Some(&sibling_grant)).unwrap();
    let script = fixture.0.join("brig");
    let marker = fixture.0.join("pending-deletion");
    fs::write(&marker, "synthetic timeout").unwrap();
    fs::write(&script, r#"#!/bin/sh
set -eu
root=${0%/*}
case "$1:$2" in
    --json:plan)
        for value do
            if [ "${previous:-}" = --home ]; then home=$value; fi
            reference=${previous:-}
            previous=$value
        done
        printf '{"apiVersion":"brig.sh/v1alpha1","kind":"Plan","data":{"profile":"%s","session":"%s","sandbox":"brig-fixture","project":{"host":"%s","guest":"/work/a"},"home":{"host":"%s-%s","mode":"read-write","guest":"/root"},"runtime":{"kind":"hull","backend":"hvi","available":true},"policies":[]}}' "${reference%@*}" "${reference#*@}" "$previous" "$home" "${reference#*@}"
        ;;
    --json:ls)
        printf '%s' '{"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"runtime":{"kind":"hull","available":true},"sandboxes":[]}}'
        ;;
    policy:show) printf '%s' 'unknown policy' >&2; exit 1 ;;
    secret:delete)
        printf '%s\n' "$3" >> "$root/removals"
        test ! -f "$root/pending-deletion"
        ;;
    --json:secret)
        printf '%s' '{"apiVersion":"brig.sh/v1alpha1","kind":"SecretList","data":[]}'
        ;;
    *) exit 9 ;;
esac
"#).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let brig = Brig {
        executable: script.clone(),
        environment: [(
            "BRIG_STATE_DIR".into(),
            fixture.0.to_string_lossy().into_owned(),
        )]
        .into(),
    };
    let settings = settings::Settings::default();
    let keychain = || {
        Ok(Keychain {
            executable: script.clone(),
            environment: Default::default(),
        })
    };
    assert!(
        revoke_for_removal_with(&brig, &file, &first, &settings, keychain)
            .unwrap_err()
            .contains("Mac Keychain")
    );
    let mut pending = Registry::load(&file).unwrap();
    assert_eq!(
        pending
            .project(&first.id)
            .unwrap()
            .github_access
            .unwrap()
            .phase,
        Phase::Revoking
    );
    assert!(pending
        .remove_environment(&first.id, &first.environment_id)
        .is_err());
    assert_eq!(
        pending.project(&sibling.id).unwrap().github_access,
        Some(sibling_grant.clone())
    );
    let first_pending = pending.project(&first.id).unwrap();
    let progress = apply(
        &mut pending,
        &file,
        &first_pending,
        &settings,
        &brig,
        &keychain().unwrap(),
        Action::Read,
        None,
    )
    .unwrap();
    assert_eq!(progress.status, Status::RevokedPendingCleanup);
    fs::remove_file(&marker).unwrap();
    revoke_for_removal_with(&brig, &file, &first, &settings, keychain).unwrap();
    let mut resolved = Registry::load(&file).unwrap();
    assert!(resolved.project(&first.id).unwrap().github_access.is_none());
    resolved
        .remove_environment(&first.id, &first.environment_id)
        .unwrap();
    assert_eq!(
        resolved.project(&sibling.id).unwrap().github_access,
        Some(sibling_grant)
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("removals")).unwrap(),
        format!("{0}\n{0}\n", first_grant.name)
    );
}
