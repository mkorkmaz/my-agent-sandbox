use super::*;

#[test]
fn a_temporary_name_collision_preserves_the_existing_file() {
    let root =
        std::env::temp_dir().join(format!("tugboat-storage-collision-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("record.json");
    let temporary = root.join("foreign.tmp");
    fs::write(&temporary, b"foreign temporary").unwrap();
    assert!(write_staged_bytes(&file, b"new record", temporary.clone()).is_err());
    assert_eq!(fs::read(&temporary).unwrap(), b"foreign temporary");
    assert!(!file.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn publication_and_cleanup_preserve_a_replaced_temporary_path() {
    let root =
        std::env::temp_dir().join(format!("tugboat-storage-replaced-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let temporary = root.join("temporary");
    let mut staged = TemporaryRecord::new(temporary.clone());
    let mut output = staged.create().unwrap();
    output.write_all(b"owned").unwrap();
    assert!(staged.replace(&root.join("record")).is_err());
    staged.sync_file(&output).unwrap();
    fs::remove_file(&temporary).unwrap();
    fs::write(&temporary, b"replacement").unwrap();
    assert!(staged.replace(&root.join("record")).is_err());
    drop(staged);
    assert_eq!(fs::read(&temporary).unwrap(), b"replacement");
    fs::remove_dir_all(root).unwrap();
}


#[test]
fn recheck_staging_sync_requires_the_file_being_published() {
    let root = std::env::temp_dir().join(format!("tugboat-sync-review-{}",std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut record = TemporaryRecord::new(root.join("first"));
    let own = record.create().unwrap();
    let other = File::create(root.join("second")).unwrap();
    let accepted = record.sync_file(&other).is_ok();
    assert!(record.sync_file(&own).is_ok());
    drop(record);
    fs::remove_dir_all(root).unwrap();
    assert!(!accepted);
}
