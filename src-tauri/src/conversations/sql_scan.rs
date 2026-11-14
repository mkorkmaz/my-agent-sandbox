use super::*;

pub(super) struct ScanState {
    loaded_ids: BTreeSet<String>,
    origin: ScanOrigin,
}

enum ScanOrigin {
    Sql { headers: Ledger, preparation: PreparationEffect },
    Legacy { #[cfg(test)] initial_ledger: Option<Ledger> },
}

enum PreparationEffect { Unchanged, Updated }
impl From<bool> for PreparationEffect {
    fn from(changed: bool) -> Self { if changed { return Self::Updated; } Self::Unchanged }
}

#[derive(Eq, PartialEq)]
enum HydrationState { HeadersOnly, Loaded }

impl Watch {
    pub(super) fn claude_owner_ids(&self, scan: &HistoryScan) -> Result<Vec<String>, String> {
        let (Some(target), Some(store)) = (&self.target, &self.store) else {
            return Ok(Vec::new());
        };
        let identities: BTreeSet<_> = scan
            .turns
            .values()
            .flat_map(|turns| turns.values())
            .flat_map(|turn| &turn.requests)
            .filter_map(|request| request.source.as_ref())
            .map(|source| {
                (
                    source.guest_namespace.clone(),
                    source.model_provider.clone(),
                    source.response_id.clone(),
                )
            })
            .collect();
        let environment = target.usage_environment()?;
        store.with(|store| store.claude_request_owner_ids(&environment, identities))
    }
    pub(super) fn scan_ledger(&self) -> Result<(Ledger, ScanState), String> {
        self.check_storage()?;
        if let (Some(target), Some(store)) = (&self.target, &self.store) {
            let environment = target.usage_environment()?;
            let (ledger, prepared) = store.with(|store| {
                let claude_changed = store.prepare_claude(&environment)?;
                let codex_changed = store.prepare_codex(&environment)?;
                let ledger = store.load_scan_state(&environment)?.0;
                Ok((ledger, claude_changed || codex_changed))
            })?;
            let origin = ScanOrigin::Sql { headers: ledger.clone(), preparation: prepared.into() };
            return Ok((
                ledger,
                ScanState {
                    loaded_ids: BTreeSet::new(),
                    origin,
                },
            ));
        }
        let ledger = self.load()?;
        let loaded_ids = ledger
            .sessions
            .keys()
            .chain(ledger.variants.keys())
            .cloned()
            .collect();
        #[cfg(test)]
        let initial_ledger = Some(ledger.clone());
        Ok((
            ledger,
            ScanState {
                loaded_ids,
                origin: ScanOrigin::Legacy { #[cfg(test)] initial_ledger },
            },
        ))
    }
}

impl ScanState {
    fn changed_records(&self, mut ledger: Ledger) -> Ledger {
        let ScanOrigin::Sql { headers: initial, .. } = &self.origin else {
            return ledger;
        };
        let ids: BTreeSet<_> = ledger
            .sessions
            .keys()
            .chain(ledger.variants.keys())
            .filter(|id| {
                self.loaded_ids.contains(*id)
                    || ledger.sessions.get(*id) != initial.sessions.get(*id)
                    || ledger.variants.get(*id) != initial.variants.get(*id)
            })
            .cloned()
            .collect();
        ledger.sessions.retain(|id, _| ids.contains(id));
        ledger.variants.retain(|id, _| ids.contains(id));
        ledger
    }
    pub(super) fn is_loaded(&self, id: &str) -> bool {
        self.hydration_state(id) == HydrationState::Loaded
    }
    fn hydration_state(&self, id: &str) -> HydrationState {
        if self.loaded_ids.contains(id) { return HydrationState::Loaded; }
        HydrationState::HeadersOnly
    }
    pub(super) fn sync_codex_source(
        &self,
        id: &str,
        source: &Path,
        namespace: &str,
        cursor: &mut usage::Cursor,
        turns: &mut BTreeMap<String, usage::TurnUsage>,
    ) -> Result<bool, String> {
        if !self.is_loaded(id) {
            // A source can grow after preflight. Leave its cursor untouched;
            // the next pass loads its stored turns before reading changed bytes.
            return Ok(false);
        }
        usage::sync_file_bound(source, namespace, id, cursor, turns)
    }
    pub(super) fn prepare_selection(
        &mut self,
        watch: &Watch,
        ledger: &mut Ledger,
        namespace: &str,
    ) -> Result<(), String> {
        let ids: BTreeSet<_> = ledger
            .sessions
            .iter()
            .filter(|(_, record)| record.guest_namespace.as_deref() != Some(namespace))
            .map(|(id, _)| id.clone())
            .chain(
                ledger
                    .variants
                    .iter()
                    .filter(|(id, _)| {
                        ledger
                            .sessions
                            .get(*id)
                            .and_then(|record| record.guest_namespace.as_deref())
                            != Some(namespace)
                    })
                    .map(|(id, _)| id.clone()),
            )
            .collect();
        self.hydrate(watch, ledger, ids, None)
    }

    pub(super) fn hydrate(
        &mut self,
        watch: &Watch,
        ledger: &mut Ledger,
        ids: impl IntoIterator<Item = String>,
        namespace: Option<&str>,
    ) -> Result<(), String> {
        let ids: Vec<_> = ids
            .into_iter()
            .filter(|id| !self.loaded_ids.contains(id))
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let (Some(target), Some(store)) = (&watch.target, &watch.store) else {
            self.loaded_ids.extend(ids);
            return Ok(());
        };
        let environment = target.usage_environment()?;
        let mut facts = store.with(|store| store.load_records(&environment, &ids))?;
        if let Some(namespace) = namespace {
            facts.select_guest(namespace);
        }
        for id in &ids {
            if let Some(record) = facts.sessions.remove(id) {
                match ledger.sessions.get_mut(id) {
                    Some(metadata) => metadata.turns = record.turns,
                    None => {
                        ledger.sessions.insert(id.clone(), record);
                    }
                }
            }
            if let Some(variants) = facts.variants.remove(id) {
                let saved = ledger.variants.entry(id.clone()).or_default();
                for (slot, record) in variants {
                    match saved.get_mut(&slot) {
                        Some(metadata) => metadata.turns = record.turns,
                        None => {
                            saved.insert(slot, record);
                        }
                    }
                }
            }
        }
        self.loaded_ids.extend(ids);
        Ok(())
    }

    pub(super) fn finish(
        self,
        watch: &Watch,
        mut ledger: Ledger,
        sources: HashMap<String, Conversation>,
        changed: bool,
    ) -> Result<(Dashboard, bool), String> {
        if let (Some(target), Some(store)) = (&watch.target, &watch.store) {
            let environment = target.usage_environment()?;
            if changed {
                ledger = self.changed_records(ledger);
            }
            let (changed, metadata, rollups) = store.with(|store| {
                let changed = changed
                    && store.save_partial_environment(&environment, &ledger, &self.loaded_ids)?;
                let (metadata, rollups) = store.load_metadata(&environment)?;
                Ok((changed, metadata, rollups))
            })?;
            return Ok((
                sql_projection::dashboard(metadata, rollups, sources)?,
                changed || matches!(&self.origin, ScanOrigin::Sql { preparation: PreparationEffect::Updated, .. }),
            ));
        }
        #[cfg(test)]
        let changed = changed
            && (ledger.schema_version != 2 || !matches!(&self.origin, ScanOrigin::Legacy { initial_ledger: Some(initial) } if initial == &ledger));
        if changed {
            watch.save(&mut ledger)?;
        }
        Ok((dashboard_from_ledger(ledger, sources)?, changed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    const OWNER: &str = "30000000-0000-4000-8000-000000000101";
    const TURN: &str = "40000000-0000-4000-8000-000000000201";
    const NAMESPACE: &str = "generated-preflight-race-guest";

    fn tokens(input: u64, output: u64) -> Value {
        json!({"input_tokens":input,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input+output})
    }

    fn append_request(file: &mut fs::File, response: &str, occurrence: u64) {
        let measured = tokens(10, 2);
        let cumulative = tokens(10 * occurrence, 2 * occurrence);
        let exact = json!({"type":"token_usage_record","timestamp":response,"payload":{"thread_id":OWNER,"turn_id":TURN,"root_turn_id":TURN,"response_id":response,"usage":measured,"thread_token_usage":cumulative}});
        let counter = json!({"type":"event_msg","timestamp":response,"payload":{"type":"token_count","info":{"last_token_usage":measured,"total_token_usage":cumulative}}});
        writeln!(file, "{exact}\n{counter}").unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn source_growth_after_preflight_defers_cursor_until_saved_turns_are_loaded() {
        let nonce = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tugboat-preflight-race-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("rollout.jsonl");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&source)
            .unwrap();
        for event in [
            json!({"type":"session_meta","payload":{"id":OWNER,"cwd":"/work/generated-fixture","cli_version":"0.167.0","model_provider":"openai","source":"cli"}}),
            json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_id":OWNER,"thread_settings":{"service_tier":"default"}}}),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":TURN}}),
            json!({"type":"turn_context","payload":{"turn_id":TURN,"model":"gpt-6.1-sol"}}),
        ] {
            writeln!(file, "{event}").unwrap();
        }
        append_request(&mut file, "request-one", 1);
        let mut cursor = usage::Cursor::default();
        let mut stored_turns = BTreeMap::new();
        assert!(
            usage::sync_file_bound(&source, NAMESPACE, OWNER, &mut cursor, &mut stored_turns)
                .unwrap()
        );
        assert_eq!(
            usage::summarize(&usage::own_turns(&stored_turns, OWNER)).requests,
            1
        );
        assert!(cursor.is_current_file(&source, NAMESPACE, OWNER).unwrap());

        let saved_cursor = cursor.clone();
        let mut state = ScanState {
            loaded_ids: BTreeSet::new(),
            origin: ScanOrigin::Legacy { initial_ledger: None },
        };
        let mut metadata_only_turns = BTreeMap::new();
        append_request(&mut file, "request-two", 2);
        assert!(!cursor.is_current_file(&source, NAMESPACE, OWNER).unwrap());
        assert!(!state
            .sync_codex_source(
                OWNER,
                &source,
                NAMESPACE,
                &mut cursor,
                &mut metadata_only_turns
            )
            .unwrap());
        assert_eq!(cursor, saved_cursor);
        assert!(metadata_only_turns.is_empty());
        assert_eq!(
            usage::summarize(&usage::own_turns(&stored_turns, OWNER)).requests,
            1
        );

        state.loaded_ids.insert(OWNER.into());
        assert!(state
            .sync_codex_source(OWNER, &source, NAMESPACE, &mut cursor, &mut stored_turns)
            .unwrap());
        let summary = usage::summarize(&usage::own_turns(&stored_turns, OWNER));
        assert_eq!(
            (summary.requests, summary.input, summary.output),
            (2, 20, 4)
        );
        assert_ne!(cursor, saved_cursor);
        assert!(cursor.is_current_file(&source, NAMESPACE, OWNER).unwrap());
        let committed_cursor = cursor.clone();
        let committed_turns = serde_json::to_value(&stored_turns).unwrap();
        assert!(!state
            .sync_codex_source(OWNER, &source, NAMESPACE, &mut cursor, &mut stored_turns)
            .unwrap());
        assert_eq!(cursor, committed_cursor);
        assert_eq!(
            serde_json::to_value(&stored_turns).unwrap(),
            committed_turns
        );
        drop(file);
        fs::remove_dir_all(directory).unwrap();
    }
}
