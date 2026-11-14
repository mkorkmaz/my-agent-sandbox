use rusqlite::{
    types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef},
    ToSql,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum TurnStatus {
    #[default]
    Unspecified,
    InProgress,
    Complete,
    Interrupted,
    Unknown,
    Unrecognized(String),
}

impl From<String> for TurnStatus {
    fn from(value: String) -> Self {
        match value.as_str() {
            "" => Self::Unspecified,
            "in-progress" => Self::InProgress,
            "complete" => Self::Complete,
            "interrupted" => Self::Interrupted,
            "unknown" => Self::Unknown,
            _ => Self::Unrecognized(value),
        }
    }
}
impl From<&str> for TurnStatus {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}
impl From<TurnStatus> for String {
    fn from(state: TurnStatus) -> Self {
        state.as_str().into()
    }
}
impl TurnStatus {
    pub fn observe_completion(&mut self, next: Self) -> bool {
        if !matches!(next, Self::Complete | Self::Interrupted)
            || matches!(self, Self::Complete | Self::Interrupted)
        {
            return false;
        }
        *self = next;
        true
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Unspecified => "",
            Self::InProgress => "in-progress",
            Self::Complete => "complete",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
            Self::Unrecognized(value) => value,
        }
    }
}
impl ToSql for TurnStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(
            self.as_str().as_bytes(),
        )))
    }
}
impl FromSql for TurnStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(Self::from(value.as_str()?))
    }
}

#[cfg(test)]
impl PartialEq<&str> for TurnStatus {
    fn eq(&self, value: &&str) -> bool {
        self.as_str() == *value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_turn_results_ignore_conflicting_replay_and_reopening() {
        for terminal in [TurnStatus::Complete, TurnStatus::Interrupted] {
            let mut status = TurnStatus::InProgress;
            assert!(status.observe_completion(terminal.clone()));
            for replay in [TurnStatus::Complete, TurnStatus::Interrupted, TurnStatus::InProgress] {
                assert!(!status.observe_completion(replay));
                assert_eq!(status, terminal);
            }
        }
    }
    #[test]
    fn saved_turn_states_round_trip_without_losing_unrecognized_legacy_values() {
        for value in [
            "",
            "in-progress",
            "complete",
            "interrupted",
            "unknown",
            "legacy-state",
        ] {
            let state: TurnStatus = serde_json::from_value(value.into()).unwrap();
            assert_eq!(serde_json::to_value(&state).unwrap(), value);
            assert_eq!(
                TurnStatus::column_result(ValueRef::Text(value.as_bytes())).unwrap(),
                state
            );
        }
    }
}
