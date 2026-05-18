use tauri::AppHandle;

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
