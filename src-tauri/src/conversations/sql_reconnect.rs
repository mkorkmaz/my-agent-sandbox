use super::*;
use crate::{storage::data_location::RecordsDirectory, usage_store::SharedStore};

impl History {
    pub(crate) fn reconnect_records(
        &self,
        store: &SharedStore,
        records: &RecordsDirectory,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|error| error.to_string())?;
        store.reconnect(records)?;
        let mut watches = self.watches.lock().map_err(|error| error.to_string())?;
        for watch in watches.values_mut() {
            watch.records_directory = Some(records.clone());
            watch.store = Some(store.clone());
        }
        Ok(())
    }
}

pub(crate) fn reconnect_storage(app: &AppHandle, records: &RecordsDirectory) -> Result<(), String> {
    let store = app
        .state::<crate::usage_store::Runtime>()
        .existing_store()?
        .ok_or("The usage writer is unavailable. Restart Tugboat to reconnect records.")?;
    app.state::<History>().reconnect_records(&store, records)
}

#[cfg(test)]
pub(crate) fn fixture_history(
    records: &RecordsDirectory,
    store: &SharedStore,
    home: &Path,
) -> History {
    let history = History::default();
    let project = "1".repeat(32);
    let target = environment_history::Target {
        project_id: project.clone(),
        environment_id: projects::EnvironmentId::for_agent(&project, AgentKind::Codex),
        agent_kind: AgentKind::Codex,
    };
    let watch = Watch {
        target: Some(target.clone()),
        records_directory: Some(records.clone()),
        store: Some(store.clone()),
        home: home.into(),
        project: "/work/p".into(),
        ledger: records.path().join("retained-test-ledger.json"),
    };
    history.watches.lock().unwrap().insert(target.key(), watch);
    history
}
