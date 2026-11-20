use super::*;
use std::cell::Cell;

#[test]
fn a_storage_failure_blocks_installation_and_can_retry() {
    let result = finish_update(
        || Err("Could not close usage storage. Fixture database is locked.".into()),
        || panic!("install before storage closed"),
        |_: ()| panic!("relaunch before installation"),
        |_, _| panic!("recovery before installation"),
    );
    assert!(
        matches!(result, Err(UpdateFailure::Shutdown(message)) if message.contains("Fixture database is locked"))
    );

    let step = Cell::new(0);
    let result = finish_update(
        || {
            assert_eq!(step.replace(1), 0);
            Ok(())
        },
        || {
            assert_eq!(step.replace(2), 1);
            Ok(())
        },
        |()| {
            assert_eq!(step.replace(3), 2);
            Ok(())
        },
        |_, _| panic!("recovery after success"),
    );
    assert!(result.is_ok());
    assert_eq!(step.get(), 3);
}

#[test]
fn a_failed_install_reports_the_previous_bundle_and_skips_relaunch() {
    let result = finish_update(
        || Ok(()),
        || {
            Err(InstallFailure {
                message: "Fixture replacement failed.".into(),
                preserved_bundle: None,
            })
        },
        |()| panic!("relaunch after failed install"),
        |message, backup| {
            assert!(backup.is_none());
            UpdateFailure::Recovery {
                message,
                installed_version: Some("0.5.0".into()),
                can_reopen: true,
            }
        },
    );
    assert!(
        matches!(result, Err(UpdateFailure::Recovery { message, installed_version, can_reopen: true })
        if message.contains("Fixture replacement failed") && installed_version.as_deref() == Some("0.5.0"))
    );
}

#[test]
fn a_failed_restore_supplies_the_preserved_app_for_recovery() {
    let preserved = PathBuf::from("/private/tmp/tauri-backup/current_app");
    let result = finish_update(
        || Ok(()),
        || {
            Err(InstallFailure {
                message: "Fixture restore failed.".into(),
                preserved_bundle: Some(preserved.clone()),
            })
        },
        |()| panic!("relaunch after failed install"),
        |message, backup| {
            assert_eq!(backup, Some(preserved.clone()));
            UpdateFailure::Recovery {
                message,
                installed_version: None,
                can_reopen: true,
            }
        },
    );
    assert!(
        matches!(result, Err(UpdateFailure::Recovery { message, installed_version: None, can_reopen: true })
        if message.contains("Fixture restore failed"))
    );
}

#[test]
fn a_failed_relaunch_reports_the_newly_installed_bundle() {
    let result = finish_update(
        || Ok(()),
        || Ok(()),
        |()| Err("Fixture app could not open.".into()),
        |message, backup| {
            assert!(backup.is_none());
            UpdateFailure::Recovery {
                message,
                installed_version: Some("0.5.1".into()),
                can_reopen: true,
            }
        },
    );
    assert!(
        matches!(result, Err(UpdateFailure::Recovery { message, installed_version, can_reopen: true })
        if message.contains("Fixture app could not open") && installed_version.as_deref() == Some("0.5.1"))
    );
}
