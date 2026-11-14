use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAttempt {
    #[serde(default)]
    pub plan_fingerprint: Option<String>,
    pub token: String,
    pub terminal_identifier: u64,
    pub completed: bool,
    pub error: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum AttemptState<'a> {
    Pending,
    Verified,
    Failed(&'a str),
}

pub enum AttemptCompletion<'a> {
    Superseded,
    Pending,
    Recorded(Option<&'a str>),
}

impl EnvironmentAttempt {
    pub fn pending(token: String, terminal_identifier: u64, fingerprint: String) -> Self {
        Self {
            token,
            terminal_identifier,
            plan_fingerprint: Some(fingerprint),
            completed: false,
            error: None,
        }
    }

    pub fn state(&self) -> AttemptState<'_> {
        match (self.completed, self.error.as_deref()) {
            (false, _) => AttemptState::Pending,
            (true, None) => AttemptState::Verified,
            (true, Some(error)) => AttemptState::Failed(error),
        }
    }

    pub fn completion(&self, token: &str, terminal_identifier: u64) -> AttemptCompletion<'_> {
        if self.token != token || self.terminal_identifier != terminal_identifier {
            return AttemptCompletion::Superseded;
        }
        match self.state() {
            AttemptState::Pending => AttemptCompletion::Pending,
            AttemptState::Verified => AttemptCompletion::Recorded(None),
            AttemptState::Failed(error) => AttemptCompletion::Recorded(Some(error)),
        }
    }

    pub fn finish(&mut self, result: Result<(), String>) -> Result<(), String> {
        if self.state() != AttemptState::Pending {
            return Err("This preparation attempt is already complete.".into());
        }
        self.completed = true;
        self.error = result.err();
        Ok(())
    }

    pub fn verifies(&self, fingerprint: &str) -> bool {
        self.state() == AttemptState::Verified
            && self.plan_fingerprint.as_deref() == Some(fingerprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_completion_cannot_replace_a_terminal_result_or_another_attempt() {
        let mut attempt = EnvironmentAttempt::pending("attempt".into(), 7, "plan".into());
        assert!(matches!(
            attempt.completion("old", 7),
            AttemptCompletion::Superseded
        ));
        assert!(matches!(
            attempt.completion("attempt", 8),
            AttemptCompletion::Superseded
        ));
        assert!(!attempt.verifies("plan"));
        attempt.finish(Err("version failed".into())).unwrap();
        assert!(matches!(
            attempt.completion("attempt", 7),
            AttemptCompletion::Recorded(Some("version failed"))
        ));
        assert!(attempt.finish(Ok(())).is_err());
        assert!(!attempt.verifies("plan"));
        let mut retry = EnvironmentAttempt::pending("retry".into(), 8, "plan".into());
        retry.finish(Ok(())).unwrap();
        assert!(retry.verifies("plan"));
        assert!(!retry.verifies("other-plan"));
        assert_eq!(serde_json::to_value(&retry).unwrap()["completed"], true);
    }
}
