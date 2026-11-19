use crate::projects::{EnvironmentId, Project};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type AccountKey = (String, EnvironmentId);

#[derive(Default)]
pub struct Accounts {
    operations: Mutex<HashMap<AccountKey, Arc<tauri::async_runtime::Mutex<()>>>>,
}

impl Accounts {
    pub fn operation(
        &self,
        project: &Project,
    ) -> Result<Arc<tauri::async_runtime::Mutex<()>>, String> {
        Ok(self
            .operations
            .lock()
            .map_err(|_| "Could not lock this Claude account.")?
            .entry((project.id.clone(), project.environment_id.clone()))
            .or_default()
            .clone())
    }
}
