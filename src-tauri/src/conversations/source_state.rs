use super::SessionRecord;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum SourceState {
    Available,
    Unavailable,
}

impl SessionRecord {
    pub(super) fn source_state(&self) -> SourceState {
        if self.available {
            return SourceState::Available;
        }
        SourceState::Unavailable
    }

    pub(super) fn observe_source(&mut self, state: SourceState) -> bool {
        let changed = self.source_state() != state;
        self.available = state == SourceState::Available;
        changed
    }
}
