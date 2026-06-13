use crate::fs::models::{FsError, FsResult};
use crate::store::{
    window_dimensions as build_window_dimensions, AppStoreState, FavoriteEntry, FavoriteGroupEntry,
    FavoriteInput, WindowDimensions,
};
use serde_json::Value;

#[tauri::command]
pub fn list_favorites(store: tauri::State<'_, AppStoreState>) -> FsResult<Vec<FavoriteEntry>> {
    store.list_favorites()
}

#[tauri::command]
pub fn list_favorite_groups(
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<Vec<FavoriteGroupEntry>> {
    store.list_favorite_groups()
}

#[tauri::command]
pub fn add_favorite_group(
    name: String,
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<FavoriteGroupEntry> {
    store.add_favorite_group(name)
}

#[tauri::command]
pub fn remove_favorite_group(id: String, store: tauri::State<'_, AppStoreState>) -> FsResult<()> {
    store.remove_favorite_group(id)
}

#[tauri::command]
pub fn add_favorite(
    favorite: FavoriteInput,
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<FavoriteEntry> {
    store.add_favorite(favorite)
}

#[tauri::command]
pub fn remove_favorite(id: String, store: tauri::State<'_, AppStoreState>) -> FsResult<()> {
    store.remove_favorite(id)
}

#[tauri::command]
pub fn move_favorite(
    id: String,
    target_group_id: Option<String>,
    target_index: i64,
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<Vec<FavoriteEntry>> {
    store.move_favorite(id, target_group_id, target_index)
}

#[tauri::command]
pub fn app_store_path(store: tauri::State<'_, AppStoreState>) -> Result<String, FsError> {
    Ok(store.path().to_string_lossy().into_owned())
}

#[tauri::command]
pub fn get_window_dimensions(
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<Option<WindowDimensions>> {
    store.window_dimensions()
}

#[tauri::command]
pub fn save_window_dimensions(
    width: f64,
    height: f64,
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<()> {
    store.save_window_dimensions(build_window_dimensions(width, height))
}

#[tauri::command]
pub fn get_app_settings(store: tauri::State<'_, AppStoreState>) -> FsResult<Option<Value>> {
    store.app_settings()
}

#[tauri::command]
pub fn save_app_settings(settings: Value, store: tauri::State<'_, AppStoreState>) -> FsResult<()> {
    store.save_app_settings(settings)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagMove {
    pub from: String,
    pub to: String,
}

#[tauri::command]
pub fn set_file_tags(
    paths: Vec<String>,
    color: Option<String>,
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<()> {
    for path in paths {
        store.set_file_tag(path, color.clone())?;
    }

    Ok(())
}

#[tauri::command]
pub fn move_file_tags(moves: Vec<TagMove>, store: tauri::State<'_, AppStoreState>) -> FsResult<()> {
    for entry in moves {
        store.move_file_tag(&entry.from, &entry.to)?;
    }

    Ok(())
}
