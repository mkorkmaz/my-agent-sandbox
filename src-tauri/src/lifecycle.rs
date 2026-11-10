use crate::terminal;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, RunEvent, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

static CONFIRMING: AtomicBool = AtomicBool::new(false);
static ALLOW_CLOSE: AtomicBool = AtomicBool::new(false);
static ALLOW_EXIT: AtomicBool = AtomicBool::new(false);

const MESSAGE: &str = "An agent or terminal process is still running. Closing the app will end open processes. Brig environments, guest homes and conversation history will be preserved.";

pub fn window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if !terminal::any_running() || ALLOW_CLOSE.swap(false, Ordering::SeqCst) {
        return;
    }
    api.prevent_close();
    if CONFIRMING.swap(true, Ordering::SeqCst) {
        return;
    }
    let window = window.clone();
    window
        .app_handle()
        .dialog()
        .message(MESSAGE)
        .parent(&window)
        .title("Close this window?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "End terminal processes".into(),
            "Cancel".into(),
        ))
        .show(move |confirmed| {
            CONFIRMING.store(false, Ordering::SeqCst);
            if confirmed {
                ALLOW_CLOSE.store(true, Ordering::SeqCst);
                ALLOW_EXIT.store(true, Ordering::SeqCst);
                let _ = window.close();
            }
        });
}

fn confirm_quit(app: &AppHandle) {
    if CONFIRMING.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut dialog = app
        .dialog()
        .message(MESSAGE)
        .title(format!("Quit {}?", app.package_info().name))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "End terminal processes".into(),
            "Cancel".into(),
        ));
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.parent(&window);
    }
    let app = app.clone();
    dialog.show(move |confirmed| {
        CONFIRMING.store(false, Ordering::SeqCst);
        if confirmed {
            ALLOW_EXIT.store(true, Ordering::SeqCst);
            app.exit(0);
        }
    });
}

pub fn request_quit(app: &AppHandle) {
    if terminal::any_running() {
        confirm_quit(app);
        return;
    }
    app.exit(0);
}

pub fn event(app: &AppHandle, event: RunEvent) {
    if let RunEvent::ExitRequested { api, .. } = event {
        if !terminal::any_running() || ALLOW_EXIT.load(Ordering::SeqCst) {
            return;
        }
        api.prevent_exit();
        confirm_quit(app);
        return;
    }
    if let RunEvent::Exit = event {
        terminal::shutdown();
        if let Err(error) = crate::conversations::flush(app) {
            eprintln!("Could not finish recording usage: {error}");
        }
    }
}
