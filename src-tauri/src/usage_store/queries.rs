use super::{codecs, error, Environment, RecordSlot, Rollup, Store};
use crate::conversations::Ledger;
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};

impl Store {
    pub(crate) fn claude_request_owner_ids(
        &self,
        environment: &Environment,
        identities: BTreeSet<(String, String, String)>,
    ) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT DISTINCT g.native_session_id FROM usage_requests q INDEXED BY ix_request_source
                 JOIN usage_turns t ON t.turn_key=q.turn_key
                 JOIN session_records r ON r.record_id=t.record_id
                 JOIN session_groups g ON g.session_key=r.session_key
                 JOIN usage_environments e ON e.environment_key=g.environment_key
                 WHERE q.source_guest=?1 AND q.model_provider=?2 AND q.response_id=?3
                 AND q.response_id IS NOT NULL AND r.variant_key IS NULL
                 AND e.project_id=?4 AND e.environment_id=?5"
            )
            .map_err(error)?;
        let mut ids = BTreeSet::new();
        for (guest, provider, response) in identities {
            let rows = statement
                .query_map(
                    params![
                        guest,
                        provider,
                        response,
                        environment.project_id,
                        environment.environment_id.to_string()
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(error)?;
            for row in rows {
                ids.insert(row.map_err(error)?);
            }
        }
        Ok(ids.into_iter().collect())
    }
    pub(crate) fn load_environment(&self, environment: &Environment) -> Result<Ledger, String> {
        self.verify()?;
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok(Ledger::default());
        };
        let mut ledger = environment_ledger(&self.connection, key)?;
        self.normalize_runtime_version(&mut ledger)?;
        self.load_groups(key, &mut ledger, None, true)?;
        Ok(ledger)
    }

    pub(crate) fn load_metadata(
        &self,
        environment: &Environment,
    ) -> Result<(Ledger, BTreeMap<RecordSlot, Rollup>), String> {
        #[cfg(test)]
        let _benchmark_detail =
            crate::conversations::sqlite_benchmark::Detail::new("load_metadata");
        self.verify()?;
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok((Ledger::default(), BTreeMap::new()));
        };
        let mut ledger = environment_ledger(&self.connection, key)?;
        self.normalize_runtime_version(&mut ledger)?;
        self.load_groups(key, &mut ledger, None, false)?;
        let mut rollups = BTreeMap::new();
        let mut statement=self.connection.prepare_cached("SELECT g.native_session_id,r.variant_key,r.record_id,p.* FROM session_groups g JOIN session_records r ON r.session_key=g.session_key LEFT JOIN session_rollups p ON p.record_id=r.record_id WHERE g.environment_key=?1 ORDER BY g.native_session_id,r.variant_key").map_err(error)?;
        let entries = statement
            .query_map([key], |row| {
                Ok((
                    RecordSlot {
                        session_id: row.get(0)?,
                        variant_key: row.get(1)?,
                    },
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>("own_summary_json")?,
                    if row.get::<_, Option<String>>("own_summary_json")?.is_some() {
                        Some(read_rollup(row)?)
                    } else {
                        None
                    },
                ))
            })
            .map_err(error)?;
        for entry in entries {
            let (slot, record_id, _, rollup) = entry.map_err(error)?;
            let rollup = match rollup {
                Some(rollup) => rollup,
                None => {
                    self.require_active()?;
                    let transaction = rusqlite::Transaction::new_unchecked(
                        &self.connection,
                        rusqlite::TransactionBehavior::Immediate,
                    )
                    .map_err(error)?;
                    let rollup =
                        super::import_rollup::build(&transaction, &slot.session_id, record_id)?;
                    codecs::write_rollup(&transaction, record_id, &rollup)?;
                    transaction.commit().map_err(error)?;
                    rollup
                }
            };
            rollups.insert(slot, rollup);
        }
        Ok((ledger, rollups))
    }

    pub(crate) fn load_records(
        &self,
        environment: &Environment,
        ids: &[String],
    ) -> Result<Ledger, String> {
        #[cfg(test)]
        let _benchmark_detail = crate::conversations::sqlite_benchmark::Detail::new("load_records");
        self.verify()?;
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok(Ledger::default());
        };
        let mut ledger = environment_ledger(&self.connection, key)?;
        self.normalize_runtime_version(&mut ledger)?;
        self.load_groups(key, &mut ledger, Some(ids), true)?;
        Ok(ledger)
    }

    pub(crate) fn load_slots(
        &self,
        environment: &Environment,
        slots: &[RecordSlot],
    ) -> Result<Ledger, String> {
        self.verify()?;
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok(Ledger::default());
        };
        let mut ledger = environment_ledger(&self.connection, key)?;
        self.normalize_runtime_version(&mut ledger)?;
        let mut statement=self.connection.prepare_cached("SELECT r.*,g.native_session_id,g.has_variants_bucket FROM session_groups g JOIN session_records r ON r.session_key=g.session_key WHERE g.environment_key=?1 AND g.native_session_id=?2 AND r.variant_key IS ?3").map_err(error)?;
        for slot in slots {
            let entry = statement
                .query_row(params![key, slot.session_id, slot.variant_key], |row| {
                    Ok((
                        row.get::<_, i64>("record_id")?,
                        row.get::<_, bool>("has_variants_bucket")?,
                    ))
                })
                .optional()
                .map_err(error)?;
            let Some((record_id, has_variants_bucket)) = entry else {
                return Err(
                    "The requested saved usage slot is unavailable. Refresh the conversation list."
                        .into(),
                );
            };
            if has_variants_bucket {
                ledger.variants.entry(slot.session_id.clone()).or_default();
            }
            codecs::add_record(
                &mut ledger,
                slot.session_id.clone(),
                slot.variant_key.clone(),
                codecs::read_record(&self.connection, record_id)?,
            );
        }
        Ok(ledger)
    }

    fn normalize_runtime_version(&self, ledger: &mut Ledger) -> Result<(), String> {
        if self.marker_version()? == 2 {
            self.require_active()?;
            ledger.schema_version = 2;
        }
        Ok(())
    }

    pub(crate) fn load_scan_state(
        &self,
        environment: &Environment,
    ) -> Result<(Ledger, BTreeMap<RecordSlot, Rollup>), String> {
        #[cfg(test)]
        let _benchmark_detail =
            crate::conversations::sqlite_benchmark::Detail::new("load_scan_state");
        let (mut ledger, rollups) = self.load_metadata(environment)?;
        let Some(key) = codecs::environment_key(&self.connection, environment)? else {
            return Ok((ledger, rollups));
        };
        let mut statement=self.connection.prepare_cached("SELECT g.native_session_id,r.variant_key,c.source_path,c.cursor_json FROM session_groups g JOIN session_records r ON r.session_key=g.session_key JOIN source_cursors c ON c.record_id=r.record_id WHERE g.environment_key=?1 ORDER BY g.native_session_id,r.variant_key,c.source_path").map_err(error)?;
        let entries = statement
            .query_map([key], |row| {
                use std::os::unix::ffi::OsStringExt;
                let bytes: Vec<u8> = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes)),
                    codecs::decode_cursor(row, "cursor_json")?,
                ))
            })
            .map_err(error)?;
        for entry in entries {
            let (id, variant, path, cursor) = entry.map_err(error)?;
            let record = match variant {
                Some(variant) => ledger
                    .variants
                    .get_mut(&id)
                    .and_then(|variants| variants.get_mut(&variant)),
                None => ledger.sessions.get_mut(&id),
            }
            .ok_or("A usage cursor has no saved record.")?;
            record.cursors.insert(path, cursor);
        }
        Ok((ledger, rollups))
    }

    fn load_groups(
        &self,
        key: i64,
        ledger: &mut Ledger,
        ids: Option<&[String]>,
        facts: bool,
    ) -> Result<(), String> {
        let mut groups=self.connection.prepare_cached("SELECT native_session_id,has_variants_bucket FROM session_groups WHERE environment_key=?1 ORDER BY native_session_id").map_err(error)?;
        for group in groups
            .query_map([key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
            })
            .map_err(error)?
        {
            let (id, variants) = group.map_err(error)?;
            if ids.is_some_and(|ids| !ids.contains(&id)) {
                continue;
            }
            if variants {
                ledger.variants.entry(id).or_default();
            }
        }
        let base="SELECT r.*,g.native_session_id FROM session_groups g JOIN session_records r ON r.session_key=g.session_key WHERE g.environment_key=?1";
        let query = if ids.is_some() {
            format!("{base} AND g.native_session_id=?2 ORDER BY r.variant_key")
        } else {
            format!("{base} ORDER BY g.native_session_id,r.variant_key")
        };
        let mut statement = self.connection.prepare_cached(&query).map_err(error)?;
        if let Some(ids) = ids {
            for id in ids {
                let mut entries = statement.query(params![key, id]).map_err(error)?;
                self.append_rows(ledger, &mut entries, facts)?;
            }
            return Ok(());
        }
        let mut entries = statement.query([key]).map_err(error)?;
        self.append_rows(ledger, &mut entries, facts)
    }

    fn append_rows(
        &self,
        ledger: &mut Ledger,
        entries: &mut rusqlite::Rows<'_>,
        facts: bool,
    ) -> Result<(), String> {
        while let Some(row) = entries.next().map_err(error)? {
            let id = row.get("native_session_id").map_err(error)?;
            let variant = row.get("variant_key").map_err(error)?;
            let record_id = row.get("record_id").map_err(error)?;
            let record = if facts {
                codecs::read_record(&self.connection, record_id)?
            } else {
                codecs::read_record_metadata(row).map_err(error)?
            };
            codecs::add_record(ledger, id, variant, record);
        }
        Ok(())
    }
}

fn environment_ledger(connection: &rusqlite::Connection, key: i64) -> Result<Ledger, String> {
    let warnings_json: String = connection
        .prepare_cached("SELECT warnings_json FROM usage_environments WHERE environment_key=?1")
        .map_err(error)?
        .query_row([key], |row| row.get(0))
        .map_err(error)?;
    let version: Option<u32> = connection
        .prepare_cached("SELECT ledger_version FROM legacy_imports WHERE environment_key=?1")
        .map_err(error)?
        .query_row([key], |row| row.get(0))
        .optional()
        .map_err(error)?;
    Ok(Ledger {
        schema_version: version.unwrap_or(2),
        warnings: serde_json::from_str(&warnings_json).map_err(error)?,
        ..Ledger::default()
    })
}

pub(crate) fn read_rollup(row: &rusqlite::Row<'_>) -> rusqlite::Result<Rollup> {
    let stored: super::StoredSummary = codecs::decode(row, "own_summary_json")?;
    Ok(Rollup {
        own_summary: stored.summary,
        offline_usage_revision: stored.offline_usage_revision,
        resolved_guest_namespace: row.get("resolved_guest_namespace")?,
        has_stored_turns: row.get("has_stored_turns")?,
        sourced_request_count: codecs::unsigned(row, "sourced_request_count")?,
        missing_turn_links: row.get("missing_turn_links")?,
        has_unread_bytes: row.get("has_unread_bytes")?,
        has_ownership_problem: row.get("has_ownership_problem")?,
        usage_revision: row.get("usage_revision")?,
    })
}
