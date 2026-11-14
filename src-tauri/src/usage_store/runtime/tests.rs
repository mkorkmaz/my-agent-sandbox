use super::*;
use crate::storage::data_location::RecordsDirectory;
use std::fs;
use std::time::Duration;

#[test]
fn pending_readers_receive_the_same_actionable_startup_failure() {
    let runtime = Runtime::default();
    let waiting = runtime.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || sender.send(waiting.wait().err()).unwrap());
    assert!(receiver.try_recv().is_err());
    runtime.complete(Err("synthetic import parity failure".into()));
    let message = receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(message.contains("synthetic import parity failure"));
    assert_eq!(runtime.wait().err(), Some(message.clone()));
    assert_eq!(runtime.failure(), Some(message));
    worker.join().unwrap();
    runtime.close().unwrap();
}

#[test]
fn closing_the_runtime_invalidates_all_cloned_watch_handles() {
    let root = std::env::temp_dir().join(format!(
        "tugboat-sqlite-runtime-close-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let records = RecordsDirectory::fixture(&root);
    let store = Store::create_staging(&records, &records.path().join("usage.sqlite3")).unwrap();
    records.activate_sqlite().unwrap();
    let shared = SharedStore::new(store);
    let watch = shared.clone();
    let runtime = Runtime::default();
    runtime.complete(Ok(shared));
    runtime
        .wait()
        .unwrap()
        .with(|store| store.integrity_check())
        .unwrap();
    runtime.close().unwrap();
    assert!(watch
        .with(|store| store.integrity_check())
        .unwrap_err()
        .contains("closed"));
    assert!(runtime.wait().err().unwrap().contains("closed"));
    runtime.close().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_missing_records_folder_fails_close_without_recreating_files() {
    let root = std::env::temp_dir().join(format!(
        "tugboat-sqlite-runtime-detach-{}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("records")).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let records = RecordsDirectory::fixture(&root.join("records"));
    let store = Store::create_staging(&records, &records.path().join("usage.sqlite3")).unwrap();
    records.activate_sqlite().unwrap();
    let shared = SharedStore::new(store);
    let watch = shared.clone();
    let runtime = Runtime::default();
    runtime.complete(Ok(shared));
    fs::rename(root.join("records"), root.join("detached")).unwrap();
    assert!(runtime.close().is_err());
    assert!(!root.join("records").exists());
    assert!(root.join("detached/usage.sqlite3").is_file());
    assert!(watch.with(|store| store.integrity_check()).is_err());
    assert!(runtime.failure().unwrap().contains("retry closing"));
    assert!(runtime.close().is_err());
    assert!(!root.join("records").exists());
    fs::rename(root.join("detached"), root.join("records")).unwrap();
    let project = "1".repeat(32);
    let environment = crate::usage_store::Environment::new(
        project.clone(),
        crate::projects::EnvironmentId::for_agent(&project, crate::agents::AgentKind::Codex),
        crate::agents::AgentKind::Codex,
    )
    .unwrap();
    let ledger = crate::conversations::Ledger {
        warnings: vec!["fixture final flush after reconnection".into()],
        ..Default::default()
    };
    runtime
        .wait()
        .unwrap()
        .with(|store| store.save_environment(&environment, &ledger).map(|_| ()))
        .unwrap();
    runtime.close().unwrap();
    assert!(runtime.failure().is_none());
    assert!(watch.with(|store| store.integrity_check()).is_err());
    let reopened = Store::open(&records).unwrap();
    assert_eq!(
        reopened.load_environment(&environment).unwrap().warnings,
        ledger.warnings
    );
    reopened.close().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_real_sqlite_close_error_retains_the_connection_and_can_be_retried() {
    let root = std::env::temp_dir().join(format!(
        "tugboat-sqlite-runtime-busy-close-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let records = RecordsDirectory::fixture(&root);
    let store = Store::create_staging(&records, &records.path().join("usage.sqlite3")).unwrap();
    records.activate_sqlite().unwrap();
    let shared = SharedStore::new(store);
    let watch = shared.clone();
    let runtime = Runtime::default();
    runtime.complete(Ok(shared));
    let statement = watch
        .with(|store| {
            let query =
                std::ffi::CString::new("SELECT records_directory_id FROM store_identity").unwrap();
            let mut statement = std::ptr::null_mut();
            let result = unsafe {
                rusqlite::ffi::sqlite3_prepare_v2(
                    store.connection.handle(),
                    query.as_ptr(),
                    -1,
                    &mut statement,
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(result, rusqlite::ffi::SQLITE_OK);
            Ok(statement)
        })
        .unwrap();
    assert!(runtime.close().is_err());
    assert!(runtime.failure().is_some());
    assert!(runtime.close().is_err());
    runtime
        .wait()
        .unwrap()
        .with(|store| store.integrity_check())
        .unwrap();
    let result = unsafe { rusqlite::ffi::sqlite3_finalize(statement) };
    assert_eq!(result, rusqlite::ffi::SQLITE_OK);
    runtime.close().unwrap();
    assert!(runtime.failure().is_none());
    assert!(watch.with(|store| store.integrity_check()).is_err());
    fs::remove_dir_all(root).unwrap();
}
