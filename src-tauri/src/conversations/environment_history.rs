use super::*;

#[derive(Clone)]
pub(super) struct Target {
    pub project_id: String,
    pub environment_id: projects::EnvironmentId,
    pub agent_kind: AgentKind,
}

impl Target {
    pub fn for_project(project: &projects::Project) -> Self {
        Self {
            project_id: project.id.clone(),
            environment_id: project.environment_id.clone(),
            agent_kind: project.agent_kind,
        }
    }
    pub fn key(&self) -> String {
        format!("{}|{}", self.project_id, self.environment_id)
    }
    pub fn annotate(&self, dashboard: &mut Dashboard) {
        for conversation in &mut dashboard.conversations {
            conversation.environment_id = Some(self.environment_id.clone());
            conversation.agent_kind = self.agent_kind;
        }
    }
}

pub(super) fn empty() -> Dashboard {
    Dashboard {
        usage_available: true,
        conversations: Vec::new(),
        usage: usage::Summary::default(),
        pricing_date: usage::PRICING_DATE.into(),
        anthropic_pricing_date: usage::anthropic::PRICING_DATE.into(),
        warnings: Vec::new(),
        coverage: usage::Coverage::Unknown,
    }
}

pub(super) fn combine(dashboards: impl Iterator<Item = Dashboard>) -> Dashboard {
    let mut result = empty();
    let mut coverage = Vec::new();
    for dashboard in dashboards {
        result.usage_available &= dashboard.usage_available;
        result.usage.add(&dashboard.usage);
        if !dashboard.conversations.is_empty() || dashboard.usage.requests > 0 {
            coverage.push(dashboard.coverage);
        }
        result.conversations.extend(dashboard.conversations);
        result.warnings.extend(dashboard.warnings);
    }
    result.coverage = combined_coverage(&coverage);
    if !result.usage_available && result.usage.requests > 0 {
        result.coverage = usage::Coverage::Partial;
    }
    result.conversations.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| {
                left.environment_id
                    .as_ref()
                    .map(ToString::to_string)
                    .cmp(&right.environment_id.as_ref().map(ToString::to_string))
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    result.warnings.sort();
    result.warnings.dedup();
    result
}

pub(super) fn saved_snapshot(
    targets: &[Target],
    store: &crate::usage_store::SharedStore,
) -> Result<Dashboard, String> {
    let mut dashboards = Vec::new();
    for target in targets {
        let dashboard = sql_backend::dashboard(target, store, true)?;
        dashboards.push(HistoryObservation::Saved(dashboard).dashboard(None));
    }
    let mut dashboard = combine(dashboards.into_iter());
    dashboard
        .warnings
        .push("Showing saved sessions while checking for updates.".into());
    Ok(dashboard)
}

pub(super) fn recorded_snapshot(
    targets: &[Target],
    store: &crate::usage_store::SharedStore,
) -> Result<Dashboard, String> {
    let dashboards = targets
        .iter()
        .map(|target| sql_backend::dashboard(target, store, false))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(combine(dashboards.into_iter()))
}

pub(super) fn retain_verified_availability(recorded: &mut Dashboard, verified: Option<&Dashboard>) {
    let available: HashMap<_, _> = verified
        .into_iter()
        .flat_map(|dashboard| &dashboard.conversations)
        .map(|conversation| {
            (
                (conversation.environment_id.clone(), conversation.id.clone()),
                conversation.available,
            )
        })
        .collect();
    for conversation in &mut recorded.conversations {
        let key = (conversation.environment_id.clone(), conversation.id.clone());
        conversation.available &= available.get(&key).copied().unwrap_or(false);
    }
    if let Some(verified) = verified {
        let warnings: Vec<_> = verified
            .warnings
            .iter()
            .filter(|warning| !recorded.warnings.contains(warning))
            .cloned()
            .collect();
        recorded.warnings.extend(warnings);
    }
}

fn matches_project(key: &str, project: &str) -> bool {
    key == project
        || key
            .strip_prefix(project)
            .is_some_and(|suffix| suffix.starts_with('|'))
}

impl History {
    pub(super) fn merged(&self, project: &str) -> Result<Dashboard, String> {
        let latest = self.latest.lock().map_err(|error| error.to_string())?;
        Ok(combine(
            latest
                .iter()
                .filter(|(key, _)| matches_project(key, project))
                .map(|(_, dashboard)| dashboard.clone()),
        ))
    }
    pub(super) fn saved_result(
        &self,
        key: &str,
        target: &Target,
        store: &crate::usage_store::SharedStore,
        message: String,
    ) -> Result<(), String> {
        let dashboard = sql_backend::dashboard(target, store, true)?;
        self.record_result(key, HistoryObservation::Saved(dashboard).dashboard(Some(message)))
    }
    pub(super) fn retain_project_keys(
        &self,
        project: &str,
        keys: &HashSet<String>,
    ) -> Result<(), String> {
        self.watches
            .lock()
            .map_err(|error| error.to_string())?
            .retain(|key, _| !matches_project(key, project) || keys.contains(key));
        self.latest
            .lock()
            .map_err(|error| error.to_string())?
            .retain(|key, _| !matches_project(key, project) || keys.contains(key));
        Ok(())
    }
    pub(super) fn record_result(&self, key: &str, dashboard: Dashboard) -> Result<(), String> {
        self.latest
            .lock()
            .map_err(|error| error.to_string())?
            .insert(key.into(), dashboard);
        Ok(())
    }
    pub(super) fn refresh(&self, key: &str, watch: &Watch) -> Result<(), String> {
        watch.prepare_store()?;
        let (observation, warning) = match synchronize(watch) {
            Ok((dashboard, _)) => (HistoryObservation::Fresh(dashboard), None),
            Err(message) => (self.recover_observation(key, watch)?, Some(message)),
        };
        self.record_result(key, observation.dashboard(warning))
    }
    fn recover_observation(&self, key: &str, watch: &Watch) -> Result<HistoryObservation, String> {
        let previous = self.latest.lock().map_err(|error| error.to_string())?.get(key).cloned();
        if let Some(dashboard) = previous { return Ok(HistoryObservation::Retained(dashboard)); }
        match watch.saved_dashboard() {
            Ok(dashboard) => Ok(HistoryObservation::Saved(dashboard)),
            Err(_) => Ok(HistoryObservation::Unavailable),
        }
    }
    pub(super) fn remove(
        &self,
        project: &str,
        environment: Option<&projects::EnvironmentId>,
        flush: bool,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|error| error.to_string())?;
        let selected = environment.map(|environment| format!("{project}|{environment}"));
        let matches = |key: &str| {
            selected
                .as_ref()
                .map_or_else(|| matches_project(key, project), |selected| key == selected)
        };
        let mut watches = self.watches.lock().map_err(|error| error.to_string())?;
        if flush {
            for (_, watch) in watches.iter().filter(|(key, _)| matches(key)) {
                synchronize(watch)?;
            }
        }
        watches.retain(|key, _| !matches(key));
        self.latest
            .lock()
            .map_err(|error| error.to_string())?
            .retain(|key, _| !matches(key));
        self.published
            .lock()
            .map_err(|error| error.to_string())?
            .remove(project);
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn saved(target: Option<&Target>, file: &Path) -> Result<Dashboard, String> {
    saved_with_records(target, file, None)
}

#[cfg(test)]
pub(super) fn saved_with_records(
    target: Option<&Target>,
    file: &Path,
    records: Option<&crate::storage::data_location::RecordsDirectory>,
) -> Result<Dashboard, String> {
    let kind = target
        .map(|target| target.agent_kind)
        .unwrap_or(AgentKind::Codex);
    let mut ledger = match records {
        Some(records) => load_saved_ledger(kind, file, records)?,
        None => load_ledger(file)?,
    };
    for record in ledger.sessions.values_mut() {
        record.observe_source(SourceState::Unavailable);
        if target.is_some_and(|target| target.agent_kind == AgentKind::ClaudeCode) {
            claude::usage::prepare_saved(record);
        }
    }
    let mut dashboard = dashboard_from_ledger(ledger, HashMap::new())?;
    if let Some(target) = target {
        target.annotate(&mut dashboard);
    }
    Ok(dashboard)
}

// Backfill without a guest or API call. The existing atomic writer and immutable
// v1 backup rules also apply when the source history is offline.
#[cfg(test)]
pub(super) fn load_saved_ledger(
    kind: AgentKind,
    file: &Path,
    records: &crate::storage::data_location::RecordsDirectory,
) -> Result<Ledger, String> {
    records.verify()?;
    if kind != AgentKind::ClaudeCode {
        return load_ledger(file);
    }
    if storage::read_regular_bytes(file)?.is_none() {
        return Ok(Ledger::default());
    }
    records.verify()?;
    let _guard = storage::lock(file)?;
    records.verify()?;
    let mut ledger = load_ledger(file)?;
    let previous = serde_json::to_value(&ledger).map_err(|error| error.to_string())?;
    for record in ledger.sessions.values_mut().chain(
        ledger
            .variants
            .values_mut()
            .flat_map(|records| records.values_mut()),
    ) {
        claude::usage::prepare(record);
    }
    if previous != serde_json::to_value(&ledger).map_err(|error| error.to_string())? {
        records.verify()?;
        write_ledger(file, &mut ledger)?;
    }
    Ok(ledger)
}

pub(super) fn bind(app: &AppHandle, identifier: &str, dashboard: &Dashboard) -> Result<(), String> {
    let codex: Vec<_> = dashboard
        .conversations
        .iter()
        .filter(|conversation| conversation.agent_kind == AgentKind::Codex)
        .cloned()
        .collect();
    if terminal::bind_conversations(identifier, &codex) {
        app.emit("terminal-sessions-changed", identifier)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
