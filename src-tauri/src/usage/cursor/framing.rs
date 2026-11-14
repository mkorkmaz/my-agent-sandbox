use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "bool", into = "bool")]
pub(super) enum RecordFraming {
    #[default]
    Records,
    SkipOversized,
}

impl From<bool> for RecordFraming {
    fn from(skipping: bool) -> Self {
        if skipping {
            return Self::SkipOversized;
        }
        Self::Records
    }
}
impl From<RecordFraming> for bool {
    fn from(state: RecordFraming) -> Self {
        state == RecordFraming::SkipOversized
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "bool", into = "bool")]
pub(super) enum ReadProgress {
    #[default]
    Current,
    WaitingForBytes,
}

impl From<bool> for ReadProgress {
    fn from(pending: bool) -> Self {
        if pending {
            return Self::WaitingForBytes;
        }
        Self::Current
    }
}
impl From<ReadProgress> for bool {
    fn from(state: ReadProgress) -> Self {
        state == ReadProgress::WaitingForBytes
    }
}
