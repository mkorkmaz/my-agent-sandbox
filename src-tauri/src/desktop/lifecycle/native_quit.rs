use super::{request, ShutdownRequest, Target};
use std::sync::OnceLock;
use tauri::AppHandle;

static APP: OnceLock<AppHandle> = OnceLock::new();

// The values are the AppKit NSApplicationTerminateReply constants.
#[repr(usize)]
enum Reply {
    Cancel = 0,
    Now = 1,
    Later = 2,
}

unsafe extern "C" {
    fn tugboat_install_quit_handler(request: extern "C" fn() -> Reply) -> bool;
    fn tugboat_reply_to_quit(approved: bool);
}

pub(super) fn install(app: &AppHandle) -> Result<(), String> {
    APP.set(app.clone())
        .map_err(|_| "The macOS quit handler is already installed.")?;
    if !unsafe { tugboat_install_quit_handler(request_quit) } {
        return Err("Could not register the macOS quit handler.".into());
    }
    Ok(())
}

extern "C" fn request_quit() -> Reply {
    let Some(app) = APP.get() else {
        return Reply::Cancel;
    };
    match request(app, Target::NativeQuit) {
        ShutdownRequest::Rejected => Reply::Cancel,
        ShutdownRequest::Immediate => Reply::Now,
        ShutdownRequest::Deferred => Reply::Later,
    }
}

pub(super) fn reply(approved: bool) {
    unsafe {
        tugboat_reply_to_quit(approved);
    }
}
