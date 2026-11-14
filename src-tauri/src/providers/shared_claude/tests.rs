use super::{
    account::{self, Runtime},
    credentials::Credentials,
    http,
    store::{Account, Session, Store},
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tugboat-claude-account-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn records(&self) -> crate::storage::data_location::RecordsDirectory {
        crate::storage::data_location::RecordsDirectory::fixture(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn response() -> Value {
    json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600,"token_type":"Bearer","scope":"user:profile user:inference"})
}
fn profile() -> Value {
    json!({"account":{"uuid":"synthetic-user","email":"fixture@example.invalid"},"organization":{"uuid":"synthetic-organization","organization_type":"claude_pro","rate_limit_tier":"default_claude_pro"}})
}
fn credentials() -> Credentials {
    Credentials::from_response(response(), profile()).unwrap()
}

#[test]
fn private_claude_account_survives_restart_and_never_publishes_refresh_tokens() {
    let fixture = Fixture::new();
    let mut session = Session::empty();
    session.connect(credentials()).unwrap();
    Store::open(fixture.records())
        .unwrap()
        .save(&session)
        .unwrap();
    let restored = Store::open(fixture.records()).unwrap().load().unwrap();
    assert!(matches!(restored.account, Account::Connected(_)));
    let published = serde_json::to_string(&account::status(&restored)).unwrap();
    assert!(!published.contains("synthetic-refresh"));
    assert!(!published.contains("synthetic-access"));
    let guest = restored.account.credentials().unwrap().guest();
    assert!(guest["claudeAiOauth"]["refreshToken"].is_null());
    assert!(!serde_json::to_string(&guest)
        .unwrap()
        .contains("synthetic-refresh"));
    assert_eq!(
        fs::metadata(fixture.0.join("claude"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(fixture.0.join("claude/session.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(guest, restored.account.credentials().unwrap().guest());
}

#[test]
fn credential_boundaries_reject_missing_fields_bad_scopes_and_identity_changes() {
    for key in ["access_token", "refresh_token", "expires_in", "scope"] {
        let mut invalid = response();
        invalid.as_object_mut().unwrap().remove(key);
        assert!(Credentials::from_response(invalid, profile()).is_err());
    }
    for (key, value) in [
        ("access_token", json!("bad\r\nheader")),
        ("refresh_token", json!("x".repeat(32_769))),
        ("expires_in", json!(0)),
        ("expires_in", json!(i64::MAX)),
        ("scope", json!("user:profile")),
    ] {
        let mut invalid = response();
        invalid[key] = value;
        assert!(Credentials::from_response(invalid, profile()).is_err());
    }
    let mut changed = response();
    changed["account"] = json!({"uuid":"other-user"});
    assert!(credentials().renew(changed).is_err());
    let mut missing_identity = profile();
    missing_identity["organization"] = Value::Null;
    assert!(Credentials::from_response(response(), missing_identity).is_err());
}

#[test]
fn uncertain_refresh_requires_sign_in_and_cannot_replay_the_old_refresh_token() {
    let fixture = Fixture::new();
    let records = fixture.records();
    let store = Store::open(records.clone()).unwrap();
    let mut session = Session::empty();
    session.connect(credentials()).unwrap();
    store.save(&session).unwrap();
    let mut runtime = Runtime::default();
    assert!(account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        records.clone(),
        |_| Err(http::Failure::Uncertain)
    )
    .is_err());
    assert!(matches!(
        store.load().unwrap().account,
        Account::NeedsSignIn(_)
    ));
    assert!(account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        records,
        |_| panic!("An uncertain rotated grant must never be replayed")
    )
    .is_err());
}

#[test]
fn rotated_credentials_are_recovered_after_a_failed_private_record_write() {
    let fixture = Fixture::new();
    let records = fixture.records();
    let store = Store::open(records.clone()).unwrap();
    let mut session = Session::empty();
    session.connect(credentials()).unwrap();
    store.save(&session).unwrap();
    let file = fixture.0.join("claude/session.json");
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    let mut runtime = Runtime::default();
    let result = account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        records,
        |previous| {
            let mut renewed = response();
            renewed["refresh_token"] = json!("synthetic-rotated");
            previous
                .renew(renewed)
                .map_err(|_| http::Failure::Uncertain)
        },
    );
    assert!(result.is_err());
    fs::remove_dir(&file).unwrap();
    runtime.restore(&store).unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .account
            .credentials()
            .unwrap()
            .refresh_token,
        "synthetic-rotated"
    );
}

#[test]
fn transient_failures_preserve_the_account_and_stop_automatic_renewal_after_five_attempts() {
    let fixture = Fixture::new();
    let records = fixture.records();
    let store = Store::open(records.clone()).unwrap();
    let mut credential = credentials();
    credential.expires_at = chrono::Utc::now().timestamp_millis() + 1_000;
    let mut session = Session::empty();
    session.connect(credential).unwrap();
    store.save(&session).unwrap();
    let mut runtime = Runtime::default();
    for _ in 0..5 {
        assert!(account::refresh_with(
            &mut runtime,
            &store,
            &mut session,
            true,
            records.clone(),
            |_| Err(http::Failure::Temporary)
        )
        .is_err());
    }
    assert!(matches!(
        store.load().unwrap().account,
        Account::Connected(_)
    ));
    let error = account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        false,
        records.clone(),
        |_| panic!("Automatic renewal must stop at its retry limit"),
    )
    .err()
    .unwrap();
    assert!(error.contains("five failed attempts"));
    account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        records,
        |previous| {
            previous
                .renew(response())
                .map_err(|_| http::Failure::Uncertain)
        },
    )
    .unwrap();
    assert!(matches!(session.account, Account::Connected(_)));
}

#[test]
fn foreign_records_are_rejected_before_rotation_and_private_links_are_preserved() {
    let fixture = Fixture::new();
    let other = Fixture::new();
    let store = Store::open(fixture.records()).unwrap();
    let mut session = Session::empty();
    session.connect(credentials()).unwrap();
    store.save(&session).unwrap();
    assert!(account::refresh_with(
        &mut Runtime::default(),
        &store,
        &mut session,
        true,
        other.records(),
        |_| panic!("Do not rotate for a foreign dataset")
    )
    .is_err());
    let file = fixture.0.join("claude/session.json");
    fs::remove_file(&file).unwrap();
    fs::write(fixture.0.join("untouched"), b"private content").unwrap();
    symlink(fixture.0.join("untouched"), &file).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(&session).is_err());
    assert_eq!(
        fs::read(fixture.0.join("untouched")).unwrap(),
        b"private content"
    );
    drop(store);
    fs::remove_file(&file).unwrap();
    fs::rename(fixture.0.join("claude"), fixture.0.join("original")).unwrap();
    symlink(fixture.0.join("original"), fixture.0.join("claude")).unwrap();
    assert!(Store::open(fixture.records()).is_err());
}

#[test]
fn invalid_transitions_and_generation_overflow_preserve_the_previous_account() {
    let mut session = Session::empty();
    assert!(session.require_sign_in().is_err());
    session.connect(credentials()).unwrap();
    session.generation = u64::MAX;
    assert!(session.disconnect().is_err());
    assert!(matches!(session.account, Account::Connected(_)));
    assert!(session.require_sign_in().is_err());
    assert!(matches!(session.account, Account::Connected(_)));
    session.generation = 1;
    session.disconnect().unwrap();
    assert!(matches!(session.account, Account::Disconnected));
    assert!(session.account.credentials().is_none());
}
