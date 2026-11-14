use crate::{
    agents::AgentKind,
    projects::{EnvironmentId, Project},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTarget {
    pub environment_id: EnvironmentId,
    pub agent_kind: AgentKind,
}

impl AgentTarget {
    pub fn for_project(project: &Project) -> Self {
        Self {
            environment_id: project.environment_id.clone(),
            agent_kind: project.agent_kind,
        }
    }

    pub fn codex(project_id: &str) -> Self {
        Self {
            environment_id: EnvironmentId::for_agent(project_id, AgentKind::Codex),
            agent_kind: AgentKind::Codex,
        }
    }
}

pub(super) fn same_process_scope(left: Option<&AgentTarget>, right: Option<&AgentTarget>) -> bool {
    left.is_none() || right.is_none() || left == right
}

pub(super) fn is_codex(target: Option<&AgentTarget>) -> bool {
    target.is_some_and(|target| target.agent_kind == AgentKind::Codex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_locks_are_scoped_by_environment_and_host_locks_cover_all_environments() {
        let codex = AgentTarget::codex("project-one");
        let claude = AgentTarget {
            environment_id: EnvironmentId::for_agent("project-one", AgentKind::ClaudeCode),
            agent_kind: AgentKind::ClaudeCode,
        };
        assert!(same_process_scope(Some(&codex), Some(&codex)));
        assert!(!same_process_scope(Some(&codex), Some(&claude)));
        assert!(!same_process_scope(Some(&claude), Some(&codex)));
        assert!(same_process_scope(None, Some(&codex)));
        assert!(same_process_scope(Some(&claude), None));
        assert!(same_process_scope(None, None));
        assert!(is_codex(Some(&codex)));
        assert!(!is_codex(Some(&claude)));
        assert!(!is_codex(None));
    }
}
