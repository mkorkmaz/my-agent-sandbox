use serde_json::Value;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Eq, PartialEq)]
pub struct PlanPaths {
    pub host_home: PathBuf,
    pub guest_project: PathBuf,
}

impl PlanPaths {
    pub fn from_plan(plan: &Value) -> Result<Self, String> {
        let guest_project = plan["data"]["project"]["guest"]
            .as_str()
            .ok_or("The guest project path is missing.")?;
        crate::projects::validate_guest_path(guest_project)?;
        let host_home = plan["data"]["home"]["host"]
            .as_str()
            .ok_or("The plan host share is missing.")?;
        Ok(Self {
            host_home: checked_path(host_home, "The plan contains an invalid host share path.")?,
            guest_project: checked_path(guest_project, "The guest project path is unsupported.")?,
        })
    }
}

fn checked_path(value: &str, message: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute()
        || value.contains('\0')
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(message.into());
    }
    Ok(path.into())
}
