use tauri::menu::{Menu, MenuItem, Submenu};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

mod build;
mod chat;
mod config;
mod secrets;

/// The settings window, opened over the main one. One per app: a second
/// Cmd+, focuses the window that is already there.
fn open_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(settings) = app.get_webview_window("settings") {
        return settings.set_focus();
    }
    let mut builder = WebviewWindowBuilder::new(
        app,
        "settings",
        WebviewUrl::App("index.html?window=settings".into()),
    )
    .title("Settings")
    // Two tabs, each a secret row over the fields that depend on it;
    // resizable, with a floor the tabs and their controls still fit.
    .inner_size(620.0, 420.0)
    .min_inner_size(520.0, 360.0)
    .resizable(true)
    .minimizable(false);
    // Parented, so it floats above the main window and travels with it —
    // the native reading of "modal" this app can offer on every platform.
    if let Some(main) = app.get_webview_window("main") {
        builder = builder.parent(&main)?;
    }
    builder.center().build().map(|_| ())
}

/// What Cmd+, in the main window asks for. The window is the backend's to
/// make, so the frontend asks rather than acts.
#[tauri::command]
async fn settings_open(app: AppHandle) -> Result<(), String> {
    open_settings(&app).map_err(|error| error.to_string())
}

/// What Ok and Esc in the settings window ask for.
#[tauri::command]
async fn settings_close(app: AppHandle) -> Result<(), String> {
    match app.get_webview_window("settings") {
        Some(settings) => settings.close().map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

/// Dresses every window — webview and native chrome alike — in `theme`,
/// "light" or "dark". App-wide, so both windows always agree.
#[tauri::command]
async fn set_theme(app: AppHandle, theme: String) -> Result<(), String> {
    let theme = match theme.as_str() {
        "dark" => tauri::Theme::Dark,
        _ => tauri::Theme::Light,
    };
    app.set_theme(Some(theme));
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // First act: converge on ~/.epik/config.toml. A file that
            // cannot be read is left exactly as it is; the reason goes to
            // stderr and the built-in defaults apply.
            let config = epik::config::converge().unwrap_or_else(|error| {
                eprintln!("{error:#}; starting with the built-in defaults");
                epik::config::Config::default()
            });
            // Managed behind a lock: the settings window replaces it.
            app.manage(std::sync::Mutex::new(config));
            // Follow the system at startup. When persistence arrives, a
            // remembered choice will override this initialization here.
            app.handle().set_theme(None);
            Ok(())
        })
        .manage(chat::ChatState::default())
        .manage(build::BuildState::default())
        .manage(build::FeatureState::default())
        .menu(|handle| {
            let menu = Menu::default(handle)?;
            let settings =
                MenuItem::with_id(handle, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
            let file = menu.items()?.into_iter().find_map(|item| {
                item.as_submenu()
                    .filter(|submenu| submenu.text().is_ok_and(|text| text == "File"))
                    .cloned()
            });
            match file {
                Some(file) => file.prepend(&settings)?,
                // A platform whose default menu has no File: give it one.
                None => menu.append(&Submenu::with_items(handle, "File", true, &[&settings])?)?,
            }
            Ok(menu)
        })
        .on_menu_event(|app, event| {
            if event.id() == "settings" {
                let _ = open_settings(app);
            }
        })
        .invoke_handler(tauri::generate_handler![
            chat::send_message,
            chat::get_transcript,
            chat::answer_question,
            secrets::secret_reveal,
            secrets::secret_save,
            config::config_read,
            config::config_write,
            config::models_list,
            config::github_login,
            set_theme,
            settings_open,
            settings_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
