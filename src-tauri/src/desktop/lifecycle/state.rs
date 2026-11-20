use std::thread::ThreadId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShutdownState {
    Open,
    Confirming,
    Draining,
    StoppingEnvironments(ThreadId),
    ClosingStorage,
    ReportingFailure,
    WindowCloseAllowed,
    ExitAllowed,
}

impl ShutdownState {
    pub fn abort(&mut self) {
        if matches!(
            self,
            Self::Confirming
                | Self::Draining
                | Self::StoppingEnvironments(_)
                | Self::ClosingStorage
                | Self::ReportingFailure
                | Self::WindowCloseAllowed
        ) {
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

    pub fn stop_environments(&mut self, worker: ThreadId) -> Result<(), String> {
        if *self != Self::Draining {
            return Err(
                "The app must finish draining processes before it stops environments.".into(),
            );
        }
        *self = Self::StoppingEnvironments(worker);
        Ok(())
    }

    pub fn environments_stopped(&mut self, worker: ThreadId) -> Result<(), String> {
        if *self != Self::StoppingEnvironments(worker) {
            return Err("The app could not confirm environment shutdown.".into());
        }
        *self = Self::ClosingStorage;
        Ok(())
    }

    pub fn blocks_commands(self, caller: ThreadId) -> bool {
        match self {
            Self::StoppingEnvironments(worker) => worker != caller,
            state => state.is_closing(),
        }
    }

    pub fn report_failure(&mut self) {
        if matches!(
            self,
            Self::Draining | Self::StoppingEnvironments(_) | Self::ClosingStorage
        ) {
            *self = Self::ReportingFailure;
        }
    }

    pub fn approve(&mut self) -> Result<(), String> {
        if *self != Self::ClosingStorage {
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
            Self::Draining
                | Self::StoppingEnvironments(_)
                | Self::ClosingStorage
                | Self::WindowCloseAllowed
                | Self::ExitAllowed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_shutdown_worker_can_run_commands_while_stopping_guests() {
        let worker = std::thread::current().id();
        let other = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        let mut state = ShutdownState::Open;
        assert!(state.request());
        state.abort();
        assert!(state.stop_environments(worker).is_err());
        assert!(state.request());
        assert!(state.begin());
        assert!(state.blocks_commands(worker));
        assert!(state.approve().is_err());
        state.stop_environments(worker).unwrap();
        assert!(state.is_closing());
        assert!(!state.blocks_commands(worker));
        assert!(state.blocks_commands(other));
        assert!(!state.allows_exit());
        assert!(state.approve().is_err());
        assert!(state.environments_stopped(other).is_err());
        state.report_failure();
        assert!(!state.allows_exit());
        state.abort();
        assert!(state.request());
    }

    #[test]
    fn a_failed_shutdown_can_retry_after_the_alert_is_dismissed() {
        let mut state = ShutdownState::Open;
        assert!(state.request());
        assert!(state.begin());
        state.report_failure();
        assert!(!state.is_closing());
        assert!(!state.allows_exit());
        assert!(!state.request());
        state.abort();
        assert!(state.request());
        assert!(state.begin());
        let worker = std::thread::current().id();
        state.stop_environments(worker).unwrap();
        state.environments_stopped(worker).unwrap();
        state.approve().unwrap();
        assert!(state.allows_exit());
    }

    #[test]
    fn a_failed_dispatch_allows_a_fresh_shutdown_but_cannot_revoke_committed_exit() {
        let mut state = ShutdownState::Open;
        assert!(state.request());
        assert!(state.begin());
        state.abort();
        assert!(state.request());
        assert!(state.begin());
        let worker = std::thread::current().id();
        state.stop_environments(worker).unwrap();
        state.environments_stopped(worker).unwrap();
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
        let worker = std::thread::current().id();
        state.stop_environments(worker).unwrap();
        state.environments_stopped(worker).unwrap();
        assert!(state.blocks_commands(worker));
        state.approve().unwrap();
        assert!(state.consume_window_close());
        assert!(!state.consume_window_close());
        assert!(state.allows_exit());
    }
}
