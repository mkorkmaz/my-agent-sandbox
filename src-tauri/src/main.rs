#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_sessions;
mod agents;
mod conversations;
mod desktop;
mod environment;
mod hash;
mod platform;
mod projects;
mod providers;
mod storage;
mod terminal;
mod usage;
mod usage_store;

use crate::agent_sessions::codex;
use crate::desktop::{lifecycle, menu, settings, startup};
use crate::environment::{boundaries, preparation};
use crate::platform::host;
use crate::projects::{operations, project_policy};
use crate::providers::{provider_login, shared_chatgpt, shared_claude};
use crate::storage::{data_location, records_watch};
use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

unsafe extern "C" {
    fn brig_window_set_dark_appearance(window: *mut std::ffi::c_void);
}

#[tauri::command]
fn frontend_ready(window: WebviewWindow) -> Result<(), String> {
    startup::ready(window)
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
    if arguments.len() == 1 && arguments[0] == "--runtime-check" {
        let status = host::runtime_check();
        let ready = status.ready();
        match serde_json::to_string_pretty(&status) {
            Ok(json) => println!("{json}"),
            Err(error) => { eprintln!("Could not encode the runtime check: {error}"); std::process::exit(1); }
        }
        if !ready { std::process::exit(1); }
        return;
    }
    tauri::Builder::default()
        .manage(startup::Startup::default())
        .manage(host::Inspection::default())
        .manage(projects::Projects::default())
        .manage(conversations::History::default())
        .manage(usage_store::Runtime::default())
        .manage(preparation::PreparationLocks::default())
        .manage(boundaries::BoundaryTests::default())
        .manage(shared_chatgpt::SharedChatgpt::default())
        .manage(shared_claude::SharedClaude::default())
        .manage(provider_login::ProviderLogins::default())
        .manage(agent_sessions::Accounts::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            frontend_ready,
            application_version,
            agents::agent_definitions,
            choose_project_folder,
            data_location::storage_location,
            data_location::choose_storage_folder,
            data_location::change_storage_location,
            data_location::cancel_storage_move,
            data_location::restart_for_storage_change,
            projects::register_project,
            projects::project_environments,
            projects::ensure_project_environment,
            projects::recent_projects,
            projects::rescan_projects,
            projects::open_project,
            projects::forget_project,
            projects::read_project_preferences,
            projects::save_project_preferences,
            conversations::saved_project_dashboard,
            conversations::recorded_project_dashboard,
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
            agent_sessions::start_agent_session,
            agent_sessions::agent_account_status,
            agent_sessions::agent_logout,
            shared_chatgpt::shared_chatgpt_status,
            shared_chatgpt::sync_project_chatgpt,
            shared_chatgpt::use_project_chatgpt,
            shared_chatgpt::retry_shared_chatgpt,
            shared_chatgpt::disconnect_shared_chatgpt,
            shared_chatgpt::cancel_shared_chatgpt_login,
            shared_claude::shared_claude_status,
            shared_claude::retry_shared_claude,
            shared_claude::disconnect_shared_claude,
            provider_login::provider_login_status,
            provider_login::start_provider_login,
            provider_login::cancel_provider_login,
            provider_login::open_provider_login,
            boundaries::run_boundary_tests,
            boundaries::copy_inner_boundary_prompt,
            operations::start_project_operation,
            operations::stop_project,
            operations::preview_removal,
            operations::remove_project,
            menu::update_menu_state,
            host::inspect_host,
            host::cancel_inspection,
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
            app.state::<startup::Startup>().records_resolved().map_err(std::io::Error::other)?;
            app.state::<usage_store::Runtime>()
                .start(app.handle().clone());
            app.state::<startup::Startup>().usage_started().map_err(std::io::Error::other)?;
            records_watch::monitor(app.handle().clone());
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
            app.state::<startup::Startup>().window_created().map_err(std::io::Error::other)?;
            unsafe { brig_window_set_dark_appearance(window.ns_window()?) };
            app.set_menu(menu::create(app.handle())?)?;
            conversations::monitor(app.handle().clone());
            shared_chatgpt::monitor(app.handle().clone());
            shared_claude::monitor(app.handle().clone());
            Ok(())
        })
        .on_menu_event(menu::handle)
        .on_window_event(lifecycle::window_event)
        .build(context)
        .expect("Tugboat could not start")
        .run(lifecycle::event);
}
