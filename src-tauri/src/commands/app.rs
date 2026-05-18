use crate::window_state::save_main_window_dimensions;
use tauri::AppHandle;

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    save_main_window_dimensions(&app);
    app.exit(0);
}
