use super::{
    credentials::Credentials,
    store::{Binding, Session, Store},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tugboat-auth-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}

impl Fixture {
    fn records(&self) -> (Store, crate::storage::data_location::RecordsDirectory) {
        let records = self.0.join("records");
        fs::create_dir(&records).unwrap();
        let guard = crate::storage::data_location::RecordsDirectory::fixture(&records);
        (Store::open(&records).unwrap(), guard)
    }

    fn project(&self, name: &str) -> crate::environment::CheckedProject {
        let path = self.0.join(name);
        let home = self.0.join(format!("{name}-home"));
        fs::create_dir(&path).unwrap();
        fs::create_dir(&home).unwrap();
        let executable = self.0.join(format!("{name}-brig"));
        fs::write(&executable, "#!/bin/sh\nif [ \"$1\" = agent ]; then printf '%s' '{\"env\":[],\"volumes\":[],\"files\":[]}'; exit 0; fi\nprintf '%s' '{\"apiVersion\":\"brig.sh/v1alpha1\",\"kind\":\"SandboxList\",\"data\":{\"sandboxes\":[]}}'\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let project = crate::projects::Registry::default()
            .register(&path, "codex")
            .unwrap();
        let plan = json!({"data":{"home":{"host":home,"guest":"/root"},"project":{"guest":format!("/work/{name}")}}});
        crate::environment::CheckedProject {
            project,
            settings: crate::desktop::settings::Settings::default(),
            brig: crate::platform::brig::Brig {
                executable,
                environment: crate::platform::command::environment(),
            },
            paths: crate::platform::brig::PlanPaths::from_plan(&plan).unwrap(),
            plan,
            fingerprint: "fixture".into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn jwt(value: Value) -> String {
    format!(
        "e30.{}.synthetic",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
    )
}
fn credential_value(account: &str) -> Value {
    json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{
        "id_token":jwt(json!({"email":"fixture@example.invalid","https://api.openai.com/auth":{"chatgpt_account_id":account}})),
        "access_token":jwt(json!({"exp":4102444800i64,"https://api.openai.com/auth":{"chatgpt_account_id":account}})),
        "refresh_token":"synthetic-private-refresh", "account_id":account,
        "oauth_provenance":{"issuer":"https://auth.openai.com","client_id":"fixture-client"},
        "login_owner_id":"fixture-owner"},"last_refresh":"2026-11-12T00:00:00Z"})
}

#[test]
fn private_session_survives_reopen_without_copying_credentials_to_public_state() {
    let fixture = Fixture::new();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    session.generation = 7;
    Store::open(&fixture.0).unwrap().save(&session).unwrap();
    let restored = Store::open(&fixture.0).unwrap().load().unwrap();
    assert_eq!(restored.generation, 7);
    let credentials = restored.credentials.unwrap();
    assert_eq!(credentials.account().unwrap(), "account-a");
    assert_eq!(
        credentials.email().as_deref(),
        Some("fixture@example.invalid")
    );
    assert_eq!(
        fs::metadata(fixture.0.join("chatgpt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(fixture.0.join("chatgpt/session.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn guest_copy_contains_no_refresh_token_and_preserves_source_credentials() {
    let source = credential_value("account-a");
    let credentials = Credentials::parse(source.clone(), true).unwrap();
    let guest = credentials.guest();
    assert_eq!(guest["tokens"]["refresh_token"], "");
    for key in [
        "access_token",
        "id_token",
        "account_id",
        "login_owner_id",
        "oauth_provenance",
    ] {
        assert_eq!(guest["tokens"][key], source["tokens"][key]);
    }
    assert!(Credentials::parse(guest.clone(), false).is_ok());
    assert!(Credentials::parse(guest, true).is_err());
    assert_eq!(
        credentials.token("refresh_token").unwrap(),
        "synthetic-private-refresh"
    );
}

#[test]
fn invalid_api_key_authority_account_and_token_inputs_have_redacted_errors() {
    let source = credential_value("account-a");
    let mut invalid = Vec::new();
    let mut value = source.clone();
    value["OPENAI_API_KEY"] = "sk-synthetic-secret".into();
    invalid.push(value);
    let mut value = source.clone();
    value["tokens"]["oauth_provenance"]["issuer"] = "https://attacker.invalid".into();
    invalid.push(value);
    let mut value = source.clone();
    value["tokens"]["account_id"] = "another-account".into();
    invalid.push(value);
    let mut value = source.clone();
    value["tokens"]["id_token"] = "synthetic-private-refresh".into();
    invalid.push(value);
    let mut value = source.clone();
    value["tokens"]["access_token"] = jwt(json!({"exp":"not-an-integer"})).into();
    invalid.push(value);
    let mut value = source;
    value["tokens"]["refresh_token"] = "".into();
    invalid.push(value);
    for value in invalid {
        let error = Credentials::parse(value, true).err().unwrap();
        assert!(!error.contains("synthetic"));
        assert!(!error.contains("attacker"));
        assert!(!error.contains("sk-"));
    }
}

#[test]
fn renewal_preserves_login_authority_and_rejects_identity_changes() {
    let credentials = Credentials::parse(credential_value("account-a"), true).unwrap();
    let access = jwt(json!({"exp":4102444900i64}));
    let renewed = credentials
        .renew(json!({"access_token":access,"refresh_token":"synthetic-rotated"}))
        .unwrap();
    assert_eq!(renewed.token("refresh_token").unwrap(), "synthetic-rotated");
    assert_eq!(renewed.client().unwrap(), "fixture-client");
    assert_eq!(renewed.account().unwrap(), "account-a");
    assert!(credentials.renew(json!({"access_token":access,"id_token":credential_value("account-b")["tokens"]["id_token"]})).is_err());
    assert!(credentials.renew(json!({"access_token":null})).is_err());
}

#[test]
fn damaged_future_and_invalid_binding_records_are_preserved() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let file = fixture.0.join("chatgpt/session.json");
    for contents in [
        "{bad synthetic-private-refresh",
        "{\"schemaVersion\":2,\"synthetic\":true}",
    ] {
        fs::write(&file, contents).unwrap();
        let error = store.load().err().unwrap();
        assert!(!error.contains("synthetic"));
        assert_eq!(fs::read_to_string(&file).unwrap(), contents);
        assert!(store.save(&Session::default()).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), contents);
    }
    let mut state = Session::default();
    state.bindings.push(Binding {
        project_id: "a".repeat(32),
        directory: PathBuf::from("/tmp/../../outside/.codex"),
        auth_hash: None,
        access_hash: None,
        pending: true,
        identity: (1, 1),
    });
    assert!(store.save(&state).is_err());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "{\"schemaVersion\":2,\"synthetic\":true}"
    );
}

#[test]
fn private_folder_and_auth_file_symlinks_cannot_redirect_storage() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, fixture.0.join("chatgpt")).unwrap();
    assert!(Store::open(&fixture.0).is_err());
    fs::remove_file(fixture.0.join("chatgpt")).unwrap();
    let store = Store::open(&fixture.0).unwrap();
    let target = outside.join("sentinel.json");
    fs::write(&target, "preserve").unwrap();
    symlink(&target, fixture.0.join("chatgpt/session.json")).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(&Session::default()).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
}

#[test]
fn missing_root_and_replaced_directory_never_recreate_or_redirect_a_record() {
    let fixture = Fixture::new();
    let records = fixture.0.join("records");
    fs::create_dir(&records).unwrap();
    let store = Store::open(&records).unwrap();
    fs::rename(&records, fixture.0.join("unmounted")).unwrap();
    assert!(store.save(&Session::default()).is_err());
    assert!(!records.exists());
    assert!(store.load().is_err());
    fs::create_dir(&records).unwrap();
    symlink(fixture.0.join("unmounted/chatgpt"), records.join("chatgpt")).unwrap();
    assert!(store.save(&Session::default()).is_err());
    fs::remove_file(records.join("chatgpt")).unwrap();
    fs::create_dir(records.join("chatgpt")).unwrap();
    assert!(store.save(&Session::default()).is_err());
    assert!(store.load().is_err());
}

#[test]
fn concurrent_record_updates_are_serialized_and_keep_every_generation() {
    let fixture = Fixture::new();
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let root = fixture.0.clone();
            std::thread::spawn(move || {
                let store = Store::open(&root).unwrap();
                let mut session = store.load().unwrap();
                session.generation += 1;
                store.save(&session).unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(
        Store::open(&fixture.0).unwrap().load().unwrap().generation,
        12
    );
}

#[test]
fn oversized_and_nonregular_records_are_rejected_without_waiting_for_input() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let file = fixture.0.join("chatgpt/session.json");
    fs::write(&file, vec![b'x'; 1_048_577]).unwrap();
    assert!(store.load().is_err());
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    assert!(store.load().is_err());
}

#[test]
fn workspace_exchange_keeps_the_selected_account_and_rejects_a_different_user() {
    let mut value = credential_value("original");
    value["tokens"]["id_token"] = jwt(json!({"https://api.openai.com/auth":{"chatgpt_account_id":"original","chatgpt_user_id":"user-a"}})).into();
    value["tokens"]["account_id"] = "workspace".into();
    value["tokens"]["access_token"] = jwt(json!({"exp":4102444800i64,"https://api.openai.com/auth":{"chatgpt_account_id":"workspace","chatgpt_user_id":"user-a"}})).into();
    let credentials = Credentials::parse(value.clone(), true).unwrap();
    assert_eq!(credentials.account().unwrap(), "workspace");
    value["tokens"]["access_token"] = jwt(json!({"exp":4102444800i64,"https://api.openai.com/auth":{"chatgpt_account_id":"workspace","chatgpt_user_id":"user-b"}})).into();
    assert!(Credentials::parse(value, true).is_err());
}

#[test]
fn adoption_and_renewal_update_only_auth_and_remove_guest_refresh_tokens() {
    let fixture = Fixture::new();
    let (store, guard) = fixture.records();
    let first = fixture.project("first");
    let second = fixture.project("second");
    let first_directory = super::guest::directory(&first).unwrap();
    let second_directory = super::guest::directory(&second).unwrap();
    fs::write(
        first_directory.join("auth.json"),
        serde_json::to_vec(&credential_value("account-a")).unwrap(),
    )
    .unwrap();
    for directory in [&first_directory, &second_directory] {
        fs::write(directory.join("config.toml"), "model = 'fixture'").unwrap();
        fs::create_dir(directory.join("sessions")).unwrap();
        fs::write(
            directory.join("sessions/history.jsonl"),
            "existing chat history",
        )
        .unwrap();
    }
    let mut session = Session::default();
    super::account::adopt(
        &store,
        &mut session,
        &first,
        super::guest::read(&first_directory).unwrap().unwrap(),
    )
    .unwrap();
    super::guest::bind(&mut session, &second, second_directory.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    let mut runtime = super::account::Runtime::default();
    let renewed_access = jwt(json!({"exp":4102444900i64}));
    super::account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        guard,
        |credentials| {
            credentials
                .renew(json!({"access_token":renewed_access,"refresh_token":"synthetic-new"}))
                .map_err(|_| super::http::Failure::SignIn)
        },
    )
    .unwrap();
    for directory in [&first_directory, &second_directory] {
        let guest: Value =
            serde_json::from_slice(&fs::read(directory.join("auth.json")).unwrap()).unwrap();
        assert_eq!(guest["tokens"]["refresh_token"], "");
        assert_eq!(guest["tokens"]["access_token"], renewed_access);
        assert_eq!(
            fs::read_to_string(directory.join("config.toml")).unwrap(),
            "model = 'fixture'"
        );
        assert_eq!(
            fs::read_to_string(directory.join("sessions/history.jsonl")).unwrap(),
            "existing chat history"
        );
    }
    assert_eq!(
        store
            .load()
            .unwrap()
            .credentials
            .unwrap()
            .token("refresh_token")
            .unwrap(),
        "synthetic-new"
    );
}

#[test]
fn interrupted_source_export_resumes_and_preserves_an_unexpected_auth_file() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("source");
    let directory = super::guest::directory(&checked).unwrap();
    fs::write(
        directory.join("auth.json"),
        serde_json::to_vec(&credential_value("account-a")).unwrap(),
    )
    .unwrap();
    let mut session = Session::default();
    session.credentials = super::guest::read(&directory).unwrap();
    super::guest::bind(&mut session, &checked, directory.clone()).unwrap();
    store.save(&session).unwrap();
    // Simulate a crash after atomic guest write and before its binding was committed.
    fs::write(
        directory.join("auth.json"),
        serde_json::to_vec(&session.credentials.as_ref().unwrap().guest()).unwrap(),
    )
    .unwrap();
    let mut reopened = store.load().unwrap();
    super::guest::publish(&store, &mut reopened).unwrap();
    assert!(!store.load().unwrap().bindings[0].pending);
    let foreign = serde_json::to_vec(&credential_value("account-b")).unwrap();
    fs::write(directory.join("auth.json"), &foreign).unwrap();
    super::guest::publish(&store, &mut reopened).unwrap();
    assert_eq!(fs::read(directory.join("auth.json")).unwrap(), foreign);
    super::guest::cleanup(&mut reopened).unwrap();
    assert_eq!(fs::read(directory.join("auth.json")).unwrap(), foreign);
}

fn staged_login(
    fixture: &Fixture,
    store: &Store,
    session: &mut Session,
    checked: &crate::environment::CheckedProject,
) -> super::store::PendingLogin {
    let nonce = "b".repeat(32);
    let (directory, guest) = super::guest::login_directory(checked, &nonce).unwrap();
    assert_eq!(guest, format!("/root/.tugboat-chatgpt-login/{nonce}"));
    let pending = super::store::PendingLogin {
        project_id: checked.project.id.clone(),
        nonce,
        identity: super::guest::identity(&directory).unwrap(),
        directory,
        imported: false,
        cancelled: false,
    };
    fs::write(
        pending.directory.join("auth.json"),
        serde_json::to_vec(&credential_value("account-b")).unwrap(),
    )
    .unwrap();
    session.pending_login = Some(pending.clone());
    store.save(session).unwrap();
    assert!(pending.directory.starts_with(&fixture.0));
    pending
}

#[test]
fn cancelled_and_stale_logins_preserve_the_old_account_and_project_files() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("login");
    let directory = super::guest::directory(&checked).unwrap();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &checked, directory.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    let original = fs::read(directory.join("auth.json")).unwrap();
    fs::write(directory.join("history.jsonl"), "preserve").unwrap();
    let pending = staged_login(&fixture, &store, &mut session, &checked);
    super::account::complete_login(
        &store,
        &mut session,
        &checked.project.id,
        &"c".repeat(32),
        true,
        Some(&checked),
    )
    .unwrap();
    assert!(session.pending_login.is_some());
    assert!(pending.directory.exists());
    // The terminal can report success after the user cancelled it.
    session.pending_login.as_mut().unwrap().cancelled = true;
    store.save(&session).unwrap();
    super::account::complete_login(
        &store,
        &mut session,
        &checked.project.id,
        &pending.nonce,
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        session.credentials.as_ref().unwrap().account().unwrap(),
        "account-a"
    );
    assert_eq!(fs::read(directory.join("auth.json")).unwrap(), original);
    assert_eq!(
        fs::read_to_string(directory.join("history.jsonl")).unwrap(),
        "preserve"
    );
    assert!(!pending.directory.exists());
    assert!(store.load().unwrap().pending_login.is_none());
}

#[test]
fn completed_login_recovers_after_import_and_after_staging_cleanup() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("login");
    let mut session = Session::default();
    let mut pending = staged_login(&fixture, &store, &mut session, &checked);
    // Persisted import, with the guest export still pending.
    session.credentials = super::guest::read(&pending.directory).unwrap();
    pending.imported = true;
    session.pending_login = Some(pending.clone());
    super::guest::bind(
        &mut session,
        &checked,
        super::guest::directory(&checked).unwrap(),
    )
    .unwrap();
    store.save(&session).unwrap();
    super::guest::remove_login_directory(&pending).unwrap();
    let mut recovered = store.load().unwrap();
    super::account::complete_login(
        &store,
        &mut recovered,
        &checked.project.id,
        &pending.nonce,
        true,
        None,
    )
    .unwrap();
    assert!(recovered.pending_login.is_none());
    assert_eq!(
        recovered.credentials.unwrap().account().unwrap(),
        "account-b"
    );
    let auth: Value = serde_json::from_slice(
        &fs::read(super::guest::directory(&checked).unwrap().join("auth.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(auth["tokens"]["account_id"], "account-b");
    assert_eq!(auth["tokens"]["refresh_token"], "");
}

#[test]
fn rotated_credentials_survive_disk_loss_and_never_redirect_to_another_root() {
    let fixture = Fixture::new();
    let (store, guard) = fixture.records();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    store.save(&session).unwrap();
    let mut runtime = super::account::Runtime::default();
    let records = fixture.0.join("records");
    let offline = fixture.0.join("offline");
    let result = super::account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        guard,
        |credentials| {
            fs::rename(&records, &offline).unwrap();
            credentials.renew(json!({"access_token":jwt(json!({"exp":4102444900i64})),"refresh_token":"synthetic-rotated"})).map_err(|_| super::http::Failure::SignIn)
        },
    );
    assert!(result.is_err());
    assert!(!records.exists());
    drop(store);
    let other = fixture.0.join("other");
    fs::create_dir(&other).unwrap();
    crate::storage::data_location::RecordsDirectory::fixture(&other);
    let other_store = Store::open(&other).unwrap();
    assert!(runtime.restore(&other_store).is_err());
    assert!(other_store.load().unwrap().credentials.is_none());
    drop(other_store);
    fs::rename(&offline, &records).unwrap();
    let restored = Store::open(&records).unwrap();
    runtime.restore(&restored).unwrap();
    assert_eq!(
        restored
            .load()
            .unwrap()
            .credentials
            .unwrap()
            .token("refresh_token")
            .unwrap(),
        "synthetic-rotated"
    );
}

#[test]
fn concurrent_projects_use_one_renewal_and_transient_failures_back_off() {
    let fixture = Fixture::new();
    let (store, guard) = fixture.records();
    let mut value = credential_value("account-a");
    value["tokens"]["access_token"] = jwt(json!({"exp":1})).into();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(value, true).unwrap());
    store.save(&session).unwrap();
    drop(store);
    let actor = std::sync::Arc::new(tauri::async_runtime::Mutex::new(
        super::account::Runtime::default(),
    ));
    let requests = std::sync::Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let actor = actor.clone();
            let requests = requests.clone();
            let root = fixture.0.join("records");
            let guard = guard.clone();
            std::thread::spawn(move || {
                let mut runtime = actor.blocking_lock();
                let store = Store::open(&root).unwrap();
                let mut session = store.load().unwrap();
                super::account::refresh_with(
                    &mut runtime,
                    &store,
                    &mut session,
                    false,
                    guard,
                    |credentials| {
                        requests.fetch_add(1, Ordering::SeqCst);
                        credentials
                            .renew(json!({"access_token":jwt(json!({"exp":4102444900i64}))}))
                            .map_err(|_| super::http::Failure::SignIn)
                    },
                )
                .unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let store = Store::open(&fixture.0.join("records")).unwrap();
    let mut session = store.load().unwrap();
    let mut runtime = actor.blocking_lock();
    let original = session
        .credentials
        .as_ref()
        .unwrap()
        .token("refresh_token")
        .unwrap()
        .to_owned();
    assert!(super::account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        guard.clone(),
        |_| Err(super::http::Failure::Temporary(
            "Network unavailable.".into()
        ))
    )
    .is_err());
    let mut expired = credential_value("account-a");
    expired["tokens"]["access_token"] = jwt(json!({"exp":1})).into();
    session.credentials = Some(Credentials::parse(expired, true).unwrap());
    assert!(
        super::account::refresh_with(&mut runtime, &store, &mut session, false, guard, |_| {
            panic!("Backoff must prevent another request");
        })
        .is_err()
    );
    assert_eq!(
        session.credentials.unwrap().token("refresh_token").unwrap(),
        original
    );
}

#[test]
fn oauth_failures_are_redacted_and_permanent_failures_require_shared_sign_in() {
    let credentials = Credentials::parse(credential_value("account-a"), true).unwrap();
    for (status, body, permanent) in [
        (401, json!({"error":"synthetic-secret"}), true),
        (
            400,
            json!({"error":{"code":"refresh_token_reused","message":"synthetic-secret"}}),
            true,
        ),
        (200, json!({"access_token":"synthetic-secret"}), true),
        (429, json!({"error":"synthetic-secret"}), false),
        (503, Value::Null, false),
    ] {
        let failure = super::http::classify_refresh(&credentials, status, body)
            .err()
            .unwrap();
        assert_eq!(failure.requires_sign_in(), permanent);
        assert!(!failure.message().contains("synthetic"));
    }
    let fixture = Fixture::new();
    let (store, guard) = fixture.records();
    let mut session = Session::default();
    session.credentials = Some(credentials);
    store.save(&session).unwrap();
    assert!(super::account::refresh_with(
        &mut super::account::Runtime::default(),
        &store,
        &mut session,
        true,
        guard,
        |_| Err(super::http::Failure::SignIn)
    )
    .is_err());
    assert!(store.load().unwrap().reconnect_required);
    assert_eq!(super::account::status(&session).state, "needs-sign-in");
}

#[test]
fn records_inside_projects_or_guest_homes_are_rejected_before_storing_secrets() {
    let fixture = Fixture::new();
    let checked = fixture.project("project");
    let mut registry = crate::projects::Registry::default();
    registry.projects.push(checked.project.clone().into());
    assert!(crate::storage::data_location::require_private_records(
        &checked.project.path.join("records"),
        &registry
    )
    .is_err());
    assert!(crate::storage::data_location::require_private_records(
        &fixture.0.join("records"),
        &registry
    )
    .is_ok());
    let home = super::guest::directory(&checked)
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let store = Store::open(&home).unwrap();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &checked, home.join(".codex")).unwrap();
    assert!(store.save(&session).is_err());
    assert!(!home.join("chatgpt/session.json").exists());
}

#[test]
fn a_successful_staged_login_commits_once_and_updates_existing_guests() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("login");
    let other = fixture.project("other");
    let other_directory = super::guest::directory(&other).unwrap();
    let directory = super::guest::directory(&checked).unwrap();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &other, other_directory.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    fs::write(directory.join("config.toml"), "project-specific").unwrap();
    let pending = staged_login(&fixture, &store, &mut session, &checked);
    super::account::complete_login(
        &store,
        &mut session,
        &checked.project.id,
        &pending.nonce,
        true,
        Some(&checked),
    )
    .unwrap();
    assert!(!pending.directory.exists());
    assert!(session.pending_login.is_none());
    let generation = session.generation;
    for path in [&directory, &other_directory] {
        let auth: Value =
            serde_json::from_slice(&fs::read(path.join("auth.json")).unwrap()).unwrap();
        assert_eq!(auth["tokens"]["account_id"], "account-b");
        assert_eq!(auth["tokens"]["refresh_token"], "");
    }
    assert_eq!(
        fs::read_to_string(directory.join("config.toml")).unwrap(),
        "project-specific"
    );
    super::account::complete_login(
        &store,
        &mut session,
        &checked.project.id,
        &pending.nonce,
        false,
        None,
    )
    .unwrap();
    assert_eq!(session.generation, generation);
    assert_eq!(session.credentials.unwrap().account().unwrap(), "account-b");
}

#[test]
fn different_users_in_the_same_workspace_are_distinct_adoption_choices() {
    let mut first = credential_value("workspace");
    let mut second = first.clone();
    first["tokens"]["id_token"] = jwt(
        json!({"sub":"user-a","https://api.openai.com/auth":{"chatgpt_account_id":"workspace"}}),
    )
    .into();
    second["tokens"]["id_token"] = jwt(
        json!({"sub":"user-b","https://api.openai.com/auth":{"chatgpt_account_id":"workspace"}}),
    )
    .into();
    let first = Credentials::parse(first, true).unwrap();
    let second = Credentials::parse(second, true).unwrap();
    assert_ne!(
        first.selection_key().unwrap(),
        second.selection_key().unwrap()
    );
    assert!(first.renew(json!({"id_token":second.token("id_token").unwrap(),"access_token":first.token("access_token").unwrap()})).is_err());
}

#[test]
fn custom_profiles_and_replaced_staging_directories_are_preserved() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("login");
    let mut session = Session::default();
    let pending = staged_login(&fixture, &store, &mut session, &checked);
    let saved = fixture.0.join("saved-staging");
    fs::rename(&pending.directory, &saved).unwrap();
    let foreign = fixture.0.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("sentinel"), "preserve").unwrap();
    symlink(&foreign, &pending.directory).unwrap();
    assert!(super::account::complete_login(
        &store,
        &mut session,
        &checked.project.id,
        &pending.nonce,
        false,
        None
    )
    .is_err());
    assert_eq!(
        fs::read_to_string(foreign.join("sentinel")).unwrap(),
        "preserve"
    );
    assert!(store.load().unwrap().pending_login.unwrap().cancelled);
    fs::write(
        &checked.brig.executable,
        "#!/bin/sh\nprintf '%s' '{\"env\":[{\"name\":\"CODEX_HOME\",\"value\":\"/custom\"}]}'\n",
    )
    .unwrap();
    assert!(super::guest::directory(&checked)
        .unwrap_err()
        .contains("custom credential location"));
}

#[test]
fn a_custom_named_standard_codex_profile_receives_the_shared_access_copy() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let mut checked = fixture.project("custom-name");
    checked.project.profile = "team-codex".into();
    fs::write(&checked.brig.executable, r#"#!/bin/sh
if [ "$1" = agent ]; then
    printf '%s' '{"name":"team-codex","kind":"agent","binary":"codex","env":[],"volumes":[],"files":[]}'
    exit 0
fi
printf '%s' '{"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[]}}'
"#).unwrap();
    checked
        .brig
        .validate_agent_profile("team-codex", crate::agents::AgentKind::Codex)
        .unwrap();
    let directory = super::guest::directory(&checked).unwrap();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &checked, directory.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    assert!(super::guest::connected(&session, &checked, &directory).unwrap());
    let access: Value =
        serde_json::from_slice(&fs::read(directory.join("auth.json")).unwrap()).unwrap();
    assert_eq!(access["tokens"]["refresh_token"], "");
}

#[test]
fn custom_codex_credential_locations_still_fail_without_changing_existing_records() {
    let fixture = Fixture::new();
    let checked = fixture.project("custom-location");
    let directory = super::guest::directory(&checked).unwrap();
    fs::write(directory.join("auth.json"), "synthetic existing sign-in").unwrap();
    fs::write(directory.join("history.jsonl"), "existing history").unwrap();
    for profile in [
        json!({"env":[{"name":"CODEX_HOME","value":"/custom"}]}),
        json!({"files":[{"path":".codex/auth.json","ref":"custom"}]}),
        json!({"volumes":[{"kind":"tmpfs","path":".codex"}]}),
    ] {
        fs::write(
            &checked.brig.executable,
            format!("#!/bin/sh\nprintf '%s' '{}'\n", profile),
        )
        .unwrap();
        assert!(super::guest::directory(&checked)
            .unwrap_err()
            .contains("custom credential location"));
        assert_eq!(
            fs::read_to_string(directory.join("auth.json")).unwrap(),
            "synthetic existing sign-in"
        );
        assert_eq!(
            fs::read_to_string(directory.join("history.jsonl")).unwrap(),
            "existing history"
        );
    }
}

#[test]
fn project_migration_keeps_legacy_account_and_pending_login_bound_only_to_codex() {
    let fixture = Fixture::new();
    let (store, records) = fixture.records();
    let checked = fixture.project("migrate-project");
    let directory = super::guest::directory(&checked).unwrap();
    let mut session = Session::default();
    session.generation = 7;
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &checked, directory.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    let pending = staged_login(&fixture, &store, &mut session, &checked);
    let account_file = fixture.0.join("records/chatgpt/session.json");
    let before = fs::read(&account_file).unwrap();
    let mut registry = crate::projects::Registry::default();
    registry.projects.push(checked.project.clone().into());
    let project_file = fixture.0.join("records/projects.json");
    crate::storage::write(
        &project_file,
        &crate::projects::migration_tests::legacy_registry(&registry),
    )
    .unwrap();
    crate::projects::migrate(&project_file, &records).unwrap();
    let mut migrated = crate::projects::Registry::load(&project_file).unwrap();
    let codex = migrated.project(&checked.project.id).unwrap();
    let environment = migrated
        .ensure_environment(
            &codex.id,
            crate::agents::AgentKind::ClaudeCode,
            &crate::desktop::settings::Settings::default(),
        )
        .unwrap();
    let claude = migrated
        .environment(&codex.id, &environment.environment_id)
        .unwrap();
    crate::storage::write(&project_file, &migrated).unwrap();
    assert_eq!(fs::read(&account_file).unwrap(), before);
    let mut restored = store.load().unwrap();
    assert_eq!(restored.generation, 7);
    assert_eq!(
        restored.pending_login.as_ref().unwrap().nonce,
        pending.nonce
    );
    assert_eq!(restored.bindings[0].environment_id(), codex.environment_id);
    assert!(restored.bindings[0].belongs_to(&codex));
    assert!(!restored.bindings[0].belongs_to(&claude));
    assert_eq!(
        restored.pending_login.as_ref().unwrap().environment_id(),
        codex.environment_id
    );
    assert_eq!(
        super::account::status(&restored).pending_environment_id,
        Some(codex.environment_id)
    );
    let mut selected = checked;
    selected.project = claude;
    assert!(super::guest::directory(&selected)
        .unwrap_err()
        .contains("only for Codex"));
    assert!(super::guest::bind(&mut restored, &selected, directory).is_err());
    assert_eq!(fs::read(&account_file).unwrap(), before);
}

#[test]
fn an_existing_codex_process_blocks_credential_adoption_without_changing_files() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let mut checked = fixture.project("busy");
    let directory = super::guest::directory(&checked).unwrap();
    let original = serde_json::to_vec(&credential_value("account-a")).unwrap();
    fs::write(directory.join("auth.json"), &original).unwrap();
    let inventory = fixture.0.join("inventory.json");
    let info = fixture.0.join("info.json");
    fs::write(&inventory,serde_json::to_vec(&json!({"apiVersion":"brig.sh/v1alpha1","kind":"SandboxList","data":{"sandboxes":[{"ref":checked.project.reference(),"state":"running"}]}})).unwrap()).unwrap();
    fs::write(&info,serde_json::to_vec(&json!({"apiVersion":"brig.sh/v1alpha1","kind":"Info","data":{"profile":checked.project.profile,"session":checked.project.label,"project":{"host":checked.project.path},"workspace":checked.plan["data"]["home"]["host"]}})).unwrap()).unwrap();
    checked.brig.environment.insert(
        "FIXTURE_INVENTORY".into(),
        inventory.to_string_lossy().into_owned(),
    );
    checked
        .brig
        .environment
        .insert("FIXTURE_INFO".into(), info.to_string_lossy().into_owned());
    fs::write(&checked.brig.executable,"#!/bin/sh\nif [ \"$1\" = sh ]; then printf busy; exit 0; fi\nif [ \"$2\" = ls ]; then cat \"$FIXTURE_INVENTORY\"; exit 0; fi\ncat \"$FIXTURE_INFO\"\n").unwrap();
    let mut session = Session::default();
    assert!(super::account::adopt(
        &store,
        &mut session,
        &checked,
        Credentials::parse(credential_value("account-a"), true).unwrap()
    )
    .unwrap_err()
    .contains("Codex process"));
    assert!(store.load().unwrap().credentials.is_none());
    assert_eq!(fs::read(directory.join("auth.json")).unwrap(), original);
}

#[test]
fn guest_projection_excludes_unknown_private_fields() {
    let mut value = credential_value("account-a");
    value["private_future_field"] = "synthetic-secret".into();
    value["tokens"]["private_future_token"] = "synthetic-secret".into();
    let credentials = Credentials::parse(value, true).unwrap();
    let guest = credentials.guest();
    assert!(guest.get("private_future_field").is_none());
    assert!(guest["tokens"].get("private_future_token").is_none());
}

#[test]
fn interrupted_refresh_requests_are_not_retried_after_credentials_were_sent() {
    for (code, uploaded, permanent) in [
        (6, 0, false),
        (7, 0, false),
        (28, 0, false),
        (28, 180, true),
        (63, 180, true),
    ] {
        let output = crate::platform::command::CommandOutput {
            stdout: format!("\n000\n{uploaded}"),
            stderr: "synthetic-private-error".into(),
            exit_code: Some(code),
            cancelled: false,
            timed_out: false,
            output_exceeded: false,
        };
        let failure = super::http::response(output).err().unwrap();
        assert_eq!(failure.requires_sign_in(), permanent);
        assert!(!failure.message().contains("synthetic"));
    }
    let output = crate::platform::command::CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        cancelled: true,
        timed_out: false,
        output_exceeded: false,
    };
    assert!(super::http::response(output)
        .err()
        .unwrap()
        .requires_sign_in());
}

#[test]
fn removing_one_managed_sign_in_preserves_the_shared_account_and_other_projects() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let first = fixture.project("first");
    let second = fixture.project("second");
    let first_directory = super::guest::directory(&first).unwrap();
    let second_directory = super::guest::directory(&second).unwrap();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &first, first_directory.clone()).unwrap();
    super::guest::bind(&mut session, &second, second_directory.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    fs::write(first_directory.join("history.jsonl"), "preserve").unwrap();
    let other = fs::read(second_directory.join("auth.json")).unwrap();
    // Codex can rewrite metadata after the original copy was applied.
    let mut changed: Value =
        serde_json::from_slice(&fs::read(first_directory.join("auth.json")).unwrap()).unwrap();
    changed["last_refresh"] = "2026-11-13T00:00:00Z".into();
    fs::write(
        first_directory.join("auth.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(super::guest::detach(&store, &mut session, &first.project.id, true).unwrap());
    assert!(!first_directory.join("auth.json").exists());
    assert_eq!(
        fs::read_to_string(first_directory.join("history.jsonl")).unwrap(),
        "preserve"
    );
    assert_eq!(fs::read(second_directory.join("auth.json")).unwrap(), other);
    assert_eq!(
        store
            .load()
            .unwrap()
            .credentials
            .unwrap()
            .account()
            .unwrap(),
        "account-a"
    );
    super::guest::detach(&store, &mut session, &second.project.id, false).unwrap();
    assert!(second_directory.join("auth.json").exists());
    assert!(session.bindings.is_empty());
}

#[test]
fn a_verified_guest_directory_handle_cannot_be_redirected_during_read_write_or_cleanup() {
    let fixture = Fixture::new();
    let checked = fixture.project("project");
    let path = super::guest::directory(&checked).unwrap();
    let directory =
        super::directory::Directory::open(&path, Some(super::guest::identity(&path).unwrap()))
            .unwrap();
    let original = path.with_file_name("original-codex");
    fs::rename(&path, &original).unwrap();
    let foreign = fixture.0.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("auth.json"), "preserve").unwrap();
    symlink(&foreign, &path).unwrap();
    directory.write(b"synthetic-guest-auth").unwrap();
    assert_eq!(directory.read().unwrap().unwrap(), b"synthetic-guest-auth");
    directory.remove().unwrap();
    assert!(!original.join("auth.json").exists());
    assert_eq!(
        fs::read_to_string(foreign.join("auth.json")).unwrap(),
        "preserve"
    );
    assert!(super::directory::Directory::open(&path, None).is_err());
}

#[test]
fn shared_sign_out_removes_managed_metadata_rewrites_and_preserves_foreign_credentials() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("project");
    let path = super::guest::directory(&checked).unwrap();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    super::guest::bind(&mut session, &checked, path.clone()).unwrap();
    store.save(&session).unwrap();
    super::guest::publish(&store, &mut session).unwrap();
    let mut guest = session.credentials.as_ref().unwrap().guest();
    guest["last_refresh"] = "2026-11-13T00:00:00Z".into();
    fs::write(path.join("auth.json"), serde_json::to_vec(&guest).unwrap()).unwrap();
    session.credentials = None;
    session.disconnected = true;
    super::guest::cleanup(&mut session).unwrap();
    assert!(!path.join("auth.json").exists());
}

#[test]
fn uncommitted_login_files_require_a_confirmed_process_result_and_ignore_late_callbacks() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let checked = fixture.project("login");
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    let pending = staged_login(&fixture, &store, &mut session, &checked);
    let mut runtime = super::account::Runtime::default();
    assert!(
        !runtime.recovery_success(&pending),
        "A file alone does not prove a completed login after a crash"
    );
    runtime.attach_login_process(pending.nonce.clone(), 42).unwrap();
    runtime.record_login_exit(&pending.nonce, true);
    assert!(runtime.recovery_success(&pending));
    runtime.record_login_exit(&"c".repeat(32), false);
    assert!(runtime.recovery_success(&pending));
    runtime.record_login_exit(&pending.nonce, false);
    assert!(!runtime.recovery_success(&pending));
    super::account::complete_login(
        &store,
        &mut session,
        &checked.project.id,
        &pending.nonce,
        runtime.recovery_success(&pending),
        None,
    )
    .unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .credentials
            .unwrap()
            .account()
            .unwrap(),
        "account-a"
    );
    assert!(!pending.directory.exists());
}

#[test]
fn an_uncertain_renewal_keeps_its_no_retry_state_when_the_records_drive_disappears() {
    let fixture = Fixture::new();
    let (store, guard) = fixture.records();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    store.save(&session).unwrap();
    let records = fixture.0.join("records");
    let offline = fixture.0.join("offline");
    let mut runtime = super::account::Runtime::default();
    assert!(super::account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        guard.clone(),
        |_| {
            fs::rename(&records, &offline).unwrap();
            Err(super::http::Failure::Uncertain)
        }
    )
    .is_err());
    drop(store);
    fs::rename(&offline, &records).unwrap();
    let restored = Store::open(&records).unwrap();
    runtime.restore(&restored).unwrap();
    let mut recovered = restored.load().unwrap();
    assert!(recovered.reconnect_required);
    assert!(super::account::refresh_with(
        &mut runtime,
        &restored,
        &mut recovered,
        true,
        guard,
        |_| panic!("Do not reuse a refresh token after an uncertain result")
    )
    .is_err());
}

#[test]
fn chatgpt_idle_checks_reject_claude_before_running_any_guest_command() {
    let fixture = Fixture::new();
    let mut checked = fixture.project("claude-idle");
    checked.project.agent_kind = crate::agents::AgentKind::ClaudeCode;
    let marker = fixture.0.join("unexpected-guest-command");
    fs::write(
        &checked.brig.executable,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    assert!(super::guest::require_idle(&checked, false)
        .unwrap_err()
        .contains("Codex"));
    assert!(!marker.exists());
}

#[test]
fn a_pending_renewal_rebinds_only_to_the_verified_matching_records_dataset() {
    let fixture = Fixture::new();
    let (store, guard) = fixture.records();
    let mut session = Session::default();
    session.credentials = Some(Credentials::parse(credential_value("account-a"), true).unwrap());
    store.save(&session).unwrap();
    let mut runtime = super::account::Runtime::default();
    let records = fixture.0.join("records");
    let located = fixture.0.join("located");
    let result = super::account::refresh_with(
        &mut runtime,
        &store,
        &mut session,
        true,
        guard.clone(),
        |credentials| {
            fs::rename(&records, &located).unwrap();
            credentials.renew(json!({"access_token":jwt(json!({"exp":4102444900i64})),"refresh_token":"synthetic-reconnected-renewal"})).map_err(|_| super::http::Failure::SignIn)
        },
    );
    assert!(result.is_err());
    assert!(!records.exists());
    drop(store);
    let foreign = fixture.0.join("foreign");
    fs::create_dir(&foreign).unwrap();
    let foreign_guard =
        crate::storage::data_location::RecordsDirectory::fixture_with_id(&foreign, &"b".repeat(32));
    assert!(runtime.reconnect_records(&foreign_guard).is_err());
    assert!(!foreign.join("chatgpt/session.json").exists());
    let located_guard =
        crate::storage::data_location::RecordsDirectory::fixture_existing(&located, guard.id());
    let nonce = "c".repeat(32);
    runtime.attach_login_process(nonce.clone(), 1).unwrap();
    runtime.record_login_exit(&nonce, true);
    let pending = super::store::PendingLogin {
        project_id: "d".repeat(32),
        nonce: nonce.clone(),
        directory: fixture.0.join("guest/.tugboat-chatgpt-login").join(&nonce),
        identity: (1, 2),
        imported: false,
        cancelled: false,
    };
    assert!(runtime.recovery_success(&pending));
    runtime.reconnect_records(&located_guard).unwrap();
    assert!(runtime.recovery_success(&pending));
    let restored = Store::open(&located).unwrap();
    runtime.restore(&restored).unwrap();
    let saved = restored.load().unwrap();
    assert!(
        saved.credentials.unwrap().token("refresh_token").unwrap()
            == "synthetic-reconnected-renewal"
    );
    assert!(!records.exists());
}


#[test]
fn recheck_disconnected_record_cannot_require_renewal_without_credentials() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let session = Session { reconnect_required:true, ..Default::default() };
    assert!(store.save(&session).is_err());
}

#[test]
fn recheck_an_imported_login_requires_committed_credentials() {
    let fixture = Fixture::new();
    let (store, _) = fixture.records();
    let nonce = "a".repeat(32);
    let pending = super::store::PendingLogin { project_id:"b".repeat(32),
        directory:fixture.0.join("guest-home/.tugboat-chatgpt-login").join(&nonce), nonce,
        identity:(1,2), imported:true, cancelled:false };
    let session = Session { pending_login:Some(pending), ..Default::default() };
    assert!(store.save(&session).is_err());
}

#[test]
fn recheck_renewal_can_commit_before_requesting_replacement_credentials() {
    let fixture = Fixture::new();
    let (store, records) = fixture.records();
    let mut session = Session::default();
    let credentials = super::credentials::Credentials::parse(credential_value("test"), true).unwrap();
    session.connect(credentials.clone()).unwrap();
    session.generation = u64::MAX;
    store.save(&session).unwrap();
    let mut requested = false;
    let result = super::account::refresh_with(&mut super::account::Runtime::default(), &store, &mut session, true,
        records, |_| { requested = true; Ok(credentials) });
    assert!(result.is_err());
    assert!(!requested);
}

#[test]
fn recheck_failed_guest_cleanup_preserves_retry_receipts_in_memory() {
    let fixture = Fixture::new();
    let mut session = Session::default();
    for index in 1..=2 {
        let dir = fixture.0.join(index.to_string());
        fs::create_dir(&dir).unwrap();
        session.bindings.push(Binding { project_id:format!("{index:032x}"),
            identity:super::guest::identity(&dir).unwrap(), directory:dir.clone(),
            auth_hash:None,access_hash:None,pending:false });
        if index == 1 { fs::create_dir(dir.join("auth.json")).unwrap(); }
    }
    assert!(super::guest::cleanup(&mut session).is_err());
    assert_eq!(session.bindings.len(), 2);
}
