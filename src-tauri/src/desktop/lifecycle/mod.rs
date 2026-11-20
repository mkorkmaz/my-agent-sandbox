use crate::{conversations, platform::command, projects::operations::shutdown, terminal};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

mod install_location;
mod native_quit;
mod relaunch;
mod state;
mod update;
pub(crate) use relaunch::{run_relaunch_supervisor, RelaunchConfirmation};
use state::ShutdownState;
pub(crate) use update::{
    begin_update, check_update_location, reopen_after_update, InstallFailure, UpdateFailure,
};
static SHUTDOWN: Mutex<ShutdownState> = Mutex::new(ShutdownState::Open);

const MESSAGE: &str = "An agent or terminal is still running. Tugboat will end these processes, save final usage, and stop its project environments. Your project files and saved chats will stay available.";

pub fn is_closing() -> bool {
    SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_closing()
}

pub(crate) fn blocks_commands() -> bool {
    SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .blocks_commands(std::thread::current().id())
}

pub(crate) fn stop_before_exit(stop: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let worker = std::thread::current().id();
    SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .stop_environments(worker)?;
    stop()?;
    SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .environments_stopped(worker)
}

enum Target {
    Window(Box<Window>),
    App,
    NativeQuit,
    Restart,
}

pub(crate) fn install_native_quit(app: &AppHandle) -> Result<(), String> {
    native_quit::install(app)
}

fn begin(app: AppHandle, target: Target) {
    if !SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .begin()
    {
        return;
    }
    // Keep AppKit responsive while PTY readers drain output and child.wait reaps processes.
    std::thread::spawn(move || {
        let result = drain(&app);
        let complete = app.clone();
        let native_request = matches!(target, Target::NativeQuit);
        let dispatched = app.run_on_main_thread(move || {
            let result = result
                .and_then(|()| close_storage(&complete))
                .and_then(|()| {
                    SHUTDOWN
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .approve()
                });
            if let Err(error) = result {
                SHUTDOWN
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .report_failure();
                if native_request {
                    native_quit::reply(false);
                }
                let mut alert = complete
                    .dialog()
                    .message(error)
                    .title("Could not close the app")
                    .kind(MessageDialogKind::Error);
                if let Some(window) = complete.get_webview_window("main") {
                    alert = alert.parent(&window);
                }
                alert.show(|_| {
                    SHUTDOWN
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .abort();
                });
                return;
            }
            finish(&complete, target);
        });
        if let Err(error) = dispatched {
            SHUTDOWN
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .abort();
            if native_request {
                native_quit::reply(false);
            }
            eprintln!("Could not dispatch app shutdown. Retry closing the app. {error}");
        }
    });
}

fn drain(app: &AppHandle) -> Result<(), String> {
    crate::providers::provider_login::cancel_all(app);
    let mut errors = Vec::new();
    let mut collect = |step: &str, result: Result<(), String>| {
        if let Err(error) = result {
            errors.push(format!("{step} {error}"));
        }
    };
    collect("Could not end terminal processes.", terminal::shutdown());
    collect(
        "Could not finish background commands.",
        command::wait_for_idle(Duration::from_secs(4)),
    );
    collect(
        "Could not finish environment operations.",
        crate::environment::EnvironmentOperation::wait_for_idle(Duration::from_secs(4)),
    );
    if errors.is_empty() {
        if let Err(error) = crate::storage::data_location::reconnect_for_shutdown(app) {
            errors.push(format!("Could not reconnect saved records. {error}"));
        }
    }
    for (step, result) in [
        (
            "Could not save provider sign-in.",
            crate::providers::provider_login::flush(app),
        ),
        (
            "Could not save the ChatGPT account.",
            crate::providers::shared_chatgpt::flush(app),
        ),
        (
            "Could not save the Claude account.",
            crate::providers::shared_claude::flush(app),
        ),
    ] {
        if let Err(error) = result {
            errors.push(format!("{step} {error}"));
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    // Writers have exited; the last usage event cannot race the final scan.
    flush_and_stop(
        || conversations::flush(app),
        || {
            stop_before_exit(|| shutdown::stop_environments(app))
                .map_err(|error| format!("Could not stop and verify project guests. {error}"))
        },
    )
}

fn close_storage(app: &AppHandle) -> Result<(), String> {
    app.state::<crate::usage_store::Runtime>()
        .close()
        .map_err(|error| format!("Could not close usage storage. {error}"))
}

fn finish(app: &AppHandle, target: Target) {
    match target {
        Target::App => app.exit(0),
        Target::NativeQuit => native_quit::reply(true),
        Target::Restart => app.restart(),
        Target::Window(window) => {
            if let Err(error) = window.close() {
                SHUTDOWN
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .abort();
                eprintln!("Could not close the window: {error}");
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ShutdownRequest {
    Rejected,
    Immediate,
    Deferred,
}

fn admit_request(state: &mut ShutdownState, target: &Target) -> ShutdownRequest {
    if matches!(target, Target::NativeQuit) && state.allows_exit() {
        return ShutdownRequest::Immediate;
    }
    if !matches!(target, Target::Restart) && state.close_after_update_failure() {
        return ShutdownRequest::Immediate;
    }
    if state.request() {
        return ShutdownRequest::Deferred;
    }
    ShutdownRequest::Rejected
}

fn request(app: &AppHandle, target: Target) -> ShutdownRequest {
    let decision = admit_request(
        &mut SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()),
        &target,
    );
    match decision {
        ShutdownRequest::Immediate => {
            if !matches!(target, Target::NativeQuit) {
                finish(app, target);
            }
            return decision;
        }
        ShutdownRequest::Rejected => return decision,
        ShutdownRequest::Deferred => {}
    }
    if !terminal::any_running() {
        begin(app.clone(), target);
        return decision;
    }
    let mut dialog = app
        .dialog()
        .message(MESSAGE)
        .title(match &target {
            Target::Window(_) => "Close this window?".into(),
            Target::App | Target::NativeQuit => format!("Quit {}?", app.package_info().name),
            Target::Restart => "Restart Tugboat?".into(),
        })
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            match &target {
                Target::Restart => "End processes and restart".into(),
                _ => "End processes and close".into(),
            },
            "Cancel".into(),
        ));
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.parent(&window);
    }
    let app = app.clone();
    dialog.show(move |confirmed| {
        if confirmed {
            begin(app, target);
        } else {
            SHUTDOWN
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .abort();
            if matches!(target, Target::NativeQuit) {
                native_quit::reply(false);
            }
        }
    });
    decision
}

pub fn window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .consume_window_close()
    {
        return;
    }
    api.prevent_close();
    request(
        window.app_handle(),
        Target::Window(Box::new(window.clone())),
    );
}

pub fn request_quit(app: &AppHandle) {
    request(app, Target::App);
}

pub fn request_restart(app: &AppHandle) {
    request(app, Target::Restart);
}

pub fn event(app: &AppHandle, event: RunEvent) {
    if let RunEvent::ExitRequested { api, .. } = event {
        if SHUTDOWN
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .allows_exit()
        {
            return;
        }
        api.prevent_exit();
        request(app, Target::App);
    }
}

#[cfg(test)]
pub(crate) fn set_closing_for_test(value: bool) {
    *SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()) = match value {
        true => ShutdownState::Draining,
        false => ShutdownState::Open,
    };
}

fn flush_and_stop(
    flush: impl FnOnce() -> Result<(), String>,
    stop: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    flush().map_err(|error| format!("Could not finish recording usage: {error}"))?;
    stop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_quit_can_exit_update_recovery_without_a_second_drain() {
        let mut state = ShutdownState::InstallingUpdate;
        assert_eq!(
            admit_request(&mut state, &Target::NativeQuit),
            ShutdownRequest::Rejected
        );
        state.report_update_recovery(None);
        assert_eq!(
            admit_request(&mut state, &Target::NativeQuit),
            ShutdownRequest::Immediate
        );
        assert!(state.allows_exit());
        assert_eq!(
            admit_request(&mut ShutdownState::Open, &Target::NativeQuit),
            ShutdownRequest::Deferred
        );
    }

    #[test]
    fn final_scan_failure_prevents_environment_shutdown() {
        let mut stopped = false;
        let result = flush_and_stop(
            || Err("fixture disk is full".into()),
            || {
                stopped = true;
                Ok(())
            },
        );
        assert_eq!(
            result.unwrap_err(),
            "Could not finish recording usage: fixture disk is full"
        );
        assert!(!stopped);
    }

    #[test]
    fn a_stop_failure_after_the_final_scan_reaches_the_close_dialog() {
        let flushed = std::cell::Cell::new(false);
        let result = flush_and_stop(
            || {
                flushed.set(true);
                Ok(())
            },
            || {
                assert!(flushed.get());
                Err("fixture guest still running".into())
            },
        );
        assert_eq!(result.unwrap_err(), "fixture guest still running");
    }
}
