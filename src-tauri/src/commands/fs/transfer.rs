use super::*;

/// One queued remote transfer, classified up front so a batch can be run
/// concurrently rather than one blocking `await` at a time.
enum RemoteTransfer {
    RemoteToRemote {
        from: RemotePath,
        to: RemotePath,
    },
    RemoteToLocal {
        from: RemotePath,
        to: std::path::PathBuf,
    },
    LocalToRemote {
        from: std::path::PathBuf,
        to: RemotePath,
        symlink_mode: operations::SymlinkMode,
    },
}

struct RemoteTask {
    transfer: RemoteTransfer,
    overwrite: bool,
    /// Path shown in progress events as the "current" item.
    label: String,
}

async fn run_one_remote_task(
    remotes: &RemoteVolumeState,
    is_move: bool,
    task: &RemoteTask,
) -> Result<(), FsError> {
    match &task.transfer {
        RemoteTransfer::RemoteToRemote { from, to } => {
            if is_move {
                move_remote_item(remotes, from.clone(), to.clone(), task.overwrite).await
            } else {
                copy_remote_item(remotes, from.clone(), to.clone(), task.overwrite).await
            }
        }
        RemoteTransfer::RemoteToLocal { from, to } => {
            if is_move {
                move_remote_to_local_item(remotes, from.clone(), to, task.overwrite).await
            } else {
                copy_remote_to_local_item(remotes, from.clone(), to, task.overwrite).await
            }
        }
        RemoteTransfer::LocalToRemote {
            from,
            to,
            symlink_mode,
        } => {
            if is_move {
                move_local_to_remote_item(remotes, from, to.clone(), task.overwrite, *symlink_mode)
                    .await
            } else {
                copy_local_to_remote_item(remotes, from, to.clone(), task.overwrite, *symlink_mode)
                    .await
            }
        }
    }
}

/// Run a batch of remote transfers with bounded concurrency. `buffer_unordered`
/// drives the futures cooperatively on one task (concurrent I/O, not parallel
/// threads), so there are no data races — only network latency is overlapped.
/// Individual failures are collected and aggregated (continue-on-error);
/// cancellation aborts the batch.
async fn run_remote_tasks<P>(
    remotes: &RemoteVolumeState,
    op_state: &FileOperationState,
    job_id: &Option<String>,
    is_move: bool,
    tasks: Vec<RemoteTask>,
    concurrency: usize,
    processed: &std::sync::atomic::AtomicU64,
    total: u64,
    on_progress: P,
) -> Result<(), FsError>
where
    P: Fn(u64, u64, String),
{
    use futures::stream::StreamExt;
    use std::sync::atomic::Ordering;

    let concurrency = concurrency.max(1);
    let on_progress = &on_progress;

    // Each future owns its task (rather than borrowing an iterator item), which
    // keeps the command's boxed future `Send` without higher-ranked lifetimes.
    let results: Vec<Result<(), FsError>> = futures::stream::iter(tasks)
        .map(|task| async move {
            // Cancellation is checked (non-blocking) before each transfer starts.
            if op_state.cancel_requested(job_id) {
                return Err(FsError::new(
                    "operation_cancelled",
                    "The file operation was cancelled.",
                    None,
                ));
            }

            run_one_remote_task(remotes, is_move, &task).await?;

            let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
            on_progress(done, total, task.label);
            Ok(())
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let mut failures = Vec::new();

    for result in results {
        if let Err(error) = result {
            if error.code == "operation_cancelled" {
                return Err(error);
            }

            failures.push(error);
        }
    }

    aggregate_transfer_failures(failures)
}

/// A lone failure is returned verbatim (so its code still drives sudo
/// escalation); multiple failures aggregate into one reported error.
fn aggregate_transfer_failures(mut failures: Vec<FsError>) -> Result<(), FsError> {
    match failures.len() {
        0 => Ok(()),
        1 => Err(failures.remove(0)),
        count => {
            let first = failures.remove(0);
            Err(FsError::new(
                "operation_partial_failure",
                format!(
                    "{count} items could not be completed. First error: {}",
                    first.message
                ),
                first.path,
            ))
        }
    }
}

#[tauri::command]
pub async fn list_directory(
    path: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
    app_store: tauri::State<'_, AppStoreState>,
) -> Result<Vec<FileEntry>, FsError> {
    let mut entries = if let Some(archive_path) = archive::parse_archive_uri(&path) {
        run_local(move |_| archive::list_archive_directory(&archive_path)).await?
    } else if let Some(remote_path) = parse_remote_path(&path) {
        list_remote_directory(&remotes, remote_path).await?
    } else {
        let sudo_path = path.clone();
        run_local_with_sudo(
            sudo_password,
            move |provider| provider.list(&path),
            move |password| sudo::list_directory(&password, &sudo_path),
        )
        .await?
    };

    app_store.apply_file_tags(&mut entries);
    Ok(entries)
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
pub async fn create_file(
    path: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    if archive::is_archive_uri(&path) {
        return Err(archive_read_only_error(&path));
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        return create_remote_file(&remotes, remote_path).await;
    }

    let sudo_path = path.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| provider.create_file(&path),
        move |password| sudo::create_file(&password, &sudo_path),
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
pub async fn restore_from_trash(paths: Vec<String>) -> Result<(), FsError> {
    for path in &paths {
        if archive::is_archive_uri(path) || parse_remote_path(path).is_some() {
            return Err(FsError::new(
                "restore_unsupported",
                "Restoring from Trash is available for local items only.",
                Some(path.clone()),
            ));
        }
    }

    run_local(move |_| restore_local_paths_from_trash(paths)).await
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore_local_paths_from_trash(paths: Vec<String>) -> FsResult<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let items = trash::os_limited::list().map_err(|error| {
        FsError::new(
            "trash_list_failed",
            format!("Unable to read the Trash: {error}"),
            None,
        )
    })?;

    let mut to_restore = Vec::with_capacity(paths.len());

    for path in &paths {
        let target = expand_local_path(path)?;
        // Pick the most recently trashed item matching this original path.
        let best = items
            .iter()
            .filter(|item| item.original_path() == target)
            .max_by_key(|item| item.time_deleted)
            .cloned();

        match best {
            Some(item) => to_restore.push(item),
            None => {
                return Err(FsError::new(
                    "trash_item_not_found",
                    "This item is no longer in the Trash and cannot be restored.",
                    Some(target.to_string_lossy().into_owned()),
                ));
            }
        }
    }

    trash::os_limited::restore_all(to_restore).map_err(|error| {
        FsError::new(
            "trash_restore_failed",
            format!("Unable to restore from Trash: {error}"),
            None,
        )
    })
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
fn restore_local_paths_from_trash(_paths: Vec<String>) -> FsResult<()> {
    Err(FsError::new(
        "trash_restore_unsupported",
        "Restoring from Trash is not supported on this platform.",
        None,
    ))
}

#[tauri::command]
pub async fn copy_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    items: Vec<TransferItem>,
    job_id: Option<String>,
    sudo_password: Option<String>,
    max_concurrency: Option<u32>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let mut archive_items = Vec::new();
    let mut local_items = Vec::new();
    let mut remote_tasks: Vec<RemoteTask> = Vec::new();
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
                let label =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                remote_tasks.push(RemoteTask {
                    transfer: RemoteTransfer::RemoteToRemote {
                        from: remote_from,
                        to: remote_to,
                    },
                    overwrite: item.overwrite,
                    label,
                });
            }
            (None, None, Some(remote_from), None) => {
                let target = expand_local_path(&item.to)?;
                remote_tasks.push(RemoteTask {
                    transfer: RemoteTransfer::RemoteToLocal {
                        from: remote_from,
                        to: target,
                    },
                    overwrite: item.overwrite,
                    label: item.to.clone(),
                });
            }
            (None, None, None, Some(remote_to)) => {
                let source = expand_local_path(&item.from)?;
                let label =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                remote_tasks.push(RemoteTask {
                    transfer: RemoteTransfer::LocalToRemote {
                        from: source,
                        to: remote_to,
                        symlink_mode: item.symlink_mode,
                    },
                    overwrite: item.overwrite,
                    label,
                });
            }
            (None, None, None, None) => local_items.push(item),
        }
    }

    let processed = std::sync::atomic::AtomicU64::new(processed_items);

    if !remote_tasks.is_empty() {
        let concurrency = crate::fs::storage::resolve_concurrency(
            crate::fs::storage::StorageClass::Remote,
            max_concurrency.map(|value| value as usize),
        );
        let progress_app = app.clone();
        let progress_job_id = job_id.clone();

        run_remote_tasks(
            &remotes,
            &operation_state,
            &job_id,
            false,
            remote_tasks,
            concurrency,
            &processed,
            total_items,
            |done, total, label| {
                emit_file_operation_progress(
                    &progress_app,
                    &progress_job_id,
                    "copy",
                    "running",
                    ProgressSnapshot {
                        processed_entries: done,
                        total_entries: total,
                        current_path: Some(label),
                        ..ProgressSnapshot::default()
                    },
                );
            },
        )
        .await?;
    }

    processed_items = processed.load(std::sync::atomic::Ordering::Relaxed);

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

    // Choose how parallel the local copy may run, based on the destination's
    // storage type (spinning disks stay serial) and the user's override.
    let local_concurrency = crate::fs::storage::resolve_concurrency(
        crate::fs::storage::classify_path(&local_items[0].to),
        max_concurrency.map(|value| value as usize),
    );

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

                // Closures are passed inline (not via a `let`) so the compiler
                // infers the higher-ranked lifetime the checkpoint signature needs.
                if local_concurrency > 1 {
                    return operations::copy_items_parallel(
                        &operation_items,
                        local_concurrency,
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
    max_concurrency: Option<u32>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let mut local_items = Vec::new();
    let mut remote_tasks: Vec<RemoteTask> = Vec::new();
    let total_items = items.len() as u64;
    let processed_items = 0_u64;

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
                let label =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                remote_tasks.push(RemoteTask {
                    transfer: RemoteTransfer::RemoteToRemote {
                        from: remote_from,
                        to: remote_to,
                    },
                    overwrite: item.overwrite,
                    label,
                });
            }
            (Some(remote_from), None) => {
                let target = expand_local_path(&item.to)?;
                remote_tasks.push(RemoteTask {
                    transfer: RemoteTransfer::RemoteToLocal {
                        from: remote_from,
                        to: target,
                    },
                    overwrite: item.overwrite,
                    label: item.to.clone(),
                });
            }
            (None, Some(remote_to)) => {
                let source = expand_local_path(&item.from)?;
                let label =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                remote_tasks.push(RemoteTask {
                    transfer: RemoteTransfer::LocalToRemote {
                        from: source,
                        to: remote_to,
                        symlink_mode: item.symlink_mode,
                    },
                    overwrite: item.overwrite,
                    label,
                });
            }
            (None, None) => local_items.push(item),
        }
    }

    if !remote_tasks.is_empty() {
        let concurrency = crate::fs::storage::resolve_concurrency(
            crate::fs::storage::StorageClass::Remote,
            max_concurrency.map(|value| value as usize),
        );
        let processed = std::sync::atomic::AtomicU64::new(processed_items);
        let progress_app = app.clone();
        let progress_job_id = job_id.clone();

        run_remote_tasks(
            &remotes,
            &operation_state,
            &job_id,
            true,
            remote_tasks,
            concurrency,
            &processed,
            total_items,
            |done, total, label| {
                emit_file_operation_progress(
                    &progress_app,
                    &progress_job_id,
                    "move",
                    "running",
                    ProgressSnapshot {
                        processed_entries: done,
                        total_entries: total,
                        current_path: Some(label),
                        ..ProgressSnapshot::default()
                    },
                );
            },
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_base(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "carelo-remote-tasks-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn fs_remote_state(root: &std::path::Path) -> RemoteVolumeState {
        let state = RemoteVolumeState::default();
        state
            .add(RemoteVolumeConfig {
                id: "test".to_string(),
                name: "Test".to_string(),
                scheme: "fs".to_string(),
                root: Some(root.to_string_lossy().into_owned()),
                options: HashMap::new(),
            })
            .expect("fs remote should register");
        state
    }

    fn local_to_remote_task(from: PathBuf, remote: &str, label: &str) -> RemoteTask {
        RemoteTask {
            transfer: RemoteTransfer::LocalToRemote {
                from,
                to: parse_remote_path(remote).expect("remote path parses"),
                symlink_mode: operations::SymlinkMode::Preserve,
            },
            overwrite: false,
            label: label.to_string(),
        }
    }

    #[test]
    fn run_remote_tasks_runs_concurrently_and_continues_past_failures() {
        let base = unique_base("continue");
        let src = base.join("src");
        let remote_root = base.join("remote");
        fs::create_dir_all(&src).expect("src dir");
        fs::create_dir_all(&remote_root).expect("remote dir");

        let mut tasks = Vec::new();
        for index in 0..6 {
            let file = src.join(format!("f{index}.txt"));
            fs::write(&file, format!("content-{index}")).expect("write source");
            tasks.push(local_to_remote_task(
                file,
                &format!("remote://test/f{index}.txt"),
                &format!("f{index}"),
            ));
        }
        // A missing source in the middle must not abort the healthy transfers.
        tasks.push(local_to_remote_task(
            src.join("missing.txt"),
            "remote://test/missing.txt",
            "missing",
        ));

        let state = fs_remote_state(&remote_root);
        let op_state = FileOperationState::default();
        let processed = AtomicU64::new(0);
        let progress_calls = AtomicU64::new(0);

        let result = tauri::async_runtime::block_on(run_remote_tasks(
            &state,
            &op_state,
            &None,
            false,
            tasks,
            4,
            &processed,
            7,
            |_done, _total, _label| {
                progress_calls.fetch_add(1, Ordering::Relaxed);
            },
        ));

        // One failure (the missing source) is reported, verbatim (not aggregated).
        assert!(result.is_err());
        assert_ne!(result.unwrap_err().code, "operation_partial_failure");

        for index in 0..6 {
            assert!(
                remote_root.join(format!("f{index}.txt")).is_file(),
                "f{index} should have been copied despite the failing item"
            );
        }
        assert_eq!(processed.load(Ordering::Relaxed), 6);
        assert_eq!(progress_calls.load(Ordering::Relaxed), 6);
        assert!(!remote_root.join("missing.txt").exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn run_remote_tasks_aggregates_multiple_failures() {
        let base = unique_base("aggregate");
        let src = base.join("src");
        let remote_root = base.join("remote");
        fs::create_dir_all(&src).expect("src dir");
        fs::create_dir_all(&remote_root).expect("remote dir");

        let good = src.join("good.txt");
        fs::write(&good, "good").expect("write good");

        let tasks = vec![
            local_to_remote_task(src.join("missing-a.txt"), "remote://test/a.txt", "a"),
            local_to_remote_task(good, "remote://test/good.txt", "good"),
            local_to_remote_task(src.join("missing-b.txt"), "remote://test/b.txt", "b"),
        ];

        let state = fs_remote_state(&remote_root);
        let op_state = FileOperationState::default();
        let processed = AtomicU64::new(0);

        let result = tauri::async_runtime::block_on(run_remote_tasks(
            &state,
            &op_state,
            &None,
            false,
            tasks,
            4,
            &processed,
            3,
            |_d, _t, _l| {},
        ));

        let error = result.expect_err("two failures should be reported");
        assert_eq!(error.code, "operation_partial_failure");
        assert!(remote_root.join("good.txt").is_file());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn run_remote_tasks_aborts_when_cancelled() {
        let base = unique_base("cancel");
        let src = base.join("src");
        let remote_root = base.join("remote");
        fs::create_dir_all(&src).expect("src dir");
        fs::create_dir_all(&remote_root).expect("remote dir");
        let file = src.join("f.txt");
        fs::write(&file, "data").expect("write source");

        let tasks = vec![local_to_remote_task(file, "remote://test/f.txt", "f")];
        let state = fs_remote_state(&remote_root);
        let op_state = FileOperationState::default();
        op_state.request_cancel("job-1");
        let processed = AtomicU64::new(0);

        let result = tauri::async_runtime::block_on(run_remote_tasks(
            &state,
            &op_state,
            &Some("job-1".to_string()),
            false,
            tasks,
            4,
            &processed,
            1,
            |_d, _t, _l| {},
        ));

        assert_eq!(
            result.expect_err("cancelled batch should error").code,
            "operation_cancelled"
        );
        assert_eq!(processed.load(Ordering::Relaxed), 0);

        let _ = fs::remove_dir_all(&base);
    }
}
