use super::*;
use crate::usage_store::{Environment, SharedStore};

impl environment_history::Target {
    pub(super) fn usage_environment(&self) -> Result<Environment, String> {
        Environment::new(
            self.project_id.clone(),
            self.environment_id.clone(),
            self.agent_kind,
        )
    }
}

impl Watch {
    pub(super) fn prepare_store(&self) -> Result<(), String> {
        if let (Some(target), Some(store)) = (&self.target, &self.store) {
            let environment = target.usage_environment()?;
            store.with(|store| store.prepare_claude(&environment).map(|_| ()))?;
        }
        Ok(())
    }
    pub(super) fn stored_metadata(&self) -> Result<Ledger, String> {
        if let (Some(target), Some(store)) = (&self.target, &self.store) {
            let environment = target.usage_environment()?;
            return store.with(|store| {
                store.prepare_claude(&environment)?;
                store.load_metadata(&environment).map(|value| value.0)
            });
        }
        self.selected_ledger()
    }
    pub(super) fn saved_dashboard(&self) -> Result<Dashboard, String> {
        if let (Some(target), Some(store)) = (&self.target, &self.store) {
            return dashboard(target, store, true);
        }
        #[cfg(test)]
        return environment_history::saved_with_records(
            self.target.as_ref(),
            &self.ledger,
            self.records_directory.as_ref(),
        );
        #[cfg(not(test))]
        Err("Usage storage is not ready. Reopen the verified records folder.".into())
    }
    pub(super) fn legacy_lock(&self) -> Result<Option<storage::FileLock>, String> {
        if self.store.is_some() {
            return Ok(None);
        }
        #[cfg(test)]
        return storage::lock(&self.ledger).map(Some);
        #[cfg(not(test))]
        Err("Usage storage is not ready. Reopen the verified records folder.".into())
    }

    pub(super) fn load(&self) -> Result<Ledger, String> {
        self.check_storage()?;
        if let Some(store) = &self.store {
            let environment = self
                .target
                .as_ref()
                .ok_or("The usage environment is missing.")?
                .usage_environment()?;
            return store.with(|store| {
                store.prepare_claude(&environment)?;
                store.load_environment(&environment)
            });
        }
        #[cfg(test)]
        return load_ledger(&self.ledger);
        #[cfg(not(test))]
        Err("Usage storage is not ready. Reopen the verified records folder.".into())
    }

    pub(super) fn save(&self, ledger: &mut Ledger) -> Result<bool, String> {
        self.check_storage()?;
        if let Some(store) = &self.store {
            let environment = self
                .target
                .as_ref()
                .ok_or("The usage environment is missing.")?
                .usage_environment()?;
            return store.with(|store| store.save_environment(&environment, ledger));
        }
        #[cfg(test)]
        {
            write_ledger(&self.ledger, ledger)?;
            Ok(true)
        }
        #[cfg(not(test))]
        Err("Usage storage is not ready. Reopen the verified records folder.".into())
    }
}

pub(super) fn change_state(
    watch: &Watch,
    session_id: &str,
    state: SessionState,
    running: impl Fn(&[String]) -> bool,
) -> Result<(), String> {
    watch.check_storage()?;
    let target = watch
        .target
        .as_ref()
        .ok_or("The usage environment is missing.")?;
    let store = watch.store.as_ref().ok_or("Usage storage is not ready.")?;
    let environment = target.usage_environment()?;
    let (mut metadata, mut rollups) = store.with(|store| {
        store.prepare_claude(&environment)?;
        store.load_metadata(&environment)
    })?;
    let namespace = watch.namespace();
    let selected = crate::usage_store::RecordSlot {
        session_id: session_id.into(),
        variant_key: None,
    };
    let different_guest = rollups
        .get(&selected)
        .and_then(|rollup| rollup.resolved_guest_namespace.as_ref())
        .is_some_and(|guest| guest != &namespace);
    let historical_guest = metadata
        .variants
        .get(session_id)
        .is_some_and(|variants| variants.contains_key(&namespace))
        && metadata
            .sessions
            .get(session_id)
            .and_then(|record| record.guest_namespace.as_ref())
            != Some(&namespace);
    if different_guest || historical_guest {
        synchronize(watch)?;
        (metadata, rollups) = store.with(|store| store.load_metadata(&environment))?;
    }
    let graph = sql_projection::graph(&metadata, &rollups);
    let record = project_record(&metadata, session_id)
        .ok_or("This saved session does not belong to the selected project.")?;
    if record.lineage.kind == lineage::Kind::Subagent {
        return Err("Subagents are managed through their root chat.".into());
    }
    if graph.relation(session_id).status == graph::Status::Invalid || graph.blocked(session_id) {
        return Err(
            "This saved session has an invalid relationship. Refresh the session list.".into(),
        );
    }
    let family: Vec<_> = std::iter::once(session_id.to_owned())
        .chain(graph.descendants(session_id))
        .collect();
    if state != SessionState::Active && running(&family) {
        return Err(
            "End this chat's root and subagent terminal processes before archiving or deleting it."
                .into(),
        );
    }
    store.with(|store| {
        store
            .change_state(&environment, session_id, state)
            .map(|_| ())
    })
}

fn potential_family(ledger: &Ledger, session_id: &str) -> BTreeSet<String> {
    let mut descendants = BTreeSet::from([session_id.to_owned()]);
    loop {
        let children: Vec<_> = ledger
            .sessions
            .iter()
            .filter(|(_, record)| {
                record
                    .lineage
                    .parent_session_id
                    .as_ref()
                    .is_some_and(|parent| descendants.contains(parent))
            })
            .map(|(id, _)| id.clone())
            .collect();
        let before = descendants.len();
        descendants.extend(children);
        if before == descendants.len() {
            break;
        }
    }
    let mut current = session_id;
    for _ in 0..64 {
        let Some(parent) = ledger
            .sessions
            .get(current)
            .and_then(|record| record.lineage.parent_session_id.as_deref())
        else {
            break;
        };
        if !descendants.insert(parent.into()) {
            break;
        }
        current = parent;
    }
    descendants
}

pub(super) fn details(
    target: &environment_history::Target,
    store: &SharedStore,
    session_id: &str,
    guest: impl FnOnce(bool) -> Result<Option<String>, String>,
) -> Result<ConversationUsageDetails, String> {
    let environment = target.usage_environment()?;
    let (mut metadata, mut rollups) = store.with(|store| {
        store.prepare_claude(&environment)?;
        store.load_metadata(&environment)
    })?;
    let namespace = guest(!metadata.variants.is_empty())?;
    let historical_slots: BTreeMap<_, _> = namespace
        .as_ref()
        .map(|namespace| {
            metadata
                .variants
                .iter()
                .filter(|(id, variants)| {
                    metadata
                        .sessions
                        .get(*id)
                        .and_then(|record| record.guest_namespace.as_ref())
                        != Some(namespace)
                        && variants.contains_key(namespace)
                })
                .map(|(id, _)| (id.clone(), namespace.clone()))
                .collect()
        })
        .unwrap_or_default();
    for (id, key) in &historical_slots {
        let source = crate::usage_store::RecordSlot {
            session_id: id.clone(),
            variant_key: Some(key.clone()),
        };
        if let Some(rollup) = rollups.get(&source).cloned() {
            rollups.insert(
                crate::usage_store::RecordSlot {
                    session_id: id.clone(),
                    variant_key: None,
                },
                rollup,
            );
        }
    }
    let mut hydrated = BTreeSet::new();
    if let Some(namespace) = &namespace {
        let candidates = potential_family(&metadata, session_id);
        let unresolved: Vec<_> = candidates
            .into_iter()
            .filter(|id| {
                metadata
                    .sessions
                    .get(id)
                    .is_some_and(|record| record.guest_namespace.is_none())
            })
            .collect();
        let facts = store.with(|store| store.load_records(&environment, &unresolved))?;
        hydrated.extend(unresolved);
        metadata.sessions.extend(facts.sessions);
        metadata.variants.extend(facts.variants);
        metadata.select_guest(namespace);
    }
    let graph = sql_projection::graph(&metadata, &rollups);
    project_record(&metadata, session_id).ok_or("Saved session not found in this project.")?;
    let mut members: BTreeSet<_> = std::iter::once(session_id.to_owned())
        .chain(graph.descendants(session_id))
        .collect();
    if let Some(root) = graph.relation(session_id).root_session_id {
        members.insert(root);
    }
    let slots: Vec<_> = members
        .iter()
        .filter(|id| !hydrated.contains(*id))
        .map(|id| crate::usage_store::RecordSlot {
            session_id: id.clone(),
            variant_key: historical_slots.get(id).cloned(),
        })
        .collect();
    let mut facts = store.with(|store| store.load_slots(&environment, &slots))?;
    for slot in slots {
        let record = match slot.variant_key {
            Some(key) => facts
                .variants
                .get_mut(&slot.session_id)
                .and_then(|variants| variants.remove(&key)),
            None => facts.sessions.remove(&slot.session_id),
        }
        .ok_or("The selected guest usage record is unavailable.")?;
        metadata.sessions.insert(slot.session_id, record);
    }
    usage_details::from_ledger_graph(&metadata, session_id, &graph)
}

pub(super) fn dashboard(
    target: &environment_history::Target,
    store: &SharedStore,
    offline: bool,
) -> Result<Dashboard, String> {
    let environment = target.usage_environment()?;
    let (mut metadata, mut rollups) = store.with(|store| {
        store.prepare_claude(&environment)?;
        store.load_metadata(&environment)
    })?;
    if offline {
        for record in metadata.sessions.values_mut() {
            record.observe_source(SourceState::Unavailable);
        }
        for rollup in rollups.values_mut() {
            rollup.usage_revision = rollup.offline_usage_revision.clone();
        }
    }
    let mut dashboard = sql_projection::dashboard(metadata, rollups, HashMap::new())?;
    target.annotate(&mut dashboard);
    Ok(dashboard)
}
