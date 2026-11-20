use crate::{platform::brig::Brig, projects::Project};
use serde_json::Value;
use std::path::Path;

pub(in crate::projects) struct SandboxInventory(Vec<Value>);

impl SandboxInventory {
    pub(in crate::projects) fn read(brig: &Brig) -> Result<Self, String> {
        let inventory = brig.json(&["--json".into(), "ls".into()], "SandboxList", None)?;
        let data = &inventory["data"];
        if data["runtime"]["kind"] != "hull" || data["runtime"]["available"] != true {
            return Err(
                "Brig could not verify the Hull runtime. Check the environment and retry.".into(),
            );
        }
        let rows = data["sandboxes"]
            .as_array()
            .ok_or("Could not read the Brig sandbox list.")?;
        if rows
            .iter()
            .any(|row| row["sandbox"].as_str().is_none() || row["state"].as_str().is_none())
        {
            return Err("The Brig sandbox list contains an incomplete entry.".into());
        }
        Ok(Self(rows.clone()))
    }

    pub(in crate::projects) fn running(
        &self,
        project: &Project,
        sandbox: &str,
        home: &Path,
    ) -> Result<Option<bool>, String> {
        let reference = project.reference();
        let mut targets = self
            .0
            .iter()
            .filter(|row| row["ref"] == reference || row["sandbox"] == sandbox);
        let Some(target) = targets.next() else {
            return Ok(None);
        };
        if targets.next().is_some()
            || target["ref"] != reference
            || target["sandbox"] != sandbox
            || target["workspace"].as_str() != home.to_str()
        {
            return Err("The Brig guest does not match the selected project. Review the session before continuing.".into());
        }
        match target["state"].as_str() {
            Some("running") => Ok(Some(true)),
            Some("stopped") => Ok(Some(false)),
            _ => Err(
                "Could not verify the project's Brig guest. Check the environment and retry."
                    .into(),
            ),
        }
    }
}
