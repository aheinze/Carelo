use crate::fs::models::{FsError, FsResult};
use crate::store::{AppStoreState, FavoriteEntry, FavoriteInput};

#[tauri::command]
pub fn list_favorites(store: tauri::State<'_, AppStoreState>) -> FsResult<Vec<FavoriteEntry>> {
    store.list_favorites()
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
    target_index: i64,
    store: tauri::State<'_, AppStoreState>,
) -> FsResult<Vec<FavoriteEntry>> {
    store.move_favorite(id, target_index)
}

#[tauri::command]
pub fn app_store_path(store: tauri::State<'_, AppStoreState>) -> Result<String, FsError> {
    Ok(store.path().to_string_lossy().into_owned())
}
