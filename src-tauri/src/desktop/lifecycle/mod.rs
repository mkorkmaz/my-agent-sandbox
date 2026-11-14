use crate::{conversations, platform::command, terminal};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

mod state;
use state::ShutdownState;
static SHUTDOWN: Mutex<ShutdownState> = Mutex::new(ShutdownState::Open);

const MESSAGE: &str = "An agent or terminal is still running. Closing the app will end these processes and save final usage. Brig environments, guest homes and conversation history will be preserved.";

pub fn is_closing() -> bool {
    SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).is_closing()
}

enum Target {
    Window(Box<Window>),
    App,
    Restart,
}

fn begin(app: AppHandle, target: Target) {
    if !SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).begin() { return; }
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
            if let Err(error) = finish_usage(
                || conversations::flush(&app),
                || Ok(()),
            ) {
                errors.push(error);
            }
        }
        let complete = app.clone();
        let dispatched = app.run_on_main_thread(move || {
            if errors.is_empty() {
                if let Err(error) = finish_usage(
                    || Ok(()),
                    || complete.state::<crate::usage_store::Runtime>().close(),
                ) {
                    errors.push(error);
                }
            }
            if errors.is_empty() {
                if let Err(error) = SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).approve() {
                    errors.push(error);
                }
            }
            if !errors.is_empty() {
                SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).abort();
                complete
                    .dialog()
                    .message(errors.join("\n"))
                    .title("Could not close the app")
                    .kind(MessageDialogKind::Error)
                    .show(|_| {});
                return;
            }
            match target {
                Target::App => complete.exit(0),
                Target::Restart => complete.restart(),
                Target::Window(window) => {
                    if let Err(error) = window.close() {
                        SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).abort();
                        eprintln!("Could not close the window: {error}");
                    }
                }
            }
        });
        if let Err(error) = dispatched {
            SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).abort();
            eprintln!("Could not dispatch app shutdown. Retry closing the app. {error}");
        }
    });
}

fn request(app: &AppHandle, target: Target) {
    if !SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).request() {
        return;
    }
    if !terminal::any_running() {
        begin(app.clone(), target);
        return;
    }
    let mut dialog = app
        .dialog()
        .message(MESSAGE)
        .title(match &target {
            Target::Window(_) => "Close this window?".into(),
            Target::App => format!("Quit {}?", app.package_info().name),
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
            SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).abort();
        }
    });
}

pub fn window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).consume_window_close() {
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
        if SHUTDOWN.lock().unwrap_or_else(|error| error.into_inner()).allows_exit() {
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

fn finish_usage(
    flush: impl FnOnce() -> Result<(), String>,
    close: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    flush().map_err(|error| format!("Could not finish recording usage: {error}"))?;
    close().map_err(|error| format!("Could not close usage storage: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_scan_failure_prevents_database_close_and_restart_approval() {
        let mut closed = false;
        let result = finish_usage(
            || Err("fixture disk is full".into()),
            || {
                closed = true;
                Ok(())
            },
        );
        assert_eq!(
            result.unwrap_err(),
            "Could not finish recording usage: fixture disk is full"
        );
        assert!(!closed);
    }

    #[test]
    fn close_failure_cannot_be_reported_as_a_successful_final_flush() {
        let result = finish_usage(|| Ok(()), || Err("fixture sync failed".into()));
        assert_eq!(
            result.unwrap_err(),
            "Could not close usage storage: fixture sync failed"
        );
    }
}
