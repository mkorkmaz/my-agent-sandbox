use super::{HistoryScan, ProjectExclusion, SessionRecord};
use crate::projects::GuestProjectScope;
use std::path::PathBuf;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum MembershipState {
    Confirmed,
    RetainedExclusion,
    Unresolved,
}

enum MembershipEvidence {
    Unresolved,
    Confirmed,
    Foreign(ProjectExclusion),
}

impl MembershipEvidence {
    fn from_root(root: Option<(String, PathBuf)>, project: &GuestProjectScope) -> Self {
        let Some((root_session_id, root_cwd)) = root else {
            return Self::Unresolved;
        };
        if project.contains(&root_cwd) {
            return Self::Confirmed;
        }
        Self::Foreign(ProjectExclusion {
            root_session_id,
            root_cwd,
        })
    }
}

impl HistoryScan {
    pub(super) fn record_membership(
        &mut self,
        id: &str,
        root: Option<(String, PathBuf)>,
        project: &GuestProjectScope,
    ) {
        match MembershipEvidence::from_root(root, project) {
            MembershipEvidence::Confirmed => {
                self.positive_members.insert(id.into());
            }
            MembershipEvidence::Foreign(exclusion) => {
                self.exclusions.insert(id.into(), exclusion);
            }
            MembershipEvidence::Unresolved => {}
        }
    }

    pub(super) fn membership(&self, id: &str, saved: Option<&SessionRecord>) -> MembershipState {
        if self.positive_members.contains(id) {
            return MembershipState::Confirmed;
        }
        if saved.is_some_and(|record| record.project_exclusion.is_some()) {
            return MembershipState::RetainedExclusion;
        }
        MembershipState::Unresolved
    }
}

impl SessionRecord {
    pub(super) fn exclude(&mut self, exclusion: &ProjectExclusion) -> bool {
        let changed = self.project_exclusion.as_ref() != Some(exclusion);
        self.project_exclusion = Some(exclusion.clone());
        changed
    }

    pub(super) fn confirm_membership(&mut self, state: MembershipState) -> bool {
        if state != MembershipState::Confirmed {
            return false;
        }
        self.project_exclusion.take().is_some()
    }
}
