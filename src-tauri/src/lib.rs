pub mod commands;
pub mod fs;
pub mod queue;
pub mod settings;
pub mod store;

use commands::app::quit_app;
use commands::fs::{
    add_remote_volume, archive_items, cancel_file_operation, copy_items, create_folder,
    delete_items, get_file_metadata, get_home_directory, list_directory, list_remote_volumes,
    list_volumes, move_items, open_with_default_app, remove_remote_volume, rename_item,
    reveal_in_file_manager, unarchive_items, FileOperationState,
};
use commands::oauth::create_oauth_tokens;
use commands::store::{
    add_favorite, app_store_path, list_favorites, move_favorite, remove_favorite,
};
use commands::terminal::{
    terminal_close, terminal_resize, terminal_start, terminal_write, TerminalState,
};
use fs::remote::RemoteVolumeState;
use store::AppStoreState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(TerminalState::default())
        .manage(RemoteVolumeState::default())
        .manage(FileOperationState::default())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let store = AppStoreState::initialize()
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.message))?;
            app.manage(store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_directory,
            get_file_metadata,
            get_home_directory,
            list_volumes,
            add_remote_volume,
            remove_remote_volume,
            list_remote_volumes,
            create_folder,
            rename_item,
            delete_items,
            copy_items,
            move_items,
            archive_items,
            unarchive_items,
            cancel_file_operation,
            open_with_default_app,
            reveal_in_file_manager,
            list_favorites,
            add_favorite,
            remove_favorite,
            move_favorite,
            app_store_path,
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
