#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShutdownState {
    Open,
    Confirming,
    Draining,
    WindowCloseAllowed,
    ExitAllowed,
}

impl ShutdownState {
    pub fn abort(&mut self) {
        if matches!(self, Self::Confirming | Self::Draining | Self::WindowCloseAllowed) {
            *self = Self::Open;
        }
    }
    pub fn request(&mut self) -> bool {
        if *self != Self::Open {
            return false;
        }
        *self = Self::Confirming;
        true
    }

    pub fn begin(&mut self) -> bool {
        if *self != Self::Confirming {
            return false;
        }
        *self = Self::Draining;
        true
    }

    pub fn approve(&mut self) -> Result<(), String> {
        if *self != Self::Draining {
            return Err("The app shutdown state changed. Retry closing the app.".into());
        }
        *self = Self::WindowCloseAllowed;
        Ok(())
    }

    pub fn consume_window_close(&mut self) -> bool {
        if *self != Self::WindowCloseAllowed {
            return false;
        }
        *self = Self::ExitAllowed;
        true
    }

    pub fn allows_exit(self) -> bool {
        matches!(self, Self::WindowCloseAllowed | Self::ExitAllowed)
    }

    pub fn is_closing(self) -> bool {
        matches!(
            self,
            Self::Draining | Self::WindowCloseAllowed | Self::ExitAllowed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_failed_dispatch_allows_a_fresh_shutdown_but_cannot_revoke_committed_exit() {
        let mut state = ShutdownState::Open;
        assert!(state.request());
        assert!(state.begin());
        state.abort();
        assert!(state.request());
        assert!(state.begin());
        state.approve().unwrap();
        assert!(state.consume_window_close());
        state.abort();
        assert!(state.allows_exit());
        assert!(!state.request());
    }
    #[test]
    fn shutdown_requires_drain_before_approval_and_consumes_window_permission_once() {
        let mut state = ShutdownState::Open;
        assert!(state.approve().is_err());
        assert!(state.request());
        assert!(!state.request());
        assert!(!state.is_closing());
        assert!(state.begin());
        assert!(state.is_closing());
        assert!(!state.allows_exit());
        state.approve().unwrap();
        assert!(state.consume_window_close());
        assert!(!state.consume_window_close());
        assert!(state.allows_exit());
    }
}
