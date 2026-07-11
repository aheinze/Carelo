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

#[derive(Clone)]
struct IndexedTransferItem {
    index: usize,
    item: TransferItem,
}

struct RemoteTask {
    index: usize,
    transfer: RemoteTransfer,
    overwrite: bool,
    from: String,
    to: String,
    /// Path shown in progress events as the "current" item.
    label: String,
}

fn operation_error_with_batch(
    batch: FileOperationBatchResult,
) -> Result<FileOperationBatchResult, FsError> {
    if batch.is_complete() {
        return Ok(batch);
    }

    if batch.cancelled {
        let path = batch
            .items
            .iter()
            .find(|item| item.status == FileOperationItemStatus::Cancelled)
            .and_then(|item| item.errors.first().and_then(|error| error.path.clone()));
        return Err(FsError::new(
            "operation_cancelled",
            "The file operation was cancelled.",
            path,
        )
        .with_batch(batch));
    }

    let problem_items = batch
        .items
        .iter()
        .filter(|item| item.status != FileOperationItemStatus::Completed)
        .collect::<Vec<_>>();
    let errors = problem_items
        .iter()
        .flat_map(|item| item.errors.iter())
        .collect::<Vec<_>>();

    let error = if errors.len() == 1 {
        let error = errors[0];
        FsError::new(
            error.code.clone(),
            error.message.clone(),
            error.path.clone(),
        )
    } else {
        let first = errors.first();
        FsError::new(
            "operation_partial_failure",
            if let Some(first) = first {
                format!(
                    "{} items could not be completed. First error: {}",
                    problem_items.len(),
                    first.message
                )
            } else {
                format!("{} items could not be completed.", problem_items.len())
            },
            first.and_then(|error| error.path.clone()),
        )
    };

    Err(error.with_batch(batch))
}

fn finish_indexed_batch(
    templates: &[(String, Option<String>)],
    outcomes: Vec<FileOperationItemResult>,
    cancelled: bool,
) -> FileOperationBatchResult {
    let mut by_index = outcomes
        .into_iter()
        .map(|outcome| (outcome.index, outcome))
        .collect::<HashMap<_, _>>();
    let items = templates
        .iter()
        .enumerate()
        .map(|(index, (from, to))| {
            by_index.remove(&index).unwrap_or_else(|| {
                FileOperationItemResult::not_started(index, from.clone(), to.clone())
            })
        })
        .collect();

    FileOperationBatchResult::new(items, cancelled)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalPathSnapshot {
    kind: u8,
    len: u64,
    modified_nanos: Option<u128>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn local_path_snapshot(path: &str) -> Option<LocalPathSnapshot> {
    let path = expand_local_path(path).ok()?;
    let metadata = fs::symlink_metadata(path).ok()?;
    let file_type = metadata.file_type();

    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    Some(LocalPathSnapshot {
        kind: if file_type.is_symlink() {
            2
        } else if metadata.is_dir() {
            1
        } else {
            0
        },
        len: metadata.len(),
        modified_nanos: metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos()),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

fn local_path_exists_for_result(path: &str) -> bool {
    local_path_snapshot(path).is_some()
}

fn local_path_is_directory_for_result(path: &str) -> bool {
    local_path_snapshot(path)
        .map(|snapshot| snapshot.kind == 1)
        .unwrap_or(false)
}

fn copy_error_affected(
    item: &TransferItem,
    _error: &FsError,
    _source_before: &Option<LocalPathSnapshot>,
    destination_before: &Option<LocalPathSnapshot>,
) -> bool {
    local_path_snapshot(&item.to) != *destination_before
}

fn move_error_affected(
    item: &TransferItem,
    _error: &FsError,
    source_before: &Option<LocalPathSnapshot>,
    destination_before: &Option<LocalPathSnapshot>,
) -> bool {
    local_path_snapshot(&item.from) != *source_before
        || local_path_snapshot(&item.to) != *destination_before
}

fn remote_delete_error_affected(error: &FsError) -> bool {
    // delete_remote_item stats the top-level path before issuing any delete.
    // Every later provider failure may represent a partially removed tree.
    error.code != "remote_stat_failed"
}

fn remote_task_error_affected(
    task: &RemoteTask,
    error: &FsError,
    is_move: bool,
    local_source_before: &Option<LocalPathSnapshot>,
    local_destination_before: &Option<LocalPathSnapshot>,
) -> bool {
    match &task.transfer {
        RemoteTransfer::RemoteToLocal { to, .. } => {
            let destination_after = local_path_snapshot(&to.to_string_lossy());

            // A move can finish the local copy and then fail while deleting the
            // remote source. The local snapshot normally catches that commit;
            // keep the provider's delete failure conservative on every OS.
            if is_move && error.code == "remote_delete_failed" {
                return true;
            }

            // Absence before and after proves that the local side was cleaned.
            if local_destination_before.is_none() && destination_after.is_none() {
                return false;
            }

            // These failures occur before a top-level local destination write.
            // Requiring an unchanged snapshot avoids treating the same codes
            // from a later directory traversal as clean.
            if matches!(
                error.code.as_str(),
                "remote_stat_failed" | "destination_exists" | "destination_type_conflict"
            ) && destination_after == *local_destination_before
            {
                return false;
            }

            true
        }
        RemoteTransfer::LocalToRemote { .. } if local_source_before.is_none() => {
            // Source metadata is read before an upload creates anything, so a
            // source that was already absent is a clean preflight failure.
            false
        }
        // Remote-to-remote helpers can reuse the same error codes during a
        // recursive traversal, after earlier descendants were copied. Without
        // a trustworthy provider snapshot, affected=true is the safe result.
        _ => true,
    }
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
) -> (Vec<FileOperationItemResult>, bool)
where
    P: Fn(u64, u64, String),
{
    use futures::stream::StreamExt;
    use std::sync::atomic::Ordering;

    let concurrency = concurrency.max(1);
    let on_progress = &on_progress;

    // Each future owns its task (rather than borrowing an iterator item), which
    // keeps the command's boxed future `Send` without higher-ranked lifetimes.
    let results: Vec<(FileOperationItemResult, bool)> = futures::stream::iter(tasks)
        .map(|task| async move {
            // Cancellation is checked (non-blocking) before each transfer starts.
            if op_state.cancel_requested(job_id) {
                return (
                    FileOperationItemResult::not_started(task.index, task.from, Some(task.to)),
                    true,
                );
            }

            let local_source_before = match &task.transfer {
                RemoteTransfer::LocalToRemote { from, .. } => {
                    local_path_snapshot(&from.to_string_lossy())
                }
                _ => None,
            };
            let local_destination_before = match &task.transfer {
                RemoteTransfer::RemoteToLocal { to, .. } => {
                    local_path_snapshot(&to.to_string_lossy())
                }
                _ => None,
            };

            match run_one_remote_task(remotes, is_move, &task).await {
                Ok(()) => {
                    let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    on_progress(done, total, task.label);
                    (
                        FileOperationItemResult::completed(task.index, task.from, Some(task.to)),
                        false,
                    )
                }
                Err(error) if error.code == "operation_cancelled" => {
                    let affected = remote_task_error_affected(
                        &task,
                        &error,
                        is_move,
                        &local_source_before,
                        &local_destination_before,
                    );
                    (
                        FileOperationItemResult::cancelled(
                            task.index,
                            task.from,
                            Some(task.to),
                            error,
                            affected,
                        ),
                        true,
                    )
                }
                // Only errors known to happen during preflight are cleanly
                // retryable. Provider write/rename failures stay conservative.
                Err(error) => {
                    let affected = remote_task_error_affected(
                        &task,
                        &error,
                        is_move,
                        &local_source_before,
                        &local_destination_before,
                    );
                    (
                        FileOperationItemResult::failed(
                            task.index,
                            task.from,
                            Some(task.to),
                            error,
                            affected,
                        ),
                        false,
                    )
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let cancelled = results.iter().any(|(_, cancelled)| *cancelled);
    let mut outcomes = results
        .into_iter()
        .map(|(result, _)| result)
        .collect::<Vec<_>>();
    outcomes.sort_by_key(|result| result.index);
    (outcomes, cancelled)
}

fn local_transfer_item(item: &TransferItem) -> operations::LocalTransferItem {
    operations::LocalTransferItem {
        from: item.from.clone(),
        to: item.to.clone(),
        overwrite: item.overwrite,
        symlink_mode: item.symlink_mode,
    }
}

fn aggregate_copy_progress(
    slots: &Mutex<Vec<operations::OperationProgress>>,
    slot: usize,
    progress: operations::OperationProgress,
) -> operations::OperationProgress {
    let mut slots = slots.lock().expect("copy progress lock poisoned");
    slots[slot] = progress.clone();

    operations::OperationProgress {
        processed_bytes: slots.iter().map(|item| item.processed_bytes).sum(),
        total_bytes: slots.iter().map(|item| item.total_bytes).sum(),
        processed_entries: slots.iter().map(|item| item.processed_entries).sum(),
        total_entries: slots.iter().map(|item| item.total_entries).sum(),
        current_path: progress.current_path,
        current_bytes: progress.current_bytes,
        current_total_bytes: progress.current_total_bytes,
    }
}

fn run_native_copy_items(
    app: AppHandle,
    operation_state: FileOperationState,
    job_id: Option<String>,
    items: Vec<IndexedTransferItem>,
    concurrency: usize,
) -> (Vec<FileOperationItemResult>, bool) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    if items.is_empty() {
        return (Vec::new(), false);
    }

    let results = Mutex::new(vec![None; items.len()]);
    let progress_slots = Mutex::new(vec![operations::OperationProgress::default(); items.len()]);
    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let worker_count = concurrency.max(1).min(items.len());

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| loop {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }

                let slot = next.fetch_add(1, Ordering::Relaxed);
                let Some(indexed) = items.get(slot) else {
                    break;
                };
                let source_before = local_path_snapshot(&indexed.item.from);
                let destination_before = local_path_snapshot(&indexed.item.to);
                let operation_item = local_transfer_item(&indexed.item);
                let result = if items.len() == 1 && concurrency > 1 {
                    operations::copy_items_parallel(
                        &[operation_item],
                        concurrency,
                        |progress| {
                            let progress = aggregate_copy_progress(&progress_slots, slot, progress);
                            emit_transfer_operation_progress(
                                &app, &job_id, "copy", "running", progress,
                            );
                        },
                        |path| operation_state.checkpoint(&job_id, path),
                    )
                } else {
                    operations::copy_items_with_progress(
                        &[operation_item],
                        |progress| {
                            let progress = aggregate_copy_progress(&progress_slots, slot, progress);
                            emit_transfer_operation_progress(
                                &app, &job_id, "copy", "running", progress,
                            );
                        },
                        |path| operation_state.checkpoint(&job_id, path),
                    )
                };

                let outcome = match result {
                    Ok(()) => FileOperationItemResult::completed(
                        indexed.index,
                        indexed.item.from.clone(),
                        Some(indexed.item.to.clone()),
                    ),
                    Err(error) if error.code == "operation_cancelled" => {
                        cancelled.store(true, Ordering::Relaxed);
                        FileOperationItemResult::cancelled(
                            indexed.index,
                            indexed.item.from.clone(),
                            Some(indexed.item.to.clone()),
                            error.clone(),
                            copy_error_affected(
                                &indexed.item,
                                &error,
                                &source_before,
                                &destination_before,
                            ),
                        )
                    }
                    Err(error) => FileOperationItemResult::failed(
                        indexed.index,
                        indexed.item.from.clone(),
                        Some(indexed.item.to.clone()),
                        error.clone(),
                        copy_error_affected(
                            &indexed.item,
                            &error,
                            &source_before,
                            &destination_before,
                        ),
                    ),
                };

                results.lock().expect("copy result lock poisoned")[slot] = Some(outcome);
            });
        }
    });

    let outcomes = results
        .into_inner()
        .expect("copy result lock poisoned")
        .into_iter()
        .flatten()
        .collect();
    (outcomes, cancelled.load(Ordering::Relaxed))
}

fn run_sequential_local_transfer<E, A>(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    operation: &str,
    items: Vec<IndexedTransferItem>,
    mut execute: E,
    affected_after_error: A,
) -> (Vec<FileOperationItemResult>, bool)
where
    E: FnMut(&TransferItem) -> FsResult<()>,
    A: Fn(&TransferItem, &FsError, &Option<LocalPathSnapshot>, &Option<LocalPathSnapshot>) -> bool,
{
    let mut outcomes = Vec::new();
    let mut completed = 0_u64;
    let total = items.len() as u64;
    let mut cancelled = false;

    for indexed in items {
        if let Err(error) = operation_state.checkpoint(job_id, None) {
            outcomes.push(FileOperationItemResult::cancelled(
                indexed.index,
                indexed.item.from,
                Some(indexed.item.to),
                error,
                false,
            ));
            cancelled = true;
            break;
        }

        let source_before = local_path_snapshot(&indexed.item.from);
        let destination_before = local_path_snapshot(&indexed.item.to);
        let result = execute(&indexed.item);
        let outcome = match result {
            Ok(()) => {
                completed = completed.saturating_add(1);
                emit_file_operation_progress(
                    app,
                    job_id,
                    operation,
                    "running",
                    ProgressSnapshot {
                        processed_entries: completed,
                        total_entries: total,
                        current_path: Some(indexed.item.to.clone()),
                        ..ProgressSnapshot::default()
                    },
                );
                FileOperationItemResult::completed(
                    indexed.index,
                    indexed.item.from.clone(),
                    Some(indexed.item.to.clone()),
                )
            }
            Err(error) if error.code == "operation_cancelled" => {
                cancelled = true;
                FileOperationItemResult::cancelled(
                    indexed.index,
                    indexed.item.from.clone(),
                    Some(indexed.item.to.clone()),
                    error.clone(),
                    affected_after_error(
                        &indexed.item,
                        &error,
                        &source_before,
                        &destination_before,
                    ),
                )
            }
            Err(error) => {
                let affected = affected_after_error(
                    &indexed.item,
                    &error,
                    &source_before,
                    &destination_before,
                );
                FileOperationItemResult::failed(
                    indexed.index,
                    indexed.item.from.clone(),
                    Some(indexed.item.to.clone()),
                    error,
                    affected,
                )
            }
        };
        outcomes.push(outcome);

        if cancelled {
            break;
        }
    }

    (outcomes, cancelled)
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
pub async fn set_permissions(
    path: String,
    mode: u32,
    recursive: bool,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    if archive::is_archive_uri(&path) {
        return Err(archive_read_only_error(&path));
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        // Mount-backed remotes (fs / SMB / password-SFTP) resolve to a real local
        // path; chmod it locally, which also supports recursion and sudo.
        if let Some(local_path) = remote_local_object_path(&remotes, &remote_path)? {
            let native = local_path.to_string_lossy().into_owned();
            let sudo_native = native.clone();
            let result = run_local_with_sudo(
                sudo_password,
                move |provider| provider.set_permissions(&native, mode, recursive),
                move |password| sudo::set_permissions(&password, &sudo_native, mode, recursive),
            )
            .await;

            if result.is_ok() {
                // Drop cached metadata so the new permissions show on refresh.
                let _ = remotes.invalidate_cache_for_path(&remote_path);
            }

            return result;
        }

        // Otherwise it's key-authenticated SFTP (no local mount): SETSTAT can
        // only target a single item.
        if recursive {
            return Err(FsError::new(
                "unsupported_target",
                "Recursive permission changes aren't available over SFTP.",
                Some(path),
            ));
        }

        return set_remote_sftp_permissions(&remotes, remote_path, mode).await;
    }

    let sudo_path = path.clone();
    run_local_with_sudo(
        sudo_password,
        move |provider| provider.set_permissions(&path, mode, recursive),
        move |password| sudo::set_permissions(&password, &sudo_path, mode, recursive),
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
) -> Result<FileOperationBatchResult, FsError> {
    let delete_mode = delete_mode.unwrap_or_default();
    let templates = paths
        .iter()
        .cloned()
        .map(|path| (path, None))
        .collect::<Vec<_>>();
    let mut local_paths = Vec::new();
    let mut remote_paths = Vec::new();
    let mut outcomes = Vec::new();

    for (index, path) in paths.into_iter().enumerate() {
        if archive::is_archive_uri(&path) {
            outcomes.push(FileOperationItemResult::failed(
                index,
                path.clone(),
                None,
                archive_read_only_error(&path),
                false,
            ));
        } else if let Some(remote_path) = parse_remote_path(&path) {
            remote_paths.push((index, path, remote_path));
        } else {
            local_paths.push((index, path));
        }
    }

    for (index, path, remote_path) in remote_paths {
        match delete_remote_item(&remotes, remote_path).await {
            Ok(()) => outcomes.push(FileOperationItemResult::completed(index, path, None)),
            Err(error) => {
                let affected = remote_delete_error_affected(&error);
                outcomes.push(FileOperationItemResult::failed(
                    index, path, None, error, affected,
                ));
            }
        }
    }

    if !local_paths.is_empty() {
        let local_result = if delete_mode == DeleteMode::Trash {
            run_local(move |_| Ok(move_local_paths_to_trash(local_paths))).await
        } else {
            let sudo_paths = local_paths.clone();
            run_local_with_sudo(
                sudo_password,
                move |provider| {
                    Ok(delete_local_paths_permanently(local_paths, |path| {
                        provider.delete(path)
                    }))
                },
                move |password| {
                    Ok(delete_local_paths_permanently(sudo_paths, |path| {
                        sudo::delete_item(&password, path)
                    }))
                },
            )
            .await
        };

        match local_result {
            Ok(local_outcomes) => outcomes.extend(local_outcomes),
            Err(error) => {
                for (index, (from, _)) in templates.iter().enumerate() {
                    if outcomes.iter().any(|outcome| outcome.index == index) {
                        continue;
                    }
                    outcomes.push(FileOperationItemResult::failed(
                        index,
                        from.clone(),
                        None,
                        error.clone(),
                        true,
                    ));
                }
            }
        }
    }

    operation_error_with_batch(finish_indexed_batch(&templates, outcomes, false))
}

fn move_local_paths_to_trash(paths: Vec<(usize, String)>) -> Vec<FileOperationItemResult> {
    let mut outcomes = Vec::with_capacity(paths.len());

    for (index, original_path) in paths {
        let path = match expand_local_path(&original_path) {
            Ok(path) => path,
            Err(error) => {
                outcomes.push(FileOperationItemResult::failed(
                    index,
                    original_path,
                    None,
                    error,
                    false,
                ));
                continue;
            }
        };

        if path.as_os_str().is_empty() || path == Path::new("/") {
            outcomes.push(FileOperationItemResult::failed(
                index,
                original_path,
                None,
                FsError::new(
                    "unsafe_delete_target",
                    "Refusing to move the root directory to Trash.",
                    Some(path.to_string_lossy().into_owned()),
                ),
                false,
            ));
            continue;
        }

        match trash::delete(&path) {
            Ok(()) => outcomes.push(FileOperationItemResult::completed(
                index,
                original_path,
                None,
            )),
            Err(error) => {
                let affected = fs::symlink_metadata(&path).is_err();
                outcomes.push(FileOperationItemResult::failed(
                    index,
                    original_path,
                    None,
                    FsError::new(
                        "trash_delete_failed",
                        format!("Unable to move item to Trash: {error}"),
                        Some(path.to_string_lossy().into_owned()),
                    ),
                    affected,
                ));
            }
        }
    }

    outcomes
}

fn delete_local_paths_permanently<E>(
    paths: Vec<(usize, String)>,
    mut execute: E,
) -> Vec<FileOperationItemResult>
where
    E: FnMut(&str) -> FsResult<()>,
{
    paths
        .into_iter()
        .map(|(index, path)| {
            let existed_before = local_path_exists_for_result(&path);
            let was_directory = local_path_is_directory_for_result(&path);
            match execute(&path) {
                Ok(()) => FileOperationItemResult::completed(index, path, None),
                Err(error) => {
                    let authentication_failed = matches!(
                        error.code.as_str(),
                        "sudo_auth_failed" | "sudo_password_required"
                    );
                    let affected = !authentication_failed
                        && ((existed_before && !local_path_exists_for_result(&path))
                            || was_directory);
                    FileOperationItemResult::failed(index, path, None, error, affected)
                }
            }
        })
        .collect()
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
) -> Result<FileOperationBatchResult, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let templates = items
        .iter()
        .map(|item| (item.from.clone(), Some(item.to.clone())))
        .collect::<Vec<_>>();
    let mut archive_items = Vec::new();
    let mut local_items = Vec::new();
    let mut remote_tasks: Vec<RemoteTask> = Vec::new();
    let total_items = items.len() as u64;
    let mut outcomes = Vec::new();
    let mut cancelled = false;

    for (index, item) in items.into_iter().enumerate() {
        let indexed = IndexedTransferItem { index, item };
        match (
            archive::parse_archive_uri(&indexed.item.from),
            archive::parse_archive_uri(&indexed.item.to),
            parse_remote_path(&indexed.item.from),
            parse_remote_path(&indexed.item.to),
        ) {
            (Some(archive_from), None, None, None) => archive_items.push((indexed, archive_from)),
            (Some(_), _, _, _) | (_, Some(_), _, _) => {
                let error_path = if archive::is_archive_uri(&indexed.item.from) {
                    indexed.item.from.clone()
                } else {
                    indexed.item.to.clone()
                };
                outcomes.push(FileOperationItemResult::failed(
                    indexed.index,
                    indexed.item.from,
                    Some(indexed.item.to),
                    archive_read_only_error(&error_path),
                    false,
                ));
            }
            (None, None, Some(remote_from), Some(remote_to)) => {
                let label =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                remote_tasks.push(RemoteTask {
                    index: indexed.index,
                    transfer: RemoteTransfer::RemoteToRemote {
                        from: remote_from,
                        to: remote_to,
                    },
                    overwrite: indexed.item.overwrite,
                    from: indexed.item.from,
                    to: indexed.item.to,
                    label,
                });
            }
            (None, None, Some(remote_from), None) => match expand_local_path(&indexed.item.to) {
                Ok(target) => remote_tasks.push(RemoteTask {
                    index: indexed.index,
                    transfer: RemoteTransfer::RemoteToLocal {
                        from: remote_from,
                        to: target,
                    },
                    overwrite: indexed.item.overwrite,
                    from: indexed.item.from,
                    to: indexed.item.to.clone(),
                    label: indexed.item.to,
                }),
                Err(error) => outcomes.push(FileOperationItemResult::failed(
                    indexed.index,
                    indexed.item.from,
                    Some(indexed.item.to),
                    error,
                    false,
                )),
            },
            (None, None, None, Some(remote_to)) => match expand_local_path(&indexed.item.from) {
                Ok(source) => {
                    let label =
                        crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                    remote_tasks.push(RemoteTask {
                        index: indexed.index,
                        transfer: RemoteTransfer::LocalToRemote {
                            from: source,
                            to: remote_to,
                            symlink_mode: indexed.item.symlink_mode,
                        },
                        overwrite: indexed.item.overwrite,
                        from: indexed.item.from,
                        to: indexed.item.to,
                        label,
                    });
                }
                Err(error) => outcomes.push(FileOperationItemResult::failed(
                    indexed.index,
                    indexed.item.from,
                    Some(indexed.item.to),
                    error,
                    false,
                )),
            },
            (None, None, None, None) => local_items.push(indexed),
        }
    }

    let processed = std::sync::atomic::AtomicU64::new(0);

    if !remote_tasks.is_empty() {
        let concurrency = crate::fs::storage::resolve_concurrency(
            crate::fs::storage::StorageClass::Remote,
            max_concurrency.map(|value| value as usize),
        );
        let progress_app = app.clone();
        let progress_job_id = job_id.clone();
        let (remote_outcomes, remote_cancelled) = run_remote_tasks(
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
        .await;
        outcomes.extend(remote_outcomes);
        cancelled |= remote_cancelled;
    }

    let processed_items = processed.load(std::sync::atomic::Ordering::Relaxed);

    if !cancelled && !archive_items.is_empty() {
        let archive_templates = archive_items
            .iter()
            .map(|(indexed, _)| {
                (
                    indexed.index,
                    indexed.item.from.clone(),
                    indexed.item.to.clone(),
                )
            })
            .collect::<Vec<_>>();
        let archive_app = app.clone();
        let archive_job_id = job_id.clone();
        let archive_operation_state = operation_state.inner().clone();
        let archive_start = processed_items;
        let archive_result = run_local(move |_| {
            let mut archive_outcomes = Vec::new();
            let mut archive_cancelled = false;

            for (offset, (indexed, archive_path)) in archive_items.into_iter().enumerate() {
                if let Err(error) = archive_operation_state.checkpoint(&archive_job_id, None) {
                    archive_outcomes.push(FileOperationItemResult::cancelled(
                        indexed.index,
                        indexed.item.from,
                        Some(indexed.item.to),
                        error,
                        false,
                    ));
                    archive_cancelled = true;
                    break;
                }

                let destination_existed = local_path_exists_for_result(&indexed.item.to);
                match archive::extract_archive_entry_to(
                    &archive_path,
                    Path::new(&indexed.item.to),
                    indexed.item.overwrite,
                ) {
                    Ok(()) => {
                        emit_file_operation_progress(
                            &archive_app,
                            &archive_job_id,
                            "copy",
                            "running",
                            ProgressSnapshot {
                                processed_entries: archive_start + offset as u64 + 1,
                                total_entries: total_items,
                                current_path: Some(indexed.item.to.clone()),
                                ..ProgressSnapshot::default()
                            },
                        );
                        archive_outcomes.push(FileOperationItemResult::completed(
                            indexed.index,
                            indexed.item.from,
                            Some(indexed.item.to),
                        ));
                    }
                    Err(error) => archive_outcomes.push(FileOperationItemResult::failed(
                        indexed.index,
                        indexed.item.from,
                        Some(indexed.item.to.clone()),
                        error,
                        indexed.item.overwrite
                            || (!destination_existed
                                && local_path_exists_for_result(&indexed.item.to)),
                    )),
                }
            }

            Ok((archive_outcomes, archive_cancelled))
        })
        .await;

        match archive_result {
            Ok((archive_outcomes, archive_cancelled)) => {
                outcomes.extend(archive_outcomes);
                cancelled |= archive_cancelled;
            }
            Err(error) => {
                for (index, from, to) in archive_templates {
                    if outcomes.iter().any(|outcome| outcome.index == index) {
                        continue;
                    }
                    outcomes.push(FileOperationItemResult::failed(
                        index,
                        from,
                        Some(to),
                        error.clone(),
                        true,
                    ));
                }
            }
        }
    }

    if !cancelled && !local_items.is_empty() {
        let local_concurrency = crate::fs::storage::resolve_concurrency(
            crate::fs::storage::classify_path(&local_items[0].item.to),
            max_concurrency.map(|value| value as usize),
        );
        let sudo_items = local_items.clone();
        let native_app = app.clone();
        let native_job_id = job_id.clone();
        let native_operation_state = operation_state.inner().clone();
        let sudo_app = app.clone();
        let sudo_job_id = job_id.clone();
        let sudo_operation_state = operation_state.inner().clone();
        let local_result = run_local_with_sudo(
            sudo_password,
            move |_| {
                Ok(run_native_copy_items(
                    native_app,
                    native_operation_state,
                    native_job_id,
                    local_items,
                    local_concurrency,
                ))
            },
            move |password| {
                emit_file_operation_status(&sudo_app, &sudo_job_id, "copy", "running");
                Ok(run_sequential_local_transfer(
                    &sudo_app,
                    &sudo_operation_state,
                    &sudo_job_id,
                    "copy",
                    sudo_items,
                    |item| sudo::copy_item(&password, &item.from, &item.to, item.overwrite),
                    copy_error_affected,
                ))
            },
        )
        .await;

        match local_result {
            Ok((local_outcomes, local_cancelled)) => {
                outcomes.extend(local_outcomes);
                cancelled |= local_cancelled;
            }
            Err(error) => {
                for (index, (from, to)) in templates.iter().enumerate() {
                    if outcomes.iter().any(|outcome| outcome.index == index) {
                        continue;
                    }
                    outcomes.push(FileOperationItemResult::failed(
                        index,
                        from.clone(),
                        to.clone(),
                        error.clone(),
                        true,
                    ));
                }
            }
        }
    }

    operation_error_with_batch(finish_indexed_batch(&templates, outcomes, cancelled))
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
) -> Result<FileOperationBatchResult, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let templates = items
        .iter()
        .map(|item| (item.from.clone(), Some(item.to.clone())))
        .collect::<Vec<_>>();
    let mut local_items = Vec::new();
    let mut remote_tasks: Vec<RemoteTask> = Vec::new();
    let total_items = items.len() as u64;
    let mut outcomes = Vec::new();
    let mut cancelled = false;

    for (index, item) in items.into_iter().enumerate() {
        let indexed = IndexedTransferItem { index, item };

        if archive::is_archive_uri(&indexed.item.from) || archive::is_archive_uri(&indexed.item.to)
        {
            let path = if archive::is_archive_uri(&indexed.item.from) {
                indexed.item.from.clone()
            } else {
                indexed.item.to.clone()
            };
            outcomes.push(FileOperationItemResult::failed(
                indexed.index,
                indexed.item.from,
                Some(indexed.item.to),
                FsError::new(
                    "archive_read_only",
                    "Archive browsing is read-only. Copy items out of the archive instead.",
                    Some(path),
                ),
                false,
            ));
            continue;
        }

        match (
            parse_remote_path(&indexed.item.from),
            parse_remote_path(&indexed.item.to),
        ) {
            (Some(remote_from), Some(remote_to)) => {
                let label =
                    crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                remote_tasks.push(RemoteTask {
                    index: indexed.index,
                    transfer: RemoteTransfer::RemoteToRemote {
                        from: remote_from,
                        to: remote_to,
                    },
                    overwrite: indexed.item.overwrite,
                    from: indexed.item.from,
                    to: indexed.item.to,
                    label,
                });
            }
            (Some(remote_from), None) => match expand_local_path(&indexed.item.to) {
                Ok(target) => remote_tasks.push(RemoteTask {
                    index: indexed.index,
                    transfer: RemoteTransfer::RemoteToLocal {
                        from: remote_from,
                        to: target,
                    },
                    overwrite: indexed.item.overwrite,
                    from: indexed.item.from,
                    to: indexed.item.to.clone(),
                    label: indexed.item.to,
                }),
                Err(error) => outcomes.push(FileOperationItemResult::failed(
                    indexed.index,
                    indexed.item.from,
                    Some(indexed.item.to),
                    error,
                    false,
                )),
            },
            (None, Some(remote_to)) => match expand_local_path(&indexed.item.from) {
                Ok(source) => {
                    let label =
                        crate::fs::remote::format_remote_uri(&remote_to.volume_id, &remote_to.path);
                    remote_tasks.push(RemoteTask {
                        index: indexed.index,
                        transfer: RemoteTransfer::LocalToRemote {
                            from: source,
                            to: remote_to,
                            symlink_mode: indexed.item.symlink_mode,
                        },
                        overwrite: indexed.item.overwrite,
                        from: indexed.item.from,
                        to: indexed.item.to,
                        label,
                    });
                }
                Err(error) => outcomes.push(FileOperationItemResult::failed(
                    indexed.index,
                    indexed.item.from,
                    Some(indexed.item.to),
                    error,
                    false,
                )),
            },
            (None, None) => local_items.push(indexed),
        }
    }

    if !remote_tasks.is_empty() {
        let concurrency = crate::fs::storage::resolve_concurrency(
            crate::fs::storage::StorageClass::Remote,
            max_concurrency.map(|value| value as usize),
        );
        let processed = std::sync::atomic::AtomicU64::new(0);
        let progress_app = app.clone();
        let progress_job_id = job_id.clone();
        let (remote_outcomes, remote_cancelled) = run_remote_tasks(
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
        .await;
        outcomes.extend(remote_outcomes);
        cancelled |= remote_cancelled;
    }

    if !cancelled && !local_items.is_empty() {
        let sudo_items = local_items.clone();
        let native_app = app.clone();
        let native_job_id = job_id.clone();
        let native_operation_state = operation_state.inner().clone();
        let operation_app = native_app.clone();
        let operation_job_id = native_job_id.clone();
        let operation_checkpoint_state = native_operation_state.clone();
        let sudo_app = app.clone();
        let sudo_job_id = job_id.clone();
        let sudo_operation_state = operation_state.inner().clone();
        let local_result = run_local_with_sudo(
            sudo_password,
            move |_| {
                Ok(run_sequential_local_transfer(
                    &native_app,
                    &native_operation_state,
                    &native_job_id,
                    "move",
                    local_items,
                    |item| {
                        operations::move_items_with_progress(
                            &[local_transfer_item(item)],
                            |progress| {
                                emit_transfer_operation_progress(
                                    &operation_app,
                                    &operation_job_id,
                                    "move",
                                    "running",
                                    progress,
                                );
                            },
                            |path| operation_checkpoint_state.checkpoint(&operation_job_id, path),
                        )
                    },
                    move_error_affected,
                ))
            },
            move |password| {
                emit_file_operation_status(&sudo_app, &sudo_job_id, "move", "running");
                Ok(run_sequential_local_transfer(
                    &sudo_app,
                    &sudo_operation_state,
                    &sudo_job_id,
                    "move",
                    sudo_items,
                    |item| sudo::move_item(&password, &item.from, &item.to, item.overwrite),
                    move_error_affected,
                ))
            },
        )
        .await;

        match local_result {
            Ok((local_outcomes, local_cancelled)) => {
                outcomes.extend(local_outcomes);
                cancelled |= local_cancelled;
            }
            Err(error) => {
                for (index, (from, to)) in templates.iter().enumerate() {
                    if outcomes.iter().any(|outcome| outcome.index == index) {
                        continue;
                    }
                    outcomes.push(FileOperationItemResult::failed(
                        index,
                        from.clone(),
                        to.clone(),
                        error.clone(),
                        true,
                    ));
                }
            }
        }
    }

    operation_error_with_batch(finish_indexed_batch(&templates, outcomes, cancelled))
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

    fn local_to_remote_task(index: usize, from: PathBuf, remote: &str, label: &str) -> RemoteTask {
        RemoteTask {
            index,
            transfer: RemoteTransfer::LocalToRemote {
                from: from.clone(),
                to: parse_remote_path(remote).expect("remote path parses"),
                symlink_mode: operations::SymlinkMode::Preserve,
            },
            overwrite: false,
            from: from.to_string_lossy().into_owned(),
            to: remote.to_string(),
            label: label.to_string(),
        }
    }

    #[test]
    fn batch_error_preserves_lone_permission_failure_and_orders_items() {
        let batch = FileOperationBatchResult::new(
            vec![
                FileOperationItemResult::failed(
                    1,
                    "/protected".to_string(),
                    Some("/target/protected".to_string()),
                    FsError::new(
                        "permission_denied",
                        "Permission denied",
                        Some("/protected".to_string()),
                    ),
                    false,
                ),
                FileOperationItemResult::completed(
                    0,
                    "/source".to_string(),
                    Some("/target/source".to_string()),
                ),
            ],
            false,
        );

        let error = operation_error_with_batch(batch).expect_err("batch should be incomplete");
        assert_eq!(error.code, "permission_denied");
        let batch = error.batch.expect("structured batch should be attached");
        assert_eq!(batch.items[0].index, 0);
        assert_eq!(batch.items[1].index, 1);
        assert_eq!(batch.items[1].status, FileOperationItemStatus::Failed);
    }

    #[test]
    fn recursive_remote_errors_remain_conservatively_affected() {
        let task = RemoteTask {
            index: 0,
            transfer: RemoteTransfer::RemoteToRemote {
                from: parse_remote_path("remote://left/Folder").expect("source parses"),
                to: parse_remote_path("remote://right/Folder").expect("target parses"),
            },
            overwrite: false,
            from: "remote://left/Folder".to_string(),
            to: "remote://right/Folder".to_string(),
            label: "Folder".to_string(),
        };
        let error = FsError::new(
            "remote_stat_failed",
            "A descendant could not be read.",
            Some("remote://left/Folder/child".to_string()),
        );

        assert!(remote_task_error_affected(
            &task, &error, false, &None, &None
        ));
    }

    #[test]
    fn finish_batch_marks_unreported_items_not_started() {
        let templates = vec![
            ("/a".to_string(), Some("/to-a".to_string())),
            ("/b".to_string(), Some("/to-b".to_string())),
        ];
        let batch = finish_indexed_batch(
            &templates,
            vec![FileOperationItemResult::completed(
                0,
                "/a".to_string(),
                Some("/to-a".to_string()),
            )],
            true,
        );

        assert!(batch.cancelled);
        assert_eq!(batch.items[0].status, FileOperationItemStatus::Completed);
        assert_eq!(batch.items[1].status, FileOperationItemStatus::NotStarted);
    }

    #[test]
    fn permanent_delete_reports_each_top_level_path() {
        let root = unique_base("delete-results");
        fs::create_dir_all(&root).expect("test root");
        let existing = root.join("existing.txt");
        let missing = root.join("missing.txt");
        fs::write(&existing, "delete me").expect("write existing");
        let provider = LocalFileProvider::new();
        let outcomes = delete_local_paths_permanently(
            vec![
                (1, missing.to_string_lossy().into_owned()),
                (0, existing.to_string_lossy().into_owned()),
            ],
            |path| provider.delete(path),
        );
        let templates = vec![
            (existing.to_string_lossy().into_owned(), None),
            (missing.to_string_lossy().into_owned(), None),
        ];
        let batch = finish_indexed_batch(&templates, outcomes, false);

        assert_eq!(batch.items[0].status, FileOperationItemStatus::Completed);
        assert_eq!(batch.items[1].status, FileOperationItemStatus::Failed);
        assert!(!batch.items[1].affected);
        assert!(!existing.exists());

        let _ = fs::remove_dir_all(root);
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
                index,
                file,
                &format!("remote://test/f{index}.txt"),
                &format!("f{index}"),
            ));
        }
        // A missing source in the middle must not abort the healthy transfers.
        tasks.push(local_to_remote_task(
            6,
            src.join("missing.txt"),
            "remote://test/missing.txt",
            "missing",
        ));

        let state = fs_remote_state(&remote_root);
        let op_state = FileOperationState::default();
        let processed = AtomicU64::new(0);
        let progress_calls = AtomicU64::new(0);

        let (results, cancelled) = tauri::async_runtime::block_on(run_remote_tasks(
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

        assert!(!cancelled);
        assert_eq!(results.len(), 7);
        assert!(results[..6]
            .iter()
            .all(|result| result.status == FileOperationItemStatus::Completed));
        assert_eq!(results[6].status, FileOperationItemStatus::Failed);
        assert_eq!(results[6].index, 6);

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
            local_to_remote_task(0, src.join("missing-a.txt"), "remote://test/a.txt", "a"),
            local_to_remote_task(1, good, "remote://test/good.txt", "good"),
            local_to_remote_task(2, src.join("missing-b.txt"), "remote://test/b.txt", "b"),
        ];

        let state = fs_remote_state(&remote_root);
        let op_state = FileOperationState::default();
        let processed = AtomicU64::new(0);

        let (results, cancelled) = tauri::async_runtime::block_on(run_remote_tasks(
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

        assert!(!cancelled);
        assert_eq!(results[0].status, FileOperationItemStatus::Failed);
        assert_eq!(results[1].status, FileOperationItemStatus::Completed);
        assert_eq!(results[2].status, FileOperationItemStatus::Failed);
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

        let tasks = vec![local_to_remote_task(0, file, "remote://test/f.txt", "f")];
        let state = fs_remote_state(&remote_root);
        let op_state = FileOperationState::default();
        op_state.request_cancel("job-1");
        let processed = AtomicU64::new(0);

        let (results, cancelled) = tauri::async_runtime::block_on(run_remote_tasks(
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

        assert!(cancelled);
        assert_eq!(results[0].status, FileOperationItemStatus::NotStarted);
        assert_eq!(processed.load(Ordering::Relaxed), 0);

        let _ = fs::remove_dir_all(&base);
    }
}
