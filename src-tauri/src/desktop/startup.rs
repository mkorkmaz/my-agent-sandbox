use std::sync::Mutex;
use tauri::{Manager, WebviewWindow};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum WindowState {
    #[default]
    Bootstrapping,
    RecordsResolved,
    UsageStarting,
    AwaitingFrontend,
    Shown,
    Focused,
}

#[derive(Default)]
pub struct Startup(Mutex<WindowState>);

impl Startup {
    pub fn records_resolved(&self) -> Result<(), String> {
        self.advance(WindowState::Bootstrapping, WindowState::RecordsResolved)
    }
    pub fn usage_started(&self) -> Result<(), String> {
        self.advance(WindowState::RecordsResolved, WindowState::UsageStarting)
    }
    pub fn window_created(&self) -> Result<(), String> {
        self.advance(WindowState::UsageStarting, WindowState::AwaitingFrontend)
    }
    fn advance(&self, expected: WindowState, next: WindowState) -> Result<(), String> {
        self.0.lock().map_err(|error| error.to_string())?.advance(expected, next)
    }
}

impl WindowState {
    fn advance(&mut self, expected: Self, next: Self) -> Result<(), String> {
        if *self != expected {
            return Err("The app startup step is out of order.".into());
        }
        *self = next;
        Ok(())
    }
    fn handoff(
        &mut self,
        show: impl FnOnce() -> Result<(), String>,
        focus: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if !matches!(self, Self::AwaitingFrontend | Self::Shown | Self::Focused) {
            return Err("The app window is not ready.".into());
        }
        if *self == Self::AwaitingFrontend {
            show()?;
            *self = Self::Shown;
        }
        focus()?;
        *self = Self::Focused;
        Ok(())
    }
}

pub fn ready(window: WebviewWindow) -> Result<(), String> {
    let startup = window.state::<Startup>();
    let mut state = startup.0.lock().map_err(|error| error.to_string())?;
    state.handoff(
        || window.show().map_err(|error| error.to_string()),
        || window.set_focus().map_err(|error| error.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_setup_rejects_skipped_and_repeated_steps() {
        let startup = Startup::default();
        assert!(startup.window_created().is_err());
        assert!(startup.usage_started().is_err());
        startup.records_resolved().unwrap();
        assert!(startup.records_resolved().is_err());
        startup.usage_started().unwrap();
        startup.window_created().unwrap();
        assert!(startup.window_created().is_err());
    }
    #[test]
    fn window_handoff_retries_only_the_failed_stage() {
        let mut state = WindowState::AwaitingFrontend;
        assert!(state
            .handoff(|| Err("show failed".into()), || panic!("focus before show"))
            .is_err());
        assert_eq!(state, WindowState::AwaitingFrontend);
        assert!(state
            .handoff(|| Ok(()), || Err("focus failed".into()))
            .is_err());
        assert_eq!(state, WindowState::Shown);
        state
            .handoff(|| panic!("window already shown"), || Ok(()))
            .unwrap();
        assert_eq!(state, WindowState::Focused);
    }
}
