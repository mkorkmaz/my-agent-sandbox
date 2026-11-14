#[derive(Default)]
pub(super) enum LoginProcess {
    #[default]
    Idle,
    Running {
        nonce: String,
        identifier: u64,
    },
    Completed {
        nonce: String,
        identifier: u64,
        success: bool,
    },
    Result {
        nonce: String,
        success: bool,
    },
}

impl LoginProcess {
    pub fn attach(&mut self, nonce: String, identifier: u64) -> Result<(), String> {
        if !matches!(self, Self::Idle) {
            return Err("A ChatGPT sign-in process is already registered.".into());
        }
        *self = Self::Running { nonce, identifier };
        Ok(())
    }

    pub fn record(&mut self, candidate: &str, success: bool) {
        let (nonce, identifier) = match self {
            Self::Running { nonce, identifier }
            | Self::Completed {
                nonce, identifier, ..
            } => (nonce, identifier),
            _ => return,
        };
        if nonce != candidate {
            return;
        }
        *self = Self::Completed {
            nonce: nonce.clone(),
            identifier: *identifier,
            success,
        };
    }

    pub fn process(&self) -> Option<(&str, u64)> {
        match self {
            Self::Running { nonce, identifier }
            | Self::Completed {
                nonce, identifier, ..
            } => Some((nonce, *identifier)),
            _ => None,
        }
    }

    pub fn result(&self) -> Option<(&str, bool)> {
        match self {
            Self::Completed { nonce, success, .. } | Self::Result { nonce, success } => {
                Some((nonce, *success))
            }
            _ => None,
        }
    }

    pub fn clear_process(&mut self) {
        *self = match self.result() {
            Some((nonce, success)) => Self::Result {
                nonce: nonce.into(),
                success,
            },
            None => Self::Idle,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonce_results_require_the_registered_process_and_stop_changing_after_recovery() {
        let mut login = LoginProcess::default();
        login.attach("current".into(), 7).unwrap();
        assert!(login.attach("other".into(), 8).is_err());
        login.record("stale", true);
        assert!(login.result().is_none());
        login.record("current", true);
        login.record("current", false);
        assert_eq!(login.result(), Some(("current", false)));
        login.clear_process();
        assert!(login.process().is_none());
        login.record("current", true);
        assert_eq!(login.result(), Some(("current", false)));
    }
}
