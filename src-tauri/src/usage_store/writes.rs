use super::{codecs, error, Environment, Store, CLAUDE_PRICING_VERSION};
use crate::{
    agents::AgentKind,
    conversations::{Ledger, SessionRecord, SessionState},
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;

impl Store {
    /// Run only before live Codex synchronization. Offline reads retain saved prices.
    pub(crate) fn prepare_codex(&mut self, environment: &Environment) -> Result<bool, String> {
        self.require_active()?;
        if environment.agent_kind != AgentKind::Codex {
            return Ok(false);
        }
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok(false);
        };
        if self.codex_prepared(key) {
            return Ok(false);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let mut statement=transaction.prepare("SELECT r.record_id,g.native_session_id FROM session_groups g JOIN session_records r ON r.session_key=g.session_key WHERE g.environment_key=?1 ORDER BY r.record_id").map_err(error)?;
        let records = statement
            .query_map([key], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(error)?;
        let mut changed = false;
        for record in records {
            let (record_id, id) = record.map_err(error)?;
            let mut record = codecs::read_record(&transaction, record_id)?;
            let mut corrected = false;
            for request in record
                .turns
                .values_mut()
                .flat_map(|turn| &mut turn.requests)
            {
                corrected |= crate::usage::correct_provider_price(request);
            }
            if corrected {
                save_record(&transaction, record_id, &id, &record)?;
                changed = true;
            }
        }
        drop(statement);
        transaction.commit().map_err(error)?;
        self.mark_codex_prepared(key);
        Ok(changed)
    }

    pub(crate) fn save_environment(
        &mut self,
        environment: &Environment,
        ledger: &Ledger,
    ) -> Result<bool, String> {
        self.save_records(environment, ledger)
    }

    /// The caller supplies all slots for each included native session ID.
    /// IDs absent from this bounded ledger retain their facts and selection.
    pub(crate) fn save_records(
        &mut self,
        environment: &Environment,
        ledger: &Ledger,
    ) -> Result<bool, String> {
        let loaded_ids = ledger
            .sessions
            .keys()
            .chain(ledger.variants.keys())
            .cloned()
            .collect();
        self.save_partial_environment(environment, ledger, &loaded_ids)
    }

    pub(crate) fn save_partial_environment(
        &mut self,
        environment: &Environment,
        ledger: &Ledger,
        loaded_ids: &BTreeSet<String>,
    ) -> Result<bool, String> {
        #[cfg(test)]
        let _benchmark_detail =
            crate::conversations::sqlite_benchmark::Detail::new("save_partial_environment");
        self.require_active()?;
        let changed_before = self.connection.total_changes();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let version = if environment.agent_kind == AgentKind::ClaudeCode {
            CLAUDE_PRICING_VERSION
        } else {
            0
        };
        let key = codecs::ensure_environment(&transaction, environment, version, &ledger.warnings)?;
        let warnings = codecs::json(&ledger.warnings)?;
        transaction.execute("UPDATE usage_environments SET warnings_json=?2 WHERE environment_key=?1 AND warnings_json IS NOT ?2",params![key,warnings]).map_err(error)?;
        let ids: BTreeSet<_> = ledger
            .sessions
            .keys()
            .chain(ledger.variants.keys())
            .collect();
        for id in ids {
            if loaded_ids.contains(id) {
                save_group(&transaction, key, id, ledger)?;
                continue;
            }
            save_header_group(&transaction, key, id, ledger)?;
        }
        #[cfg(test)]
        let _benchmark_commit =
            crate::conversations::sqlite_benchmark::Detail::new("append_commit");
        transaction.commit().map_err(error)?;
        Ok(self.connection.total_changes() != changed_before)
    }

    pub(crate) fn change_state(
        &mut self,
        environment: &Environment,
        id: &str,
        state: SessionState,
    ) -> Result<bool, String> {
        self.require_active()?;
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Err("This saved conversation is unavailable.".into());
        };
        let state = codecs::json(&state)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let record_id:Option<i64>=transaction.query_row("SELECT r.record_id FROM session_groups g JOIN session_records r ON r.session_key=g.session_key WHERE g.environment_key=?1 AND g.native_session_id=?2 AND r.variant_key IS NULL AND r.exclusion_root_id IS NULL",params![key,id],|row|row.get(0)).optional().map_err(error)?;
        let record_id = record_id.ok_or("This saved conversation is unavailable.")?;
        let changed = transaction
            .execute(
                "UPDATE session_records SET state=?2 WHERE record_id=?1 AND state IS NOT ?2",
                params![record_id, state.trim_matches('"')],
            )
            .map_err(error)?;
        transaction.commit().map_err(error)?;
        Ok(changed != 0)
    }

    pub(crate) fn prepare_claude(&mut self, environment: &Environment) -> Result<bool, String> {
        self.require_active()?;
        if environment.agent_kind != AgentKind::ClaudeCode {
            return Ok(false);
        }
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok(false);
        };
        let version: i64 = self
            .connection
            .query_row(
                "SELECT claude_pricing_version FROM usage_environments WHERE environment_key=?1",
                [key],
                |row| row.get(0),
            )
            .map_err(error)?;
        if version > CLAUDE_PRICING_VERSION {
            return Err("The saved Claude pricing policy is newer than this build. Use a current SQLite-capable build.".into());
        }
        if version == CLAUDE_PRICING_VERSION {
            return Ok(false);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let mut statement=transaction.prepare("SELECT r.record_id,g.native_session_id FROM session_groups g JOIN session_records r ON r.session_key=g.session_key WHERE g.environment_key=?1 ORDER BY r.record_id").map_err(error)?;
        let records = statement
            .query_map([key], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(error)?;
        for record in records {
            let (record_id, id) = record.map_err(error)?;
            let mut record = codecs::read_record(&transaction, record_id)?;
            if crate::conversations::prepare_saved_claude(&mut record) {
                save_record(&transaction, record_id, &id, &record)?;
            }
        }
        drop(statement);
        transaction
            .execute(
                "UPDATE usage_environments SET claude_pricing_version=?2 WHERE environment_key=?1",
                params![key, CLAUDE_PRICING_VERSION],
            )
            .map_err(error)?;
        transaction.commit().map_err(error)?;
        Ok(true)
    }
}

fn save_header_group(
    connection: &Connection,
    key: i64,
    id: &str,
    ledger: &Ledger,
) -> Result<(), String> {
    let group = codecs::ensure_group(connection, key, id, ledger.variants.contains_key(id))?;
    if let Some(record) = ledger.sessions.get(id) {
        save_header(connection, group, id, None, record)?;
    }
    if let Some(variants) = ledger.variants.get(id) {
        for (slot, record) in variants {
            save_header(connection, group, id, Some(slot), record)?;
        }
    }
    Ok(())
}

fn save_header(
    connection: &Connection,
    group: i64,
    id: &str,
    slot: Option<&str>,
    record: &SessionRecord,
) -> Result<(), String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("save_header");
    let previous = connection
        .query_row(
            "SELECT * FROM session_records WHERE session_key=?1 AND variant_key IS ?2",
            params![group, slot],
            |row| {
                Ok((
                    row.get::<_, i64>("record_id")?,
                    codecs::read_record_metadata(row)?,
                ))
            },
        )
        .optional()
        .map_err(error)?;
    let Some((record_id, mut previous)) = previous else {
        if !record.turns.is_empty() {
            return Err("A new usage record requires its complete measured facts.".into());
        }
        let record_id = codecs::insert_record(connection, group, slot, record)?;
        return codecs::write_rollup(
            connection,
            record_id,
            &crate::conversations::record_summary::rollup(id, record)?,
        );
    };
    let mut metadata = record.clone();
    metadata.turns.clear();
    metadata.cursors.clear();
    let mut digest_metadata = previous.clone();
    digest_metadata.state = metadata.state;
    if previous.agent_kind == Some(AgentKind::ClaudeCode) && digest_metadata != metadata {
        return Err(
            "A Claude metadata change requires its stored turns to update the usage revision."
                .into(),
        );
    }
    if previous != metadata {
        codecs::update_record_metadata(connection, record_id, &metadata)?;
    }
    previous.cursors = codecs::read_cursors(connection, record_id)?;
    if previous.agent_kind == Some(AgentKind::ClaudeCode) && previous.cursors != record.cursors {
        return Err(
            "A Claude cursor change requires its stored turns to update the usage revision.".into(),
        );
    }
    save_cursors(connection, record_id, &previous, record)?;
    let old = connection
        .query_row(
            "SELECT * FROM session_rollups WHERE record_id=?1",
            [record_id],
            super::queries::read_rollup,
        )
        .optional()
        .map_err(error)?;
    let Some(old) = old else {
        return codecs::write_rollup(
            connection,
            record_id,
            &super::import_rollup::build(connection, id, record_id)?,
        );
    };
    let mut next = crate::conversations::record_summary::rollup(id, record)?;
    next.own_summary = old.own_summary;
    next.has_stored_turns = old.has_stored_turns;
    next.sourced_request_count = old.sourced_request_count;
    next.missing_turn_links = old.missing_turn_links;
    next.resolved_guest_namespace = next
        .resolved_guest_namespace
        .or(old.resolved_guest_namespace);
    if record.agent_kind == Some(AgentKind::ClaudeCode) {
        // Claude hashes its turns. Header-only writes cannot reconstruct this hash.
        next.usage_revision = old.usage_revision;
        next.offline_usage_revision = old.offline_usage_revision;
    }
    codecs::write_rollup(connection, record_id, &next)
}

fn save_group(connection: &Connection, key: i64, id: &str, ledger: &Ledger) -> Result<(), String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("save_group");
    let session_key = codecs::ensure_group(connection, key, id, ledger.variants.contains_key(id))?;
    let mut desired: Vec<(Option<String>, &SessionRecord)> = Vec::new();
    if let Some(record) = ledger.sessions.get(id) {
        desired.push((None, record));
    }
    if let Some(variants) = ledger.variants.get(id) {
        desired.extend(
            variants
                .iter()
                .map(|(slot, record)| (Some(slot.clone()), record)),
        );
    }
    let mut statement=connection.prepare("SELECT record_id,variant_key FROM session_records WHERE session_key=?1 ORDER BY record_id").map_err(error)?;
    let old = statement
        .query_map([session_key], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(error)?;
    let mut previous = Vec::new();
    for old in old {
        let (record_id, slot) = old.map_err(error)?;
        previous.push((record_id, slot, codecs::read_record(connection, record_id)?));
    }
    let mut assignments = vec![None; desired.len()];
    let mut used = BTreeSet::new();
    for mode in 0..5 {
        for (index, (slot, record)) in desired.iter().enumerate() {
            if assignments[index].is_some() {
                continue;
            }
            let candidate = previous.iter().find(|(record_id, old_slot, old)| {
                !used.contains(record_id)
                    && match mode {
                        0 => old_slot == slot && old == *record,
                        1 => old == *record,
                        2 => {
                            record.guest_namespace.is_some()
                                && old.guest_namespace == record.guest_namespace
                        }
                        3 => slot.as_deref() == Some("") && old.guest_namespace.is_none(),
                        _ => old_slot == slot,
                    }
            });
            if let Some((record_id, _, _)) = candidate {
                assignments[index] = Some(*record_id);
                used.insert(*record_id);
            }
        }
    }
    if previous
        .iter()
        .any(|(record_id, _, _)| !used.contains(record_id))
    {
        return Err(
            "A usage slot change would discard saved evidence. The transaction was cancelled."
                .into(),
        );
    }
    for (index, assigned) in assignments.iter().enumerate() {
        if let Some(record_id) = assigned.filter(|record_id| {
            previous
                .iter()
                .any(|(old_id, slot, _)| old_id == record_id && slot != &desired[index].0)
        }) {
            let mut temporary = format!("tugboat-slot-move:{record_id}");
            loop {
                let occupied:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM session_records WHERE session_key=?1 AND variant_key=?2)",params![session_key,temporary],|row|row.get(0)).map_err(error)?;
                if !occupied
                    && !desired
                        .iter()
                        .any(|(slot, _)| slot.as_deref() == Some(temporary.as_str()))
                {
                    break;
                }
                temporary.push(':');
            }
            connection
                .execute(
                    "UPDATE session_records SET variant_key=?2 WHERE record_id=?1",
                    params![record_id, temporary],
                )
                .map_err(error)?;
        }
    }
    for (index, (slot, record)) in desired.iter().enumerate() {
        if let Some(record_id) = assignments[index] {
            connection.execute("UPDATE session_records SET variant_key=?2 WHERE record_id=?1 AND variant_key IS NOT ?2",params![record_id,slot]).map_err(error)?;
            let (_, _, before) = previous
                .iter()
                .find(|(old_id, _, _)| *old_id == record_id)
                .expect("assigned stored record");
            save_record_delta(connection, record_id, id, before, record)?;
            continue;
        }
        let record_id = codecs::insert_record(connection, session_key, slot.as_deref(), record)?;
        let rollup = crate::conversations::record_summary::rollup(id, record)?;
        codecs::write_rollup(connection, record_id, &rollup)?;
    }
    Ok(())
}

pub(crate) fn save_record(
    connection: &Connection,
    record_id: i64,
    id: &str,
    record: &SessionRecord,
) -> Result<(), String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("save_record");
    let previous = codecs::read_record(connection, record_id)?;
    save_record_delta(connection, record_id, id, &previous, record)
}

fn save_record_delta(
    connection: &Connection,
    record_id: i64,
    id: &str,
    previous: &SessionRecord,
    record: &SessionRecord,
) -> Result<(), String> {
    #[cfg(test)]
    let _benchmark_detail =
        crate::conversations::sqlite_benchmark::Detail::new("save_record_delta");
    let mut before_metadata = previous.clone();
    before_metadata.turns.clear();
    before_metadata.cursors.clear();
    let mut after_metadata = record.clone();
    after_metadata.turns.clear();
    after_metadata.cursors.clear();
    if before_metadata != after_metadata {
        codecs::update_record_metadata(connection, record_id, record)?;
    }
    for (bucket, turn) in &record.turns {
        if previous.turns.get(bucket) == Some(turn) {
            continue;
        }
        let turn_key: Option<i64> = connection
            .query_row(
                "SELECT turn_key FROM usage_turns WHERE record_id=?1 AND bucket_key=?2",
                params![record_id, bucket],
                |row| row.get(0),
            )
            .optional()
            .map_err(error)?;
        let Some(turn_key) = turn_key else {
            codecs::insert_turn(connection, record_id, bucket, turn)?;
            continue;
        };
        let old = &previous.turns[bucket];
        if old.id != turn.id
            || old.status != turn.status
            || old.origin_thread_id != turn.origin_thread_id
            || old.root_turn_id != turn.root_turn_id
            || old.parent_turn_id != turn.parent_turn_id
        {
            connection.execute("UPDATE usage_turns SET native_turn_id=?2,status=?3,origin_thread_id=?4,root_turn_id=?5,parent_turn_id=?6 WHERE turn_key=?1",params![turn_key,turn.id,turn.status,turn.origin_thread_id,turn.root_turn_id,turn.parent_turn_id]).map_err(error)?;
        }
        for (ordinal, request) in turn.requests.iter().enumerate() {
            if old.requests.get(ordinal) != Some(request) {
                codecs::request_upsert(connection, turn_key, ordinal, request)?;
            }
        }
        if turn.requests.len() < old.requests.len() {
            connection
                .execute(
                    "DELETE FROM usage_requests WHERE turn_key=?1 AND ordinal>=?2",
                    params![turn_key, i64::try_from(turn.requests.len()).map_err(error)?],
                )
                .map_err(error)?;
        }
    }
    for bucket in previous
        .turns
        .keys()
        .filter(|bucket| !record.turns.contains_key(*bucket))
    {
        connection
            .execute(
                "DELETE FROM usage_turns WHERE record_id=?1 AND bucket_key=?2",
                params![record_id, bucket],
            )
            .map_err(error)?;
    }
    save_cursors(connection, record_id, previous, record)?;
    codecs::write_rollup(
        connection,
        record_id,
        &crate::conversations::record_summary::rollup(id, record)?,
    )
}

fn save_cursors(
    connection: &Connection,
    record_id: i64,
    previous: &SessionRecord,
    record: &SessionRecord,
) -> Result<(), String> {
    #[cfg(test)]
    let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("save_cursors");
    for (path, cursor) in &record.cursors {
        if previous.cursors.get(path) == Some(cursor) {
            continue;
        }
        connection.execute("INSERT INTO source_cursors(record_id,source_path,cursor_json) VALUES(?1,?2,?3) ON CONFLICT(record_id,source_path) DO UPDATE SET cursor_json=excluded.cursor_json",params![record_id,path.as_os_str().as_bytes(),codecs::json(cursor)?]).map_err(error)?;
    }
    for path in previous
        .cursors
        .keys()
        .filter(|path| !record.cursors.contains_key(*path))
    {
        connection
            .execute(
                "DELETE FROM source_cursors WHERE record_id=?1 AND source_path=?2",
                params![record_id, path.as_os_str().as_bytes()],
            )
            .map_err(error)?;
    }
    Ok(())
}
