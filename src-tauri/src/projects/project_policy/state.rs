use super::Action;
use crate::projects::{Project, ProjectStage};

#[derive(Clone, Copy)]
enum DocumentState {
    Editable,
    ReadOnly,
}

#[derive(Clone, Copy)]
enum BindingState {
    AttachedOwned,
    AttachedExternal,
    DetachedOwned,
    DetachedExternal,
}

pub(super) struct PolicyState {
    document: DocumentState,
    binding: BindingState,
}

impl PolicyState {
    pub(super) fn already_applied(&self, action: Action) -> bool {
        matches!((action, self.binding),
            (Action::Attach, BindingState::AttachedOwned | BindingState::AttachedExternal)
            | (Action::Detach, BindingState::DetachedOwned | BindingState::DetachedExternal))
    }

    pub(super) fn observed(editable: bool, attached: bool, owned: bool) -> Self {
        let document = match editable {
            true => DocumentState::Editable,
            false => DocumentState::ReadOnly,
        };
        let binding = match (attached, owned) {
            (true, true) => BindingState::AttachedOwned,
            (true, false) => BindingState::AttachedExternal,
            (false, true) => BindingState::DetachedOwned,
            (false, false) => BindingState::DetachedExternal,
        };
        Self { document, binding }
    }

    pub(super) fn authorize(&self, action: Action) -> Result<(), String> {
        if matches!(
            (action, self.document),
            (Action::Save, DocumentState::ReadOnly)
        ) {
            return Err("This policy is externally managed or shared with other sessions. The document was preserved.".into());
        }
        if matches!(action, Action::Detach)
            && matches!(
                self.binding,
                BindingState::AttachedExternal | BindingState::DetachedExternal
            )
        {
            return Err("The policy binding created outside the app was preserved.".into());
        }
        Ok(())
    }

    pub(super) fn begin(&self, project: &mut Project, action: Action) {
        if matches!(action, Action::Detach) {
            project.policy_detached = true;
        }
        project.environment_verified = Some(false);
        project.environment_attempt = None;
        project.stage = ProjectStage::EditingPolicy;
    }

    pub(super) fn claim_attachment(&self, project: &mut Project, action: Action) -> bool {
        if !matches!(action, Action::Attach)
            || !matches!(
                self.binding,
                BindingState::DetachedOwned | BindingState::DetachedExternal
            )
        {
            return false;
        }
        project.owns_binding = true;
        true
    }

    pub(super) fn complete(&self, project: &mut Project, action: Action) {
        if matches!(action, Action::Detach) {
            project.owns_binding = false;
        }
        if matches!(action, Action::Attach) {
            project.policy_detached = false;
        }
        project.stage = ProjectStage::Ready;
        if project.policy_detached {
            project.stage = ProjectStage::PolicyDetached;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_documents_and_bindings_cannot_cross_owned_edit_transitions() {
        let external = PolicyState::observed(false, true, false);
        assert!(external.authorize(Action::Save).is_err());
        assert!(external.authorize(Action::Detach).is_err());
        assert!(external.authorize(Action::Accept).is_ok());
        assert!(external.authorize(Action::Attach).is_ok());
        let owned = PolicyState::observed(true, false, true);
        assert!(owned.authorize(Action::Save).is_ok());
        assert!(owned.authorize(Action::Detach).is_ok());
        assert!(PolicyState::observed(false, false, true)
            .authorize(Action::Save)
            .is_err());
    }
}
