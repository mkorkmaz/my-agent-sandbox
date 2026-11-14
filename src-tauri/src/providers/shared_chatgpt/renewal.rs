use super::store::Session;
use crate::storage::data_location::RecordsDirectory;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) enum CredentialWrite {
    #[default]
    Clean,
    Pending {
        records: RecordsDirectory,
        session: Session,
    },
}

#[derive(Default)]
enum RetryPhase {
    #[default]
    Ready,
    Waiting(Instant),
}

#[derive(Default)]
pub(crate) struct RenewalRetry {
    phase: RetryPhase,
    failures: u32,
}

impl RenewalRetry {
    pub fn waiting(&self, now: Instant) -> bool {
        matches!(self.phase, RetryPhase::Waiting(deadline) if deadline > now)
    }

    pub fn exhausted(&self) -> bool { self.failures >= 5 }

    pub fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1).min(5);
        self.phase =
            RetryPhase::Waiting(now + Duration::from_secs((10u64 << self.failures).min(300)));
    }

    pub fn succeeded(&mut self) {
        *self = Self::default();
    }
    pub fn allow_retry(&mut self) {
        self.phase = RetryPhase::Ready;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renewal_retry_is_bounded_and_success_resets_its_failure_history() {
        let now = Instant::now();
        let mut retry = RenewalRetry::default();
        assert!(!retry.waiting(now));
        retry.failed(now);
        assert!(retry.waiting(now + Duration::from_secs(19)));
        assert!(!retry.waiting(now + Duration::from_secs(20)));
        for _ in 0..20 {
            retry.failed(now);
        }
        assert!(retry.waiting(now + Duration::from_secs(299)));
        assert!(!retry.waiting(now + Duration::from_secs(300)));
        retry.succeeded();
        retry.failed(now);
        assert!(!retry.waiting(now + Duration::from_secs(20)));
    }
}
