use crate::{conversations, platform::command, projects::operations::shutdown, terminal};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

mod native_quit;
mod state;
use state::ShutdownState;
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
    crate::providers::provider_login::cancel_all(&app);
    // Keep AppKit responsive while PTY readers drain output and child.wait reaps processes.
    std::thread::spawn(move || {
        let mut errors = Vec::new();
        if let Err(error) = terminal::shutdown() {
            errors.push(error);
        }
        if let Err(error) = command::wait_for_idle(Duration::from_secs(4)) {
            errors.push(error);
        }
        if let Err(error) =
            crate::environment::EnvironmentOperation::wait_for_idle(Duration::from_secs(4))
        {
            errors.push(error);
        }
        if errors.is_empty() {
            if let Err(error) = crate::storage::data_location::reconnect_for_shutdown(&app) {
                errors.push(error);
            }
        }
        if let Err(error) = crate::providers::provider_login::flush(&app) {
            errors.push(error);
        }
        if let Err(error) = crate::providers::shared_chatgpt::flush(&app) {
            errors.push(error);
        }
        if let Err(error) = crate::providers::shared_claude::flush(&app) {
            errors.push(error);
        }
        if errors.is_empty() {
            // Writers have exited; the last usage event cannot race the final scan.
            if let Err(error) = flush_and_stop(
                || conversations::flush(&app),
                || stop_before_exit(|| shutdown::stop_environments(&app)),
            ) {
                errors.push(error);
            }
        }
        let complete = app.clone();
        let native_request = matches!(target, Target::NativeQuit);
        let dispatched = app.run_on_main_thread(move || {
            if errors.is_empty() {
                if let Err(error) = complete.state::<crate::usage_store::Runtime>().close() {
                    errors.push(format!("Could not close usage storage: {error}"));
                }
            }
            if errors.is_empty() {
                if let Err(error) = SHUTDOWN
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .approve()
                {
                    errors.push(error);
                }
            }
            if !errors.is_empty() {
                SHUTDOWN
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .report_failure();
                if native_request {
                    native_quit::reply(false);
                }
                let mut alert = complete
                    .dialog()
                    .message(errors.join("\n"))
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
            match target {
                Target::App => complete.exit(0),
                Target::NativeQuit => native_quit::reply(true),
                Target::Restart => complete.restart(),
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

fn request(app: &AppHandle, target: Target) -> bool {
    if !SHUTDOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .request()
    {
        return false;
    }
    if !terminal::any_running() {
        begin(app.clone(), target);
        return true;
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
    true
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
