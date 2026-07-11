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
    store: tauri::State<'_, AppStoreState>,
) -> Result<RemoteVolumeInfo, FsError> {
    // A desktop credential store can be locked while Carelo starts. Reload the
    // persisted config before an explicit/periodic health check so credentials
    // become usable as soon as the user unlocks the keyring, without requiring
    // an application restart. A still-locked keyring simply yields the same
    // safe, credential-free config that was loaded during startup.
    if let Some(mut config) = store
        .list_remote_volume_configs()?
        .into_iter()
        .find(|config| config.id == id)
    {
        // Do not discard credentials that are already live merely because the
        // keyring became temporarily unavailable after startup. A later
        // successful load replaces these values normally.
        if let Ok(current) = remotes.config(&id) {
            retain_live_remote_options(&mut config, current);
        }

        remotes.add(config)?;
    }

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

fn retain_live_remote_options(config: &mut RemoteVolumeConfig, current: RemoteVolumeConfig) {
    for (key, value) in current.options {
        config.options.entry(key).or_insert(value);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_with_options(options: &[(&str, &str)]) -> RemoteVolumeConfig {
        RemoteVolumeConfig {
            id: "remote".to_string(),
            name: "Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: options
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    #[test]
    fn credential_refresh_retains_options_missing_from_a_locked_keyring() {
        let mut persisted = remote_with_options(&[("endpoint", "https://dav.test")]);
        let current = remote_with_options(&[("password", "already-loaded")]);

        retain_live_remote_options(&mut persisted, current);

        assert_eq!(
            persisted.options.get("password").map(String::as_str),
            Some("already-loaded")
        );
    }

    #[test]
    fn credential_refresh_prefers_newly_loaded_values() {
        let mut persisted = remote_with_options(&[("password", "newly-loaded")]);
        let current = remote_with_options(&[("password", "stale")]);

        retain_live_remote_options(&mut persisted, current);

        assert_eq!(
            persisted.options.get("password").map(String::as_str),
            Some("newly-loaded")
        );
    }
}
