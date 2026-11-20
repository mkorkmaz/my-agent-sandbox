use crate::storage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: u64 = 24 * 60 * 60;

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Schedule {
    checked: Option<u64>,
    deferred_until: Option<u64>,
    pub manifest: Option<Value>,
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Schedule {
    pub fn automatic_due(&self, now: u64) -> bool {
        self.checked
            .is_none_or(|last| now.saturating_sub(last) >= DAY)
            && self.deferred_until.is_none_or(|until| now >= until)
    }

    pub fn claim(&mut self, now: u64, manual: bool) -> bool {
        if !manual && !self.automatic_due(now) {
            return false;
        }
        self.checked = Some(now);
        self.manifest = None;
        true
    }

    pub fn defer(&mut self, now: u64) {
        self.deferred_until = Some(now.saturating_add(DAY));
    }

    pub fn claim_file(
        file: &Path,
        now: u64,
        manual: bool,
    ) -> Result<(bool, Option<Value>), String> {
        let _guard = storage::lock(file)?;
        let mut schedule: Self = storage::read(file)?.unwrap_or_default();
        let claimed = schedule.claim(now, manual);
        if claimed {
            storage::write(file, &schedule)?;
        }
        Ok((claimed, schedule.manifest))
    }

    pub fn edit<T>(file: &Path, change: impl FnOnce(&mut Self) -> T) -> Result<T, String> {
        let _guard = storage::lock(file)?;
        let mut schedule: Self = storage::read(file)?.unwrap_or_default();
        let result = change(&mut schedule);
        storage::write(file, &schedule)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_and_deferrals_survive_serialization_and_manual_checks_run_immediately() {
        let mut schedule = Schedule::default();
        assert!(schedule.claim(100, false));
        let saved = serde_json::to_vec(&schedule).unwrap();
        let mut restarted: Schedule = serde_json::from_slice(&saved).unwrap();
        assert!(!restarted.claim(100 + DAY - 1, false));
        assert!(restarted.claim(100 + DAY, false));
        restarted.defer(101 + DAY);
        assert!(restarted.claim(200 + DAY, true));
        assert!(!restarted.claim(199 + DAY * 2, false));
        assert!(restarted.claim(200 + DAY * 2, false));
        restarted.defer(210 + DAY * 2);
        assert!(!restarted.claim(209 + DAY * 3, false));
        assert!(restarted.claim(210 + DAY * 3, false));
    }
}
