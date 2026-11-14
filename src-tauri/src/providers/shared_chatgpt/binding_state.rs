use super::{guest::hash, store::Binding};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum BindingPhase {
    Pending,
    Published,
}

impl Binding {
    pub(super) fn phase(&self) -> BindingPhase {
        if self.pending {
            return BindingPhase::Pending;
        }
        BindingPhase::Published
    }

    pub fn publish_receipt(&mut self, bytes: &[u8], access: &str) {
        self.auth_hash = Some(hash(bytes));
        self.access_hash = Some(hash(access.as_bytes()));
        self.pending = false;
    }
}
