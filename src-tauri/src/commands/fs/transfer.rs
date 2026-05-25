use super::*;

#[tauri::command]
pub async fn list_directory(
    path: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<FileEntry>, FsError> {
    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        return run_local(move |_| archive::list_archive_directory(&archive_path)).await;
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        return list_remote_directory(&remotes, remote_path).await;
    }

    let sudo_path = path.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| provider.list(&path),
        move |password| sudo::list_directory(&password, &sudo_path),
    )
    .await
}

#[tauri::command]
pub async fn get_home_directory() -> Result<String, FsError> {
    LocalFileProvider::home_dir().map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn same_volume(paths: Vec<String>, target_directory: String) -> Result<bool, FsError> {
    let Some(first_path) = paths.first() else {
        return Ok(true);
    };

    if archive::is_archive_uri(&target_directory)
        || paths.iter().any(|path| archive::is_archive_uri(path))
    {
        return Ok(false);
    }

    if let Some(remote_target) = parse_remote_path(&target_directory) {
        return Ok(paths.iter().all(|path| {
            parse_remote_path(path)
                .map(|remote_path| remote_path.volume_id == remote_target.volume_id)
                .unwrap_or(false)
        }));
    }

    if parse_remote_path(first_path).is_some()
        || paths.iter().any(|path| parse_remote_path(path).is_some())
    {
        return Ok(false);
    }

    tauri::async_runtime::spawn_blocking(move || {
        let target = expand_local_path(&target_directory)?;
        let target_volume = local_volume_identity(&target)?;

        for path in paths {
            let source = expand_local_path(&path)?;

            if local_volume_identity(&source)? != target_volume {
                return Ok(false);
            }
        }

        Ok(true)
    })
    .await
    .map_err(|error| {
        FsError::new(
            "task_join_error",
            format!("Volume comparison failed: {error}"),
            None,
        )
    })?
}

#[tauri::command]
pub async fn get_file_metadata(
    path: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<FileMetadata, FsError> {
    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        return run_local(move |_| archive::stat_archive_entry(&archive_path)).await;
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        return stat_remote_item(&remotes, remote_path).await;
    }

    let sudo_path = path.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| provider.stat(&path),
        move |password| sudo::get_file_metadata(&password, &sudo_path),
    )
    .await
}

#[tauri::command]
pub async fn create_folder(
    path: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    if archive::is_archive_uri(&path) {
        return Err(archive_read_only_error(&path));
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        return create_remote_folder(&remotes, remote_path).await;
    }

    let sudo_path = path.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| provider.create_dir(&path),
        move |password| sudo::create_folder(&password, &sudo_path),
    )
    .await
}

#[tauri::command]
pub async fn rename_item(
    from: String,
    to: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    if archive::is_archive_uri(&from) || archive::is_archive_uri(&to) {
        return Err(archive_read_only_error(if archive::is_archive_uri(&from) {
            &from
        } else {
            &to
        }));
    }

    match (parse_remote_path(&from), parse_remote_path(&to)) {
        (Some(remote_from), Some(remote_to)) => {
            return if remote_from.volume_id == remote_to.volume_id {
                rename_remote_item(&remotes, remote_from, remote_to).await
            } else {
                move_remote_item(&remotes, remote_from, remote_to, false).await
            };
        }
        (Some(remote_from), None) => {
            let target = expand_local_path(&to)?;
            return move_remote_to_local_item(&remotes, remote_from, &target, false).await;
        }
        (None, Some(remote_to)) => {
            let source = expand_local_path(&from)?;
            return move_local_to_remote_item(
                &remotes,
                &source,
                remote_to,
                false,
                operations::SymlinkMode::Preserve,
            )
            .await;
        }
        (None, None) => {}
    }

    let sudo_from = from.clone();
    let sudo_to = to.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| provider.rename(&from, &to),
        move |password| sudo::rename_item(&password, &sudo_from, &sudo_to),
    )
    .await
}

#[tauri::command]
pub async fn delete_items(
    paths: Vec<String>,
    delete_mode: Option<DeleteMode>,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let delete_mode = delete_mode.unwrap_or_default();
    let mut local_paths = Vec::new();
    let mut remote_paths = Vec::new();

    for path in paths {
        if archive::is_archive_uri(&path) {
            return Err(archive_read_only_error(&path));
        } else if let Some(remote_path) = parse_remote_path(&path) {
            remote_paths.push((path, remote_path));
        } else {
            local_paths.push(path);
        }
    }

    for (_, remote_path) in remote_paths {
        delete_remote_item(&remotes, remote_path).await?;
    }

    if local_paths.is_empty() {
        return Ok(());
    }

    if delete_mode == DeleteMode::Trash {
        return run_local(move |_| move_local_paths_to_trash(local_paths)).await;
    }

    let sudo_paths = local_paths.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| {
            for path in local_paths {
                provider.delete(&path)?;
            }

            Ok(())
        },
        move |password| {
            for path in &sudo_paths {
                sudo::delete_item(&password, path)?;
            }

            Ok(())
        },
    )
    .await
}

fn move_local_paths_to_trash(paths: Vec<String>) -> FsResult<()> {
    for path in paths {
        let path = expand_local_path(&path)?;

        if path.as_os_str().is_empty() || path == Path::new("/") {
            return Err(FsError::new(
                "unsafe_delete_target",
                "Refusing to move the root directory to Trash.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        trash::delete(&path).map_err(|error| {
            FsError::new(
                "trash_delete_failed",
                format!("Unable to move item to Trash: {error}"),
                Some(path.to_string_lossy().into_owned()),
            )
        })?;
    }

    Ok(())
}

#[tauri::command]
pub async fn copy_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    items: Vec<TransferItem>,
    job_id: Option<String>,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let mut archive_items = Vec::new();
    let mut local_items = Vec::new();
    let total_items = items.len() as u64;
    let mut processed_items = 0_u64;

    for item in items {
        operation_state.checkpoint(&job_id, None)?;

        match (
            archive::parse_archive_uri(&item.from),
            archive::parse_archive_uri(&item.to),
            parse_remote_path(&item.from),
            parse_remote_path(&item.to),
        ) {
            (Some(archive_from), None, None, None) => archive_items.push((item, archive_from)),
            (Some(_), _, _, _) | (_, Some(_), _, _) => {
                return Err(archive_read_only_error(
                    if archive::is_archive_uri(&item.from) {
                        &item.from
                    } else {
                        &item.to
                    },
                ));
            }
            (None, None, Some(remote_from), Some(remote_to)) => {
                let target_uri =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                copy_remote_item(&remotes, remote_from, remote_to, item.overwrite).await?;
                processed_items = processed_items.saturating_add(1);
                emit_file_operation_progress(
                    &app,
                    &job_id,
                    "copy",
                    "running",
                    ProgressSnapshot {
                        processed_entries: processed_items,
                        total_entries: total_items,
                        current_path: Some(target_uri),
                        ..ProgressSnapshot::default()
                    },
                );
            }
            (None, None, Some(remote_from), None) => {
                let target = expand_local_path(&item.to)?;
                copy_remote_to_local_item(&remotes, remote_from, &target, item.overwrite).await?;
                processed_items = processed_items.saturating_add(1);
                emit_file_operation_progress(
                    &app,
                    &job_id,
                    "copy",
                    "running",
                    ProgressSnapshot {
                        processed_entries: processed_items,
                        total_entries: total_items,
                        current_path: Some(item.to.clone()),
                        ..ProgressSnapshot::default()
                    },
                );
            }
            (None, None, None, Some(remote_to)) => {
                let source = expand_local_path(&item.from)?;
                let target_uri =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                copy_local_to_remote_item(
                    &remotes,
                    &source,
                    remote_to,
                    item.overwrite,
                    item.symlink_mode,
                )
                .await?;
                processed_items = processed_items.saturating_add(1);
                emit_file_operation_progress(
                    &app,
                    &job_id,
                    "copy",
                    "running",
                    ProgressSnapshot {
                        processed_entries: processed_items,
                        total_entries: total_items,
                        current_path: Some(target_uri),
                        ..ProgressSnapshot::default()
                    },
                );
            }
            (None, None, None, None) => local_items.push(item),
        }
    }

    if !archive_items.is_empty() {
        let archive_app = app.clone();
        let archive_job_id = job_id.clone();
        let archive_operation_state = operation_state.inner().clone();
        let archive_start = processed_items;

        run_local(move |_| {
            for (index, (item, archive_path)) in archive_items.iter().enumerate() {
                archive_operation_state.checkpoint(&archive_job_id, None)?;
                archive::extract_archive_entry_to(
                    archive_path,
                    Path::new(&item.to),
                    item.overwrite,
                )?;
                emit_file_operation_progress(
                    &archive_app,
                    &archive_job_id,
                    "copy",
                    "running",
                    ProgressSnapshot {
                        processed_entries: archive_start + index as u64 + 1,
                        total_entries: total_items,
                        current_path: Some(item.to.clone()),
                        ..ProgressSnapshot::default()
                    },
                );
            }

            Ok(())
        })
        .await?;
    }

    if local_items.is_empty() {
        if let Some(job_id) = &job_id {
            operation_state.clear_cancel(job_id);
            operation_state.clear_pause(job_id);
        }

        return Ok(());
    }

    let sudo_items = local_items.clone();
    let native_app = app.clone();
    let native_job_id = job_id.clone();
    let native_operation_state = operation_state.inner().clone();
    let sudo_app = app.clone();
    let sudo_job_id = job_id.clone();
    let sudo_operation_state = operation_state.inner().clone();
    let result = run_local_with_sudo(
        sudo_password,
        move |provider| {
            if native_job_id.is_some() {
                let operation_items = transfer_items_for_operations(&local_items);
                return operations::copy_items_with_progress(
                    &operation_items,
                    |progress| {
                        emit_transfer_operation_progress(
                            &native_app,
                            &native_job_id,
                            "copy",
                            "running",
                            progress,
                        );
                    },
                    |path| native_operation_state.checkpoint(&native_job_id, path),
                );
            }

            for item in local_items {
                provider.copy(&item.from, &item.to, item.overwrite)?;
            }

            Ok(())
        },
        move |password| {
            emit_file_operation_status(&sudo_app, &sudo_job_id, "copy", "running");
            for (index, item) in sudo_items.iter().enumerate() {
                sudo_operation_state.checkpoint(&sudo_job_id, None)?;
                sudo::copy_item(&password, &item.from, &item.to, item.overwrite)?;
                emit_file_operation_progress(
                    &sudo_app,
                    &sudo_job_id,
                    "copy",
                    "running",
                    ProgressSnapshot {
                        processed_entries: (index + 1) as u64,
                        total_entries: sudo_items.len() as u64,
                        current_path: Some(item.to.clone()),
                        ..ProgressSnapshot::default()
                    },
                );
            }

            Ok(())
        },
    )
    .await;

    if let Some(job_id) = &job_id {
        operation_state.clear_cancel(job_id);
        operation_state.clear_pause(job_id);
    }

    result
}

#[tauri::command]
pub async fn move_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    items: Vec<TransferItem>,
    job_id: Option<String>,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let mut local_items = Vec::new();
    let total_items = items.len() as u64;
    let mut processed_items = 0_u64;

    for item in items {
        operation_state.checkpoint(&job_id, None)?;

        if archive::is_archive_uri(&item.from) || archive::is_archive_uri(&item.to) {
            return Err(FsError::new(
                "archive_read_only",
                "Archive browsing is read-only. Copy items out of the archive instead.",
                Some(if archive::is_archive_uri(&item.from) {
                    item.from
                } else {
                    item.to
                }),
            ));
        }

        match (parse_remote_path(&item.from), parse_remote_path(&item.to)) {
            (Some(remote_from), Some(remote_to)) => {
                let target_uri =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                move_remote_item(&remotes, remote_from, remote_to, item.overwrite).await?;
                processed_items = processed_items.saturating_add(1);
                emit_file_operation_progress(
                    &app,
                    &job_id,
                    "move",
                    "running",
                    ProgressSnapshot {
                        processed_entries: processed_items,
                        total_entries: total_items,
                        current_path: Some(target_uri),
                        ..ProgressSnapshot::default()
                    },
                );
            }
            (Some(remote_from), None) => {
                let target = expand_local_path(&item.to)?;
                move_remote_to_local_item(&remotes, remote_from, &target, item.overwrite).await?;
                processed_items = processed_items.saturating_add(1);
                emit_file_operation_progress(
                    &app,
                    &job_id,
                    "move",
                    "running",
                    ProgressSnapshot {
                        processed_entries: processed_items,
                        total_entries: total_items,
                        current_path: Some(item.to.clone()),
                        ..ProgressSnapshot::default()
                    },
                );
            }
            (None, Some(remote_to)) => {
                let source = expand_local_path(&item.from)?;
                let target_uri =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                move_local_to_remote_item(
                    &remotes,
                    &source,
                    remote_to,
                    item.overwrite,
                    item.symlink_mode,
                )
                .await?;
                processed_items = processed_items.saturating_add(1);
                emit_file_operation_progress(
                    &app,
                    &job_id,
                    "move",
                    "running",
                    ProgressSnapshot {
                        processed_entries: processed_items,
                        total_entries: total_items,
                        current_path: Some(target_uri),
                        ..ProgressSnapshot::default()
                    },
                );
            }
            (None, None) => local_items.push(item),
        }
    }

    if local_items.is_empty() {
        if let Some(job_id) = &job_id {
            operation_state.clear_cancel(job_id);
            operation_state.clear_pause(job_id);
        }

        return Ok(());
    }

    let sudo_items = local_items.clone();
    let native_app = app.clone();
    let native_job_id = job_id.clone();
    let native_operation_state = operation_state.inner().clone();
    let sudo_app = app.clone();
    let sudo_job_id = job_id.clone();
    let sudo_operation_state = operation_state.inner().clone();
    let result = run_local_with_sudo(
        sudo_password,
        move |provider| {
            if native_job_id.is_some() {
                let operation_items = transfer_items_for_operations(&local_items);
                return operations::move_items_with_progress(
                    &operation_items,
                    |progress| {
                        emit_transfer_operation_progress(
                            &native_app,
                            &native_job_id,
                            "move",
                            "running",
                            progress,
                        );
                    },
                    |path| native_operation_state.checkpoint(&native_job_id, path),
                );
            }

            for item in local_items {
                provider.move_item(&item.from, &item.to, item.overwrite)?;
            }

            Ok(())
        },
        move |password| {
            emit_file_operation_status(&sudo_app, &sudo_job_id, "move", "running");
            for (index, item) in sudo_items.iter().enumerate() {
                sudo_operation_state.checkpoint(&sudo_job_id, None)?;
                sudo::move_item(&password, &item.from, &item.to, item.overwrite)?;
                emit_file_operation_progress(
                    &sudo_app,
                    &sudo_job_id,
                    "move",
                    "running",
                    ProgressSnapshot {
                        processed_entries: (index + 1) as u64,
                        total_entries: sudo_items.len() as u64,
                        current_path: Some(item.to.clone()),
                        ..ProgressSnapshot::default()
                    },
                );
            }

            Ok(())
        },
    )
    .await;

    if let Some(job_id) = &job_id {
        operation_state.clear_cancel(job_id);
        operation_state.clear_pause(job_id);
    }

    result
}

fn transfer_items_for_operations(items: &[TransferItem]) -> Vec<operations::LocalTransferItem> {
    items
        .iter()
        .map(|item| operations::LocalTransferItem {
            from: item.from.clone(),
            to: item.to.clone(),
            overwrite: item.overwrite,
            symlink_mode: item.symlink_mode,
        })
        .collect()
}
