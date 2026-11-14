use super::*;

const MISSING: &str = "00000000-0000-4000-8000-000000000021";
const UNCHANGED: &str = "00000000-0000-4000-8000-000000000022";

fn add_native(watch: &Watch, source: &Path, id: &str) -> PathBuf {
    let path = watch
        .home
        .join(".codex/sessions")
        .join(format!("rollout-{id}.jsonl"));
    fs::write(&path, fs::read_to_string(source).unwrap().replace(CHAT, id)).unwrap();
    path
}

fn append_occurrence(source: &Path, number: u64) {
    let mut file = OpenOptions::new().append(true).open(source).unwrap();
    let tokens = |factor: u64| {
        json!({"input_tokens":100*factor,"cached_input_tokens":0,
        "cache_write_input_tokens":0,"output_tokens":10*factor,"total_tokens":110*factor})
    };
    writeln!(file,"{}",json!({"type":"token_usage_record","timestamp":format!("2026-11-13T00:00:{number:02}Z"),
        "payload":{"thread_id":CHAT,"turn_id":"aaaaaaaa-0000-4000-8000-000000000001",
        "root_turn_id":"aaaaaaaa-0000-4000-8000-000000000001","response_id":format!("append-regression-{number}"),
        "usage":tokens(1),"thread_token_usage":tokens(number)}})).unwrap();
}

#[test]
fn appending_one_record_preserves_unrelated_facts_and_atomically_saves_source_loss() {
    let (_fixture, sql, native, original, baseline) = generated();
    let original_bytes = fs::read(&original).unwrap();
    let missing_native = add_native(&sql, &native, MISSING);
    add_native(&sql, &native, UNCHANGED);
    assert_eq!(synchronize(&sql).unwrap().0.usage.requests, 3);
    change_state(&sql, MISSING, SessionState::Archived, |_| false).unwrap();
    let store = sql.store.as_ref().unwrap();
    let project = "22222222222222222222222222222222";
    let other = crate::usage_store::Environment::new(
        project.into(),
        projects::EnvironmentId::for_agent(project, AgentKind::Codex),
        AgentKind::Codex,
    )
    .unwrap();
    store
        .with(|store| store.save_environment(&other, &baseline))
        .unwrap();
    let before = sql.load().unwrap();
    let foreign_before = store.with(|store| store.load_environment(&other)).unwrap();

    append_occurrence(&native, 2);
    crate::usage_store::record_reads::begin();
    let first = synchronize(&sql).unwrap();
    let reads = crate::usage_store::record_reads::finish();
    assert_eq!(
        reads.len(),
        2,
        "hydrate and one transaction-local prior read"
    );
    assert_eq!(
        reads[0], reads[1],
        "append must load only the changed record"
    );
    store
        .with(|store| {
            let identity: (String, String) = store
                .connection
                .query_row(
                    "SELECT e.project_id,g.native_session_id FROM session_records r JOIN session_groups g ON g.session_key=r.session_key JOIN usage_environments e ON e.environment_key=g.environment_key WHERE r.record_id=?1",
                    [reads[0]],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::usage_store::error)?;
            assert_eq!(identity, (PROJECT.into(), CHAT.into()));
            Ok(())
        })
        .unwrap();
    assert!(first.1);
    let first_saved = sql.load().unwrap();
    assert_eq!(
        usage::summarize(&first_saved.sessions[CHAT].turns).requests,
        2
    );
    for id in [MISSING, UNCHANGED] {
        assert_eq!(
            value(&first_saved.sessions[id]),
            value(&before.sessions[id])
        );
    }
    assert_eq!(
        value(&store.with(|store| store.load_environment(&other)).unwrap()),
        value(&foreign_before)
    );

    append_occurrence(&native, 3);
    fs::remove_file(&missing_native).unwrap();
    store.with(|store| store.connection.execute_batch(
        "CREATE TEMP TRIGGER abort_append_probe BEFORE INSERT ON source_cursors BEGIN SELECT RAISE(ABORT,'generated append cursor failure'); END;"
    ).map_err(crate::usage_store::error)).unwrap();
    assert!(synchronize(&sql).is_err());
    assert_eq!(value(&sql.load().unwrap()), value(&first_saved));
    store
        .with(|store| {
            store
                .connection
                .execute_batch("DROP TRIGGER abort_append_probe")
                .map_err(crate::usage_store::error)
        })
        .unwrap();

    let retried = synchronize(&sql).unwrap();
    assert!(retried.1);
    let saved = sql.load().unwrap();
    assert_eq!(usage::summarize(&saved.sessions[CHAT].turns).requests, 3);
    assert!(!saved.sessions[MISSING].available);
    assert_eq!(saved.sessions[MISSING].state, SessionState::Archived);
    assert_eq!(
        value(&saved.sessions[MISSING].turns),
        value(&before.sessions[MISSING].turns)
    );
    assert_eq!(
        value(&saved.sessions[UNCHANGED]),
        value(&before.sessions[UNCHANGED])
    );
    assert_eq!(
        value(&store.with(|store| store.load_environment(&other)).unwrap()),
        value(&foreign_before)
    );
    assert!(resume_dashboard(&sql, MISSING).is_err());
    assert!(!synchronize(&sql).unwrap().1);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    emit(
        "append-header-rollback",
        &json!({"afterFirstAppend":first_saved,"afterRetry":saved,"firstAppendRawRecordReads":reads,
        "foreignProject":foreign_before,"cursorFailureRolledBackRequestAndAvailability":true,
        "missingSourceResumeRejected":true,"quietReplayUnchanged":true,
        "originalSha256":crate::hash::hex(Sha256::digest(original_bytes))}),
    );
    store.close().unwrap();
}
