pub mod commands;
pub mod fs;
pub mod open_with;
pub mod queue;
pub mod settings;
pub mod store;
pub mod window_state;

use commands::app::quit_app;
use commands::clipboard::{read_system_file_clipboard, write_system_file_clipboard};
use commands::fs::{
    DirectoryWatchState, FileOperationState, FileSearchIndexState, MediaStreamState,
    RemoteEditSyncState,
};
use commands::oauth::create_oauth_tokens;
use commands::store::{
    add_favorite, add_favorite_group, app_store_path, get_app_settings, get_window_dimensions,
    list_favorite_groups, list_favorites, move_favorite, remove_favorite, remove_favorite_group,
    save_app_settings, save_window_dimensions,
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
    let builder = tauri::Builder::default();

    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .manage(TerminalState::default())
        .manage(RemoteVolumeState::default())
        .manage(RemoteEditSyncState::default())
        .manage(FileSearchIndexState::default())
        .manage(FileOperationState::default())
        .manage(MediaStreamState::default())
        .manage(DirectoryWatchState::default())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let store = AppStoreState::initialize()
                .map_err(|error| std::io::Error::other(error.message))?;
            let remotes = app.state::<RemoteVolumeState>();

            for config in store
                .list_remote_volume_configs()
                .map_err(|error| std::io::Error::other(error.message))?
            {
                remotes
                    .add(config)
                    .map_err(|error| std::io::Error::other(error.message))?;
            }

            app.manage(store);
            restore_main_window_state(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::fs::transfer::list_directory,
            commands::fs::search::search_files,
            commands::fs::search::search_content,
            commands::fs::transfer::get_file_metadata,
            commands::fs::git::get_git_file_info,
            commands::fs::preview::compare_file_checksums,
            commands::fs::preview::read_text_preview,
            commands::fs::preview::read_media_preview,
            commands::fs::preview::create_media_stream_url,
            commands::fs::transfer::get_home_directory,
            commands::fs::volumes::list_volumes,
            commands::fs::volumes::mount_volume,
            commands::fs::volumes::unlock_volume,
            commands::fs::volumes::eject_volume,
            commands::fs::watcher::watch_active_directories,
            commands::fs::transfer::same_volume,
            commands::fs::remotes::add_remote_volume,
            commands::fs::remotes::remove_remote_volume,
            commands::fs::remotes::list_remote_volumes,
            commands::fs::remotes::check_remote_volume,
            commands::fs::remotes::set_active_remote_volumes,
            commands::fs::transfer::create_folder,
            commands::fs::transfer::rename_item,
            commands::fs::transfer::delete_items,
            commands::fs::transfer::copy_items,
            commands::fs::transfer::move_items,
            commands::fs::size::measure_items_size,
            commands::fs::archives::archive_items,
            commands::fs::archives::unarchive_items,
            commands::fs::image_tools::convert_images,
            commands::fs::pdf_tools::compress_pdfs,
            commands::fs::state::cancel_file_operation,
            commands::fs::state::pause_file_operation,
            commands::fs::state::resume_file_operation,
            commands::fs::tools::edit_file,
            commands::fs::tools::open_with_default_app,
            commands::fs::tools::list_open_with_apps,
            commands::fs::tools::open_with_app,
            commands::fs::tools::run_custom_tool,
            commands::fs::tools::reveal_in_file_manager,
            list_favorite_groups,
            add_favorite_group,
            remove_favorite_group,
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
            write_system_file_clipboard,
            read_system_file_clipboard,
            terminal_start,
            terminal_write,
            terminal_resize,
            terminal_close,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Carelo desktop application");
}
