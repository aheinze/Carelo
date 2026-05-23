use super::*;

#[tauri::command]
pub async fn add_remote_volume(
    config: RemoteVolumeConfig,
    remotes: tauri::State<'_, RemoteVolumeState>,
    store: tauri::State<'_, AppStoreState>,
) -> Result<RemoteVolumeInfo, FsError> {
    check_remote(&config).await?;
    let added_id = config.id.clone();
    let info = remotes.add(config.clone())?;
    remotes.set_health(
        &info.id,
        crate::fs::models::RemoteVolumeHealth {
            status: "connected".to_string(),
            message: None,
            checked_at: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            ),
        },
    )?;
    let info = remotes
        .list()?
        .into_iter()
        .find(|remote| remote.id == info.id)
        .unwrap_or(info);

    if let Err(error) = store.save_remote_volume_config(config) {
        let _ = remotes.remove(&info.id);
        return Err(error);
    }

    release_inactive_remote_after_check(&remotes, &added_id).await?;
    Ok(remotes
        .list()?
        .into_iter()
        .find(|remote| remote.id == added_id)
        .unwrap_or(info))
}

#[tauri::command]
pub async fn remove_remote_volume(
    id: String,
    remotes: tauri::State<'_, RemoteVolumeState>,
    media_state: tauri::State<'_, MediaStreamState>,
    store: tauri::State<'_, AppStoreState>,
) -> Result<bool, FsError> {
    let config = remotes.config(&id).ok();
    let remote_ids = HashSet::from([id.clone()]);
    let _ = media_state.release_remote_entries(&remote_ids);
    if let Some(config) = config {
        let _ =
            tauri::async_runtime::spawn_blocking(move || release_remote_volume_resources(&config))
                .await;
    }

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

#[tauri::command]
pub async fn check_remote_volume(
    id: String,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<RemoteVolumeInfo, FsError> {
    let info = check_registered_remote(&remotes, id.clone()).await?;
    if info.health.status == "connected" {
        release_inactive_remote_after_check(&remotes, &id).await?;
    }

    Ok(remotes
        .list()?
        .into_iter()
        .find(|remote| remote.id == id)
        .unwrap_or(info))
}

#[tauri::command]
pub async fn set_active_remote_volumes(
    ids: Vec<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
    media_state: tauri::State<'_, MediaStreamState>,
) -> Result<Vec<RemoteReleaseResult>, FsError> {
    let active_ids = ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>();
    let active_ids_for_health = active_ids.clone();
    let released_configs = remotes.set_active_ids(active_ids)?;

    for id in active_ids_for_health {
        if remotes.health_for(&id)?.status == "idle" {
            remotes.set_health(
                &id,
                crate::fs::models::RemoteVolumeHealth {
                    status: "connected".to_string(),
                    message: None,
                    checked_at: Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    ),
                },
            )?;
        }
    }

    if released_configs.is_empty() {
        return Ok(Vec::new());
    }

    let released_ids = released_configs
        .iter()
        .map(|config| config.id.clone())
        .collect::<HashSet<_>>();
    media_state.release_remote_entries(&released_ids)?;

    let results = tauri::async_runtime::spawn_blocking(move || {
        released_configs
            .into_iter()
            .map(|config| release_remote_volume_resources(&config))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| {
        FsError::new(
            "remote_release_failed",
            format!("Unable to release remote volume resources: {error}"),
            None,
        )
    })?;

    for result in &results {
        if let Some(message) = result.message.clone() {
            remotes.set_health(
                &result.id,
                crate::fs::models::RemoteVolumeHealth {
                    status: "connected".to_string(),
                    message: Some(message),
                    checked_at: Some(current_unix_seconds()),
                },
            )?;
        } else {
            remotes.mark_idle(
                &result.id,
                "No open tabs are using this remote volume.".to_string(),
            )?;
        }
    }

    Ok(results)
}

async fn release_inactive_remote_after_check(
    remotes: &RemoteVolumeState,
    id: &str,
) -> Result<(), FsError> {
    if remotes.is_active(id)? {
        return Ok(());
    }

    let config = remotes.config(id)?;
    let result =
        tauri::async_runtime::spawn_blocking(move || release_remote_volume_resources(&config))
            .await
            .map_err(|error| {
                FsError::new(
                    "remote_release_failed",
                    format!("Unable to release remote volume resources: {error}"),
                    None,
                )
            })?;

    if let Some(message) = result.message {
        remotes.set_health(
            id,
            crate::fs::models::RemoteVolumeHealth {
                status: "connected".to_string(),
                message: Some(message),
                checked_at: Some(current_unix_seconds()),
            },
        )
    } else {
        remotes.mark_idle(
            id,
            "Connection checked and released because no open tabs are using this remote volume.",
        )
    }
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
