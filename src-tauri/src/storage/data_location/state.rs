use super::{Location, PendingLocation};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "bool", into = "bool")]
pub(super) enum ChangeKind {
    Move,
    Locate,
}

impl From<bool> for ChangeKind {
    fn from(locate: bool) -> Self {
        if locate {
            return Self::Locate;
        }
        Self::Move
    }
}
impl From<ChangeKind> for bool {
    fn from(kind: ChangeKind) -> Self {
        kind == ChangeKind::Locate
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "bool", into = "bool")]
pub(super) enum MarkerState {
    Incomplete,
    Complete,
}

impl From<bool> for MarkerState {
    fn from(complete: bool) -> Self {
        if complete {
            return Self::Complete;
        }
        Self::Incomplete
    }
}
impl From<MarkerState> for bool {
    fn from(state: MarkerState) -> Self {
        state == MarkerState::Complete
    }
}

pub(super) enum MovePhase<'a> {
    Pending(&'a PendingLocation),
    Cleanup(&'a PathBuf),
    Stable,
}

impl Location {
    pub(super) fn phase(&self) -> MovePhase<'_> {
        if let Some(pending) = &self.pending {
            return MovePhase::Pending(pending);
        }
        if let Some(previous) = &self.previous_directory {
            return MovePhase::Cleanup(previous);
        }
        MovePhase::Stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_location_states_keep_the_existing_boolean_wire_format() {
        for value in [false, true] {
            let marker: MarkerState = serde_json::from_value(value.into()).unwrap();
            let kind: ChangeKind = serde_json::from_value(value.into()).unwrap();
            assert_eq!(serde_json::to_value(marker).unwrap(), value);
            assert_eq!(serde_json::to_value(kind).unwrap(), value);
        }
        assert!(serde_json::from_str::<MarkerState>("1").is_err());
    }
}
