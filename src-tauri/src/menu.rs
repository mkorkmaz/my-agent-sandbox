use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager};

pub fn create(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let package = app.package_info();
    let name = &package.name;
    let application = Submenu::with_items(
        app,
        name,
        true,
        &[
            &PredefinedMenuItem::about(
                app,
                Some(&format!("About {name}")),
                Some(AboutMetadata {
                    name: Some(name.clone()),
                    version: Some(package.version.to_string()),
                    credits: Some(package.description.to_owned()),
                    ..Default::default()
                }),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("Services"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(&format!("Hide {name}")))?,
            &PredefinedMenuItem::hide_others(app, Some("Hide Others"))?,
            &PredefinedMenuItem::show_all(app, Some("Show All"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                "quit",
                format!("Quit {name}"),
                true,
                Some("CmdOrCtrl+Q"),
            )?,
        ],
    )?;
    let file = Submenu::with_id_and_items(
        app,
        "file",
        "File",
        true,
        &[
            &MenuItem::with_id(
                app,
                "choose-folder",
                "Add new project",
                true,
                Some("CmdOrCtrl+O"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("Close Window"))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("Undo"))?,
            &PredefinedMenuItem::redo(app, Some("Redo"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("Cut"))?,
            &PredefinedMenuItem::copy(app, Some("Copy"))?,
            &PredefinedMenuItem::paste(app, Some("Paste"))?,
            &PredefinedMenuItem::select_all(app, Some("Select All"))?,
        ],
    )?;
    let view = Submenu::with_id_and_items(
        app,
        "view",
        "View",
        true,
        &[
            &MenuItem::with_id(
                app,
                "project",
                "Return to project",
                true,
                Some("CmdOrCtrl+1"),
            )?,
            &MenuItem::with_id(
                app,
                "return-terminal",
                "Return to terminal",
                false,
                Some("CmdOrCtrl+2"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, Some("Enter Full Screen"))?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("Minimize"))?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &PredefinedMenuItem::bring_all_to_front(app, Some("Bring All to Front"))?,
        ],
    )?;
    window.set_as_windows_menu_for_nsapp()?;
    Menu::with_items(app, &[&application, &file, &edit, &view, &window])
}

pub fn handle(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    if id == "quit" {
        crate::lifecycle::request_quit(app);
        return;
    }
    if ["choose-folder", "settings", "project", "return-terminal"].contains(&id) {
        if let Some(window) = app.get_webview_window("main") {
            if let Err(error) = window.emit("menu-action", id) {
                eprintln!("Native menu action could not be delivered: {error}");
            }
        }
    }
}

#[tauri::command]
pub fn update_menu_state(
    app: AppHandle,
    project_id: Option<String>,
    busy: bool,
) -> Result<(), String> {
    let menu = app.menu().ok_or("Application menu not found.")?;
    for (parent, id, enabled) in [
        ("file", "choose-folder", !busy && project_id.is_none()),
        (
            "view",
            "return-terminal",
            project_id
                .as_deref()
                .and_then(crate::terminal::status)
                .is_some(),
        ),
    ] {
        let Some(tauri::menu::MenuItemKind::Submenu(submenu)) = menu.get(parent) else {
            return Err("Native submenu not found.".into());
        };
        let Some(tauri::menu::MenuItemKind::MenuItem(item)) = submenu.get(id) else {
            return Err("Native menu action not found.".into());
        };
        item.set_enabled(enabled)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
