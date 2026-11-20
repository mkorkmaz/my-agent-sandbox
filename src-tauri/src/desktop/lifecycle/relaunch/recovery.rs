use core_foundation::base::{CFOptionFlags, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURLRef;
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr::null;

const CAUTION_ALERT: CFOptionFlags = 2;
const GET_INSTALLER: CFOptionFlags = 1;
const REOPEN: CFOptionFlags = 2;

unsafe extern "C" {
    fn CFUserNotificationDisplayAlert(
        timeout: f64,
        flags: CFOptionFlags,
        icon: CFURLRef,
        sound: CFURLRef,
        localization: CFURLRef,
        title: CFStringRef,
        message: CFStringRef,
        default_button: CFStringRef,
        alternate_button: CFStringRef,
        other_button: CFStringRef,
        response: *mut CFOptionFlags,
    ) -> i32;
}

pub(super) fn show(bundle: &Path, executable: &Path, version: &str, error: &str, can_reopen: bool) {
    let message = format!(
        "Tugboat {version} is installed at {}.\n\n{error}\n\nIf Tugboat still cannot start, get the signed installer. Keep your records folder.",
        bundle.display()
    );
    eprintln!("{message}");
    let mut action = Command::new("/usr/bin/open");
    match recovery_choice(&message, can_reopen) {
        GET_INSTALLER => action.arg(crate::desktop::update::INSTALLER),
        // Launch Services opens the exact bundle selected by the updater.
        REOPEN
            if bundle
                .extension()
                .is_some_and(|extension| extension == "app") =>
        {
            action.args(["-a"]).arg(bundle)
        }
        // Tauri may preserve a previous bundle in a directory with no .app extension.
        REOPEN => {
            if let Err(error) = Command::new(executable)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
            {
                eprintln!("Could not reopen Tugboat {version}. {error}");
            }
            return;
        }
        _ => return,
    };
    if let Err(error) = action.spawn() {
        eprintln!("Could not open the selected recovery option. {error}");
    }
}

fn recovery_choice(message: &str, can_reopen: bool) -> CFOptionFlags {
    let title = CFString::new("Could not restart Tugboat");
    let message = CFString::new(message);
    let close = CFString::new("Close");
    let installer = CFString::new("Get Signed Installer");
    let reopen = can_reopen.then(|| CFString::new("Reopen Tugboat"));
    let mut response = 0;
    // The macOS alert is independent of Tauri and closes after five minutes.
    let status = unsafe {
        CFUserNotificationDisplayAlert(
            300.0,
            CAUTION_ALERT,
            null(),
            null(),
            null(),
            title.as_concrete_TypeRef(),
            message.as_concrete_TypeRef(),
            close.as_concrete_TypeRef(),
            installer.as_concrete_TypeRef(),
            reopen.as_ref().map_or(null(), TCFType::as_concrete_TypeRef),
            &mut response,
        )
    };
    if status != 0 {
        eprintln!("Could not display the Tugboat recovery alert. macOS error {status}.");
        return 0;
    }
    response & 3
}
