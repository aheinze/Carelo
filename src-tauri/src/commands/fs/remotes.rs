use super::*;

#[tauri::command]
pub async fn add_remote_volume(
    config: RemoteVolumeConfig,
    remotes: tauri::State<'_, RemoteVolumeState>,
    store: tauri::State<'_, AppStoreState>,
) -> Result<RemoteVolumeInfo, FsError> {
    check_remote(&config).await?;
    let info = remotes.add(config.clone())?;

    if let Err(error) = store.save_remote_volume_config(config) {
        let _ = remotes.remove(&info.id);
        return Err(error);
    }

    Ok(info)
}

#[tauri::command]
pub async fn remove_remote_volume(
    id: String,
    remotes: tauri::State<'_, RemoteVolumeState>,
    store: tauri::State<'_, AppStoreState>,
) -> Result<bool, FsError> {
    let removed_live = remotes.remove(&id)?;
    let removed_saved = store.remove_remote_volume_config(&id)?;
    Ok(removed_live || removed_saved)
}

#[tauri::command]
pub async fn list_remote_volumes(
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<RemoteVolumeInfo>, FsError> {
    remotes.list()
}
