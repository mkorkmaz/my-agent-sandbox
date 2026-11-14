use super::Store;
use super::connection::ReconnectDecision;
use std::sync::{Arc, Condvar, Mutex};
use tauri::AppHandle;

#[derive(Clone)]
pub(crate) struct SharedStore(Arc<Mutex<Option<Store>>>);

impl SharedStore {
    pub(crate) fn new(store: Store) -> Self {
        Self(Arc::new(Mutex::new(Some(store))))
    }

    pub(crate) fn with<T>(
        &self,
        work: impl FnOnce(&mut Store) -> Result<T, String>,
    ) -> Result<T, String> {
        #[cfg(test)]
        let _benchmark_phase = crate::conversations::sqlite_benchmark::Phase::backend();
        let mut guard = self.0.lock().map_err(|error| error.to_string())?;
        let store = guard
            .as_mut()
            .ok_or("Usage storage is closed. Restart Tugboat to reopen its verified records.")?;
        if let Err(failure) = store.prepare_access() {
            store.note_failure(&failure);
            return Err(failure);
        }
        let result = work(store);
        if let Err(failure) = &result {
            store.note_failure(failure);
        }
        result
    }

    pub(crate) fn reconnect(
        &self,
        records: &crate::storage::data_location::RecordsDirectory,
    ) -> Result<(), String> {
        let mut guard = self.0.lock().map_err(|error| error.to_string())?;
        let current = guard
            .as_ref()
            .ok_or("The usage writer is closed. Restart Tugboat to reconnect it.")?;
        match current.reconnect_decision(records) {
            ReconnectDecision::Current => return Ok(()),
            ReconnectDecision::OriginalAvailable => return Err("The original records folder is available again. Cancel the pending Locate operation before closing Tugboat.".into()),
            ReconnectDecision::TransactionPending => return Err("Finish the pending usage transaction before reconnecting records.".into()),
            ReconnectDecision::Replace => {},
        }
        let target = Store::open(records)?;
        target.require_active()?;
        target.integrity_check()?;
        let current = guard.take().expect("verified usage handle");
        if let Err((current, error)) = current.try_close_for_reconnect(&target) {
            *guard = Some(current);
            return Err(error);
        }
        *guard = Some(target);
        Ok(())
    }

    pub(crate) fn close(&self) -> Result<(), String> {
        let mut guard = self.0.lock().map_err(|error| error.to_string())?;
        if let Some(store) = guard.as_ref() {
            store.verify()?;
        }
        let Some(store) = guard.take() else {
            return Ok(());
        };
        match store.try_close() {
            Ok(()) => Ok(()),
            Err((store, error)) => {
                *guard = Some(store);
                Err(error)
            }
        }
    }
}

enum State {
    Pending,
    Ready(SharedStore),
    CloseFailed(SharedStore, String),
    Failed(String),
    Closed,
}

#[derive(Clone)]
pub(crate) struct Runtime(Arc<(Mutex<State>, Condvar)>);

impl Default for Runtime {
    fn default() -> Self {
        Self(Arc::new((Mutex::new(State::Pending), Condvar::new())))
    }
}

impl Runtime {
    pub(crate) fn start(&self, app: AppHandle) {
        let runtime = self.clone();
        std::thread::spawn(move || {
            let result =
                crate::storage::data_location::records_directory(&app).and_then(|records| {
                    records.verify()?;
                    let registry =
                        crate::projects::Registry::load(&records.path().join("projects.json"))?;
                    crate::storage::data_location::require_private_records(
                        records.path(),
                        &registry,
                    )?;
                    super::importer::activate(&records)?;
                    Store::open(&records).map(SharedStore::new)
                });
            runtime.complete(result);
        });
    }

    fn complete(&self, result: Result<SharedStore, String>) {
        let (state, changed) = &*self.0;
        let Ok(mut state) = state.lock() else {
            return;
        };
        *state = match result {
            Ok(store) => State::Ready(store),
            Err(error) => State::Failed(format!(
                "Usage storage is unavailable. Preserve the records folder and reconnect or recover it in Settings. {error}"
            )),
        };
        changed.notify_all();
    }

    pub(crate) fn wait(&self) -> Result<SharedStore, String> {
        let (state, changed) = &*self.0;
        let mut state = state.lock().map_err(|error| error.to_string())?;
        loop {
            match &*state {
                State::Ready(store) | State::CloseFailed(store, _) => return Ok(store.clone()),
                State::Failed(error) => return Err(error.clone()),
                State::Closed => return Err("Usage storage is closed. Restart Tugboat.".into()),
                State::Pending => {
                    state = changed.wait(state).map_err(|error| error.to_string())?;
                }
            }
        }
    }

    pub(crate) fn existing_store(&self) -> Result<Option<SharedStore>, String> {
        let (state, changed) = &*self.0;
        let mut state = state.lock().map_err(|error| error.to_string())?;
        while matches!(*state, State::Pending) {
            state = changed.wait(state).map_err(|error| error.to_string())?;
        }
        match &*state {
            State::Ready(store) | State::CloseFailed(store, _) => Ok(Some(store.clone())),
            _ => Ok(None),
        }
    }

    pub(crate) fn failure(&self) -> Option<String> {
        let state = self.0 .0.lock().ok()?;
        match &*state {
            State::Failed(error) | State::CloseFailed(_, error) => Some(error.clone()),
            _ => None,
        }
    }

    pub(crate) fn close(&self) -> Result<(), String> {
        let (state, changed) = &*self.0;
        let mut state = state.lock().map_err(|error| error.to_string())?;
        while matches!(*state, State::Pending) {
            state = changed.wait(state).map_err(|error| error.to_string())?;
        }
        let store = match &*state {
            State::Ready(store) | State::CloseFailed(store, _) => store.clone(),
            _ => {
                *state = State::Closed;
                return Ok(());
            }
        };
        let result = store.close();
        *state = match &result {
            Ok(()) => State::Closed,
            Err(error) => State::CloseFailed(store, format!(
                "Could not close usage storage. Reconnect its original records folder and retry closing Tugboat. {error}"
            )),
        };
        changed.notify_all();
        result
    }
}

#[cfg(test)]
mod tests;
