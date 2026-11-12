#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod boundaries;
mod brig;
mod codex;
mod command;
mod conversation_graph;
mod conversations;
mod data_location;
mod environment;
mod host;
mod hull;
mod installation;
mod lifecycle;
mod lineage;
mod menu;
mod operations;
mod policy_editor;
mod preparation;
mod project_policy;
mod projects;
mod pty;
mod settings;
mod shared_chatgpt;
mod storage;
mod terminal;
mod terminal_title;
mod usage;

use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

unsafe extern "C" {
    fn brig_window_set_dark_appearance(window: *mut std::ffi::c_void);
}

#[tauri::command]
fn frontend_ready(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
fn application_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
async fn choose_project_folder(window: WebviewWindow) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let selected = window
            .app_handle()
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Add new project")
            .blocking_pick_folder();
        selected
            .map(|folder| {
                folder
                    .into_path()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .transpose()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn main() {
    let context = tauri::generate_context!();
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--package-version" {
        println!("{}", context.package_info().version);
        return;
    }
    tauri::Builder::default()
        .manage(host::Inspection::default())
        .manage(projects::Projects::default())
        .manage(conversations::History::default())
        .manage(preparation::PreparationLocks::default())
        .manage(boundaries::BoundaryTests::default())
        .manage(installation::Installation::default())
        .manage(shared_chatgpt::SharedChatgpt::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            frontend_ready,
            application_version,
            choose_project_folder,
            data_location::storage_location,
            data_location::choose_storage_folder,
            data_location::change_storage_location,
            data_location::cancel_storage_move,
            data_location::restart_for_storage_change,
            projects::register_project,
            projects::recent_projects,
            projects::rescan_projects,
            projects::open_project,
            projects::forget_project,
            projects::read_project_preferences,
            projects::save_project_preferences,
            conversations::project_dashboard,
            conversations::change_conversation_state,
            conversations::conversation_usage,
            preparation::prepare_project,
            project_policy::read_project_policy,
            project_policy::change_project_policy,
            environment::preview_project,
            environment::prepare_environment,
            environment::finish_environment,
            codex::start_codex_session,
            shared_chatgpt::shared_chatgpt_status,
            shared_chatgpt::sync_project_chatgpt,
            shared_chatgpt::use_project_chatgpt,
            shared_chatgpt::retry_shared_chatgpt,
            shared_chatgpt::disconnect_shared_chatgpt,
            shared_chatgpt::cancel_shared_chatgpt_login,
            boundaries::run_boundary_tests,
            boundaries::copy_inner_boundary_prompt,
            operations::start_project_operation,
            operations::stop_project,
            operations::preview_removal,
            operations::remove_project,
            menu::update_menu_state,
            host::inspect_host,
            host::cancel_inspection,
            installation::install_brig,
            installation::cancel_installation,
            settings::load_settings,
            settings::save_settings,
            settings::telemetry_status,
            settings::load_interface_preferences,
            settings::save_interface_preferences,
            terminal::terminal_start,
            terminal::terminal_layout,
            terminal::terminal_focus,
            terminal::terminal_close,
            terminal::terminal_sessions,
            terminal::terminal_snapshot
        ])
        .setup(|app| {
            app.manage(data_location::initialize(app.handle()).map_err(std::io::Error::other)?);
            let window = WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .on_navigation(|url| {
                    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (cfg!(debug_assertions)
                            && url.scheme() == "http"
                            && url.host_str() == Some("127.0.0.1")
                            && url.port() == Some(1420))
                })
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            unsafe { brig_window_set_dark_appearance(window.ns_window()?) };
            app.set_menu(menu::create(app.handle())?)?;
            conversations::monitor(app.handle().clone());
            shared_chatgpt::monitor(app.handle().clone());
            Ok(())
        })
        .on_menu_event(menu::handle)
        .on_window_event(lifecycle::window_event)
        .build(context)
        .expect("Tugboat could not start")
        .run(lifecycle::event);
}
