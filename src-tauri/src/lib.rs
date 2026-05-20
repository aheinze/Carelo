pub mod commands;
pub mod fs;
pub mod open_with;
pub mod queue;
pub mod settings;
pub mod store;
pub mod window_state;

use commands::app::quit_app;
use commands::fs::{
    add_remote_volume, archive_items, cancel_file_operation, copy_items, create_folder,
    delete_items, get_file_metadata, get_home_directory, list_directory, list_open_with_apps,
    list_remote_volumes, list_volumes, measure_items_size, move_items, open_with_app,
    open_with_default_app, pause_file_operation, read_media_preview, read_text_preview,
    remove_remote_volume, rename_item, resume_file_operation, reveal_in_file_manager,
    run_custom_tool, same_volume, search_content, search_files, unarchive_items,
    FileOperationState,
};
use commands::oauth::create_oauth_tokens;
use commands::store::{
    add_favorite, app_store_path, get_app_settings, get_window_dimensions, list_favorites,
    move_favorite, remove_favorite, save_app_settings, save_window_dimensions,
};
use commands::terminal::{
    terminal_close, terminal_resize, terminal_start, terminal_write, TerminalState,
};
use fs::remote::RemoteVolumeState;
use store::AppStoreState;
use tauri::Manager;
use window_state::restore_main_window_state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(TerminalState::default())
        .manage(RemoteVolumeState::default())
        .manage(FileOperationState::default())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let store = AppStoreState::initialize()
                .map_err(|error| std::io::Error::other(error.message))?;
            app.manage(store);
            restore_main_window_state(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_directory,
            search_files,
            search_content,
            get_file_metadata,
            read_text_preview,
            read_media_preview,
            get_home_directory,
            list_volumes,
            same_volume,
            add_remote_volume,
            remove_remote_volume,
            list_remote_volumes,
            create_folder,
            rename_item,
            delete_items,
            copy_items,
            move_items,
            measure_items_size,
            archive_items,
            unarchive_items,
            cancel_file_operation,
            pause_file_operation,
            resume_file_operation,
            open_with_default_app,
            list_open_with_apps,
            open_with_app,
            run_custom_tool,
            reveal_in_file_manager,
            list_favorites,
            add_favorite,
            remove_favorite,
            move_favorite,
            app_store_path,
            get_app_settings,
            save_app_settings,
            get_window_dimensions,
            save_window_dimensions,
            create_oauth_tokens,
            terminal_start,
            terminal_write,
            terminal_resize,
            terminal_close,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Carelo desktop application");
}
