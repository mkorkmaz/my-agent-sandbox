use super::{text, valid_id, Cursor};
use serde_json::Value;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum OwnershipPhase {
    Unclaimed,
    Own,
    CopiedAncestor,
    Blocked,
}

impl Cursor {
    pub(super) fn ownership_phase(&self) -> OwnershipPhase {
        let Some(owner) = &self.owner else {
            return OwnershipPhase::Unclaimed;
        };
        if self.expected_owner.as_ref().is_some_and(|expected| expected != owner) {
            return OwnershipPhase::Blocked;
        }
        match &self.active_owner {
            Some(active) if active == owner => OwnershipPhase::Own,
            Some(_) => OwnershipPhase::CopiedAncestor,
            None => OwnershipPhase::Blocked,
        }
    }

    pub(super) fn accept_owner(&mut self, id: &String, payload: &Value) -> bool {
        if self.ownership_phase() == OwnershipPhase::Unclaimed {
            self.owner = Some(id.clone());
            self.active_owner = Some(id.clone());
            self.version = text(&payload["cli_version"]);
            self.fork_origin = valid_id(&payload["forked_from_id"]);
            if self
                .expected_owner
                .as_ref()
                .is_some_and(|expected| expected != id)
            {
                self.active_owner = None;
                self.ownership_problem = Some("Session owner changed while reading".into());
                return false;
            }
            return true;
        }
        if self.owner.as_ref() == Some(id) {
            return true;
        }
        if self.fork_origin.as_ref() != Some(id) {
            self.active_owner = None;
            self.ownership_problem = Some("Copied metadata has no verified fork owner".into());
            return false;
        }
        self.active_owner = Some(id.clone());
        true
    }

    pub(super) fn restore_owner(&mut self, id: &str) {
        if self.owner.as_deref() != Some(id)
            || self.expected_owner.as_deref().is_some_and(|expected| expected != id)
        {
            return;
        }
        self.active_owner = Some(id.into());
    }
}
