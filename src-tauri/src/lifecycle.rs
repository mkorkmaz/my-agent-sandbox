use crate::{command, conversations, installation::Installation, terminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

static REQUESTING: AtomicBool = AtomicBool::new(false);
static CLOSING: AtomicBool = AtomicBool::new(false);
static ALLOW_CLOSE: AtomicBool = AtomicBool::new(false);
static ALLOW_EXIT: AtomicBool = AtomicBool::new(false);

const MESSAGE: &str = "An agent, terminal or installation is still running. Closing the app will end these processes and save final usage. Brig environments, guest homes and conversation history will be preserved.";

pub fn is_closing() -> bool {
    CLOSING.load(Ordering::SeqCst)
}

enum Target {
    Window(Box<Window>),
    App,
    Restart,
}

fn begin(app: AppHandle, target: Target) {
    CLOSING.store(true, Ordering::SeqCst);
    // Keep AppKit responsive while PTY readers drain output and child.wait reaps processes.
    std::thread::spawn(move || {
        let mut errors = Vec::new();
        if let Err(error) = app
            .state::<Installation>()
            .cancel_and_wait(Duration::from_secs(4))
        {
            errors.push(error);
        }
        if let Err(error) = terminal::shutdown() {
            errors.push(error);
        }
        if let Err(error) = command::wait_for_idle(Duration::from_secs(4)) {
            errors.push(error);
        }
        if let Err(error) = crate::shared_chatgpt::flush(&app) {
            errors.push(error);
        }
        if errors.is_empty() {
            // Writers have now exited; the last usage event cannot race the final scan.
            if let Err(error) = conversations::flush(&app) {
                eprintln!("Could not finish recording usage: {error}");
            }
        }
        let complete = app.clone();
        let _ = app.run_on_main_thread(move || {
            if !errors.is_empty() {
                CLOSING.store(false, Ordering::SeqCst);
                REQUESTING.store(false, Ordering::SeqCst);
                complete
                    .dialog()
                    .message(errors.join("\n"))
                    .title("Could not close the app")
                    .kind(MessageDialogKind::Error)
                    .show(|_| {});
                return;
            }
            ALLOW_CLOSE.store(true, Ordering::SeqCst);
            ALLOW_EXIT.store(true, Ordering::SeqCst);
            match target {
                Target::App => complete.exit(0),
                Target::Restart => complete.restart(),
                Target::Window(window) => {
                    if let Err(error) = window.close() {
                        ALLOW_CLOSE.store(false, Ordering::SeqCst);
                        ALLOW_EXIT.store(false, Ordering::SeqCst);
                        CLOSING.store(false, Ordering::SeqCst);
                        REQUESTING.store(false, Ordering::SeqCst);
                        eprintln!("Could not close the window: {error}");
                    }
                }
            }
        });
    });
}

fn request(app: &AppHandle, target: Target) {
    if REQUESTING.swap(true, Ordering::SeqCst) {
        return;
    }
    if !terminal::any_running() && !app.state::<Installation>().running() {
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
            REQUESTING.store(false, Ordering::SeqCst);
        }
    });
}

pub fn window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if ALLOW_CLOSE.swap(false, Ordering::SeqCst) {
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
        if ALLOW_EXIT.load(Ordering::SeqCst) {
            return;
        }
        api.prevent_exit();
        request(app, Target::App);
    }
}

#[cfg(test)]
pub(crate) fn set_closing_for_test(value: bool) {
    CLOSING.store(value, Ordering::SeqCst);
}
