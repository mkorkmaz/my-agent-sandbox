use crate::projects::EnvironmentId;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

static ACTIVE: OnceLock<Mutex<HashSet<EnvironmentId>>> = OnceLock::new();

fn active() -> &'static Mutex<HashSet<EnvironmentId>> {
    ACTIVE.get_or_init(Default::default)
}

pub(crate) struct EnvironmentOperation(EnvironmentId);

impl EnvironmentOperation {
    pub fn begin(identifier: &EnvironmentId) -> Result<Self, String> {
        let mut operations = active().lock().map_err(|error| error.to_string())?;
        if !operations.insert(identifier.clone()) {
            return Err("This environment operation is already in progress. Wait and retry.".into());
        }
        Ok(Self(identifier.clone()))
    }

    pub fn pending(identifier: &EnvironmentId) -> bool {
        active().lock().unwrap_or_else(|error| error.into_inner()).contains(identifier)
    }
}

impl Drop for EnvironmentOperation {
    fn drop(&mut self) {
        active().lock().unwrap_or_else(|error| error.into_inner()).remove(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unpublished_starts_and_stops_share_admission_until_the_operation_returns() {
        let id = EnvironmentId::for_agent("gate-test", crate::agents::AgentKind::Codex);
        let operation = EnvironmentOperation::begin(&id).unwrap();
        assert!(EnvironmentOperation::pending(&id));
        assert!(EnvironmentOperation::begin(&id).is_err());
        drop(operation);
        assert!(!EnvironmentOperation::pending(&id));
        assert!(EnvironmentOperation::begin(&id).is_ok());
    }
}
