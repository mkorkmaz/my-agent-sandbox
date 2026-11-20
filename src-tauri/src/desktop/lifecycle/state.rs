use super::install_location::InstallLocation;
use std::thread::ThreadId;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ShutdownState {
    Open,
    Confirming,
    Draining,
    StoppingEnvironments(ThreadId),
    ClosingStorage,
    InstallingUpdate,
    UpdateRecovery(Option<InstallLocation>),
    RelaunchingUpdate,
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

    pub fn begin_update(&mut self) -> bool {
        if *self != Self::Open {
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

    pub fn blocks_commands(&self, caller: ThreadId) -> bool {
        match self {
            Self::StoppingEnvironments(worker) => *worker != caller,
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

    pub fn install_update(&mut self) -> Result<(), String> {
        if *self != Self::ClosingStorage {
            return Err("The app must close usage storage before installing an update.".into());
        }
        *self = Self::InstallingUpdate;
        Ok(())
    }

    pub fn report_update_recovery(&mut self, location: Option<InstallLocation>) {
        if matches!(self, Self::InstallingUpdate | Self::RelaunchingUpdate) {
            *self = Self::UpdateRecovery(location);
        }
    }

    pub fn reopen_update(&mut self) -> Result<InstallLocation, String> {
        let Self::UpdateRecovery(Some(location)) = self else {
            return Err("There is no app update to reopen.".into());
        };
        let location = location.clone();
        *self = Self::RelaunchingUpdate;
        Ok(location)
    }

    pub fn allow_update_exit(&mut self) -> Result<(), String> {
        if !matches!(self, Self::InstallingUpdate | Self::RelaunchingUpdate) {
            return Err("The app update state changed before restart.".into());
        }
        *self = Self::ExitAllowed;
        Ok(())
    }

    pub fn close_after_update_failure(&mut self) -> bool {
        if !matches!(self, Self::UpdateRecovery(_)) {
            return false;
        }
        *self = Self::WindowCloseAllowed;
        true
    }

    pub fn consume_window_close(&mut self) -> bool {
        if *self != Self::WindowCloseAllowed {
            return false;
        }
        *self = Self::ExitAllowed;
        true
    }

    pub fn allows_exit(&self) -> bool {
        matches!(self, Self::WindowCloseAllowed | Self::ExitAllowed)
    }

    pub fn is_closing(&self) -> bool {
        matches!(
            self,
            Self::Draining
                | Self::StoppingEnvironments(_)
                | Self::ClosingStorage
                | Self::InstallingUpdate
                | Self::UpdateRecovery(_)
                | Self::RelaunchingUpdate
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

    #[test]
    fn an_update_blocks_new_work_until_the_approved_app_can_reopen() {
        let worker = std::thread::current().id();
        let mut state = ShutdownState::Open;
        assert!(state.begin_update());
        assert!(!state.begin_update());
        assert!(state.is_closing());
        assert!(state.install_update().is_err());
        assert!(state.allow_update_exit().is_err());

        state.stop_environments(worker).unwrap();
        assert!(state.install_update().is_err());
        state.environments_stopped(worker).unwrap();
        state.install_update().unwrap();
        assert!(!state.allows_exit());

        let location = InstallLocation::for_executable(
            "/Applications/Tugboat (previous version).app/Contents/MacOS/tugboat".into(),
        )
        .unwrap();
        state.report_update_recovery(Some(location.clone()));
        assert!(state.is_closing());
        assert!(!state.request());
        assert!(state.allow_update_exit().is_err());
        assert_eq!(state.reopen_update().unwrap(), location);
        assert!(state.reopen_update().is_err());
        state.report_update_recovery(Some(location.clone()));
        assert_eq!(state.reopen_update().unwrap(), location);
        state.allow_update_exit().unwrap();
        assert!(state.allows_exit());
    }

    #[test]
    fn the_app_can_close_after_reporting_a_failed_update() {
        let mut state = ShutdownState::InstallingUpdate;
        assert!(!state.close_after_update_failure());
        state.report_update_recovery(None);
        assert!(state.reopen_update().is_err());
        assert!(state.close_after_update_failure());
        assert!(state.consume_window_close());
    }
}
