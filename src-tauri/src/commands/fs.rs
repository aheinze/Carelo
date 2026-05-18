use crate::fs::local::LocalFileProvider;
use crate::fs::models::{FileEntry, FileMetadata, FsError, FsResult, VolumeEntry};
use crate::fs::provider::FileProvider;
use crate::fs::remote::{
    check_remote, copy_remote_item, create_remote_folder, delete_remote_item,
    list_remote_directory, move_remote_item, parse_remote_path, rename_remote_item,
    stat_remote_item, RemoteVolumeConfig, RemoteVolumeInfo, RemoteVolumeState,
};
use crate::fs::sudo;
use crate::fs::{archive, operations};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferItem {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub symlink_mode: operations::SymlinkMode,
}

#[derive(Clone, Default)]
pub struct FileOperationState {
    cancelled_jobs: Arc<Mutex<HashSet<String>>>,
    paused_jobs: Arc<Mutex<HashSet<String>>>,
}

impl FileOperationState {
    fn request_cancel(&self, job_id: &str) {
        if let Ok(mut cancelled_jobs) = self.cancelled_jobs.lock() {
            cancelled_jobs.insert(job_id.to_string());
        }
    }

    fn clear_cancel(&self, job_id: &str) {
        if let Ok(mut cancelled_jobs) = self.cancelled_jobs.lock() {
            cancelled_jobs.remove(job_id);
        }
    }

    fn clear_pause(&self, job_id: &str) {
        if let Ok(mut paused_jobs) = self.paused_jobs.lock() {
            paused_jobs.remove(job_id);
        }
    }

    fn request_pause(&self, job_id: &str) {
        if let Ok(mut paused_jobs) = self.paused_jobs.lock() {
            paused_jobs.insert(job_id.to_string());
        }
    }

    fn request_resume(&self, job_id: &str) {
        self.clear_pause(job_id);
    }

    fn is_cancelled(&self, job_id: &Option<String>) -> bool {
        let Some(job_id) = job_id else {
            return false;
        };

        self.cancelled_jobs
            .lock()
            .map(|cancelled_jobs| cancelled_jobs.contains(job_id))
            .unwrap_or(false)
    }

    fn is_paused(&self, job_id: &Option<String>) -> bool {
        let Some(job_id) = job_id else {
            return false;
        };

        self.paused_jobs
            .lock()
            .map(|paused_jobs| paused_jobs.contains(job_id))
            .unwrap_or(false)
    }

    fn checkpoint(&self, job_id: &Option<String>, path: Option<&Path>) -> FsResult<()> {
        loop {
            if self.is_cancelled(job_id) {
                return Err(FsError::new(
                    "operation_cancelled",
                    "The file operation was cancelled.",
                    path.map(|path| path.to_string_lossy().into_owned()),
                ));
            }

            if !self.is_paused(job_id) {
                return Ok(());
            }

            thread::sleep(Duration::from_millis(120));
        }
    }

    fn wait_if_paused_or_cancelled(&self, job_id: &Option<String>) -> bool {
        self.checkpoint(job_id, None).is_err()
    }
}

struct OperationStateCleanup {
    operation_state: FileOperationState,
    job_id: Option<String>,
}

impl OperationStateCleanup {
    fn new(operation_state: FileOperationState, job_id: Option<String>) -> Self {
        Self {
            operation_state,
            job_id,
        }
    }
}

impl Drop for OperationStateCleanup {
    fn drop(&mut self) {
        if let Some(job_id) = &self.job_id {
            self.operation_state.clear_cancel(job_id);
            self.operation_state.clear_pause(job_id);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOperationProgress {
    job_id: String,
    operation: String,
    status: String,
    processed_bytes: u64,
    total_bytes: u64,
    processed_entries: u64,
    total_entries: u64,
    current_path: Option<String>,
    current_bytes: u64,
    current_total_bytes: u64,
}

#[tauri::command]
pub async fn list_directory(
    path: String,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<FileEntry>, FsError> {
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
pub async fn list_volumes(
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<VolumeEntry>, FsError> {
    let mut volumes = tauri::async_runtime::spawn_blocking(list_system_volumes)
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Volume scan failed: {error}"),
                None,
            )
        })??;

    volumes.extend(remotes.volume_entries()?);
    sort_volumes(&mut volumes);
    Ok(volumes)
}

#[tauri::command]
pub async fn same_volume(paths: Vec<String>, target_directory: String) -> Result<bool, FsError> {
    let Some(first_path) = paths.first() else {
        return Ok(true);
    };

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
    match (parse_remote_path(&from), parse_remote_path(&to)) {
        (Some(remote_from), Some(remote_to)) => {
            return rename_remote_item(&remotes, remote_from, remote_to).await;
        }
        (Some(remote_from), None) => {
            return Err(cross_provider_error(
                "Renaming from a remote volume to a local path is not implemented yet.",
                &remote_from.volume_id,
                &remote_from.path,
            ));
        }
        (None, Some(remote_to)) => {
            return Err(cross_provider_error(
                "Renaming from a local path to a remote volume is not implemented yet.",
                &remote_to.volume_id,
                &remote_to.path,
            ));
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
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let mut local_paths = Vec::new();

    for path in paths {
        if let Some(remote_path) = parse_remote_path(&path) {
            delete_remote_item(&remotes, remote_path).await?;
        } else {
            local_paths.push(path);
        }
    }

    if local_paths.is_empty() {
        return Ok(());
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
    let mut local_items = Vec::new();
    let total_items = items.len() as u64;
    let mut processed_items = 0_u64;

    for item in items {
        operation_state.checkpoint(&job_id, None)?;

        match (parse_remote_path(&item.from), parse_remote_path(&item.to)) {
            (Some(remote_from), Some(remote_to)) => {
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
            (Some(remote_from), None) => {
                return Err(cross_provider_error(
                    "Copying from a remote volume to a local path is not implemented yet.",
                    &remote_from.volume_id,
                    &remote_from.path,
                ));
            }
            (None, Some(remote_to)) => {
                return Err(cross_provider_error(
                    "Copying from a local path to a remote volume is not implemented yet.",
                    &remote_to.volume_id,
                    &remote_to.path,
                ));
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
                return Err(cross_provider_error(
                    "Moving from a remote volume to a local path is not implemented yet.",
                    &remote_from.volume_id,
                    &remote_from.path,
                ));
            }
            (None, Some(remote_to)) => {
                return Err(cross_provider_error(
                    "Moving from a local path to a remote volume is not implemented yet.",
                    &remote_to.volume_id,
                    &remote_to.path,
                ));
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

#[tauri::command]
pub async fn archive_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    destination: String,
    options: Option<archive::ArchiveOptions>,
    overwrite: bool,
    job_id: Option<String>,
    sudo_password: Option<String>,
) -> Result<(), FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    for path in &paths {
        if let Some(remote_path) = parse_remote_path(path) {
            return Err(cross_provider_error(
                "Creating archives from remote volumes is not implemented yet.",
                &remote_path.volume_id,
                &remote_path.path,
            ));
        }
    }

    if let Some(remote_path) = parse_remote_path(&destination) {
        return Err(cross_provider_error(
            "Creating archives on remote volumes is not implemented yet.",
            &remote_path.volume_id,
            &remote_path.path,
        ));
    }

    let sudo_paths = paths.clone();
    let sudo_destination = destination.clone();
    let options = options.unwrap_or_default();
    let sudo_options = options.clone();
    let operation_state = operation_state.inner().clone();
    let cleanup_operation_state = operation_state.clone();
    let native_app = app.clone();
    let native_job_id = job_id.clone();
    let native_operation_state = operation_state.clone();
    let sudo_app = app.clone();
    let sudo_job_id = job_id.clone();
    let sudo_operation_state = operation_state.clone();
    let result = run_local_with_sudo(
        sudo_password,
        move |_| {
            archive::archive_items_with_progress(
                &paths,
                &destination,
                overwrite,
                &options,
                |progress| {
                    emit_file_operation_progress(
                        &native_app,
                        &native_job_id,
                        "archive",
                        "running",
                        progress,
                    );
                },
                || native_operation_state.wait_if_paused_or_cancelled(&native_job_id),
            )
        },
        move |password| {
            sudo_operation_state.checkpoint(&sudo_job_id, None)?;
            emit_file_operation_status(&sudo_app, &sudo_job_id, "archive", "running");
            sudo::archive_items(
                &password,
                &sudo_paths,
                &sudo_destination,
                overwrite,
                &sudo_options,
            )
        },
    )
    .await;

    if let Some(job_id) = &job_id {
        cleanup_operation_state.clear_cancel(job_id);
        cleanup_operation_state.clear_pause(job_id);
    }

    result
}

#[tauri::command]
pub async fn unarchive_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    destination_directory: String,
    job_id: Option<String>,
    sudo_password: Option<String>,
) -> Result<Vec<String>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    for path in &paths {
        if let Some(remote_path) = parse_remote_path(path) {
            return Err(cross_provider_error(
                "Extracting zip archives from remote volumes is not implemented yet.",
                &remote_path.volume_id,
                &remote_path.path,
            ));
        }
    }

    if let Some(remote_path) = parse_remote_path(&destination_directory) {
        return Err(cross_provider_error(
            "Extracting zip archives to remote volumes is not implemented yet.",
            &remote_path.volume_id,
            &remote_path.path,
        ));
    }

    let sudo_paths = paths.clone();
    let sudo_destination_directory = destination_directory.clone();
    let operation_state = operation_state.inner().clone();
    let cleanup_operation_state = operation_state.clone();
    let native_app = app.clone();
    let native_job_id = job_id.clone();
    let native_operation_state = operation_state.clone();
    let sudo_app = app.clone();
    let sudo_job_id = job_id.clone();
    let sudo_operation_state = operation_state.clone();
    let result = run_local_with_sudo(
        sudo_password,
        move |_| {
            archive::unarchive_items_with_progress(
                &paths,
                &destination_directory,
                |progress| {
                    emit_file_operation_progress(
                        &native_app,
                        &native_job_id,
                        "unarchive",
                        "running",
                        progress,
                    );
                },
                || native_operation_state.wait_if_paused_or_cancelled(&native_job_id),
            )
        },
        move |password| {
            sudo_operation_state.checkpoint(&sudo_job_id, None)?;
            emit_file_operation_status(&sudo_app, &sudo_job_id, "unarchive", "running");
            sudo::unarchive_items(&password, &sudo_paths, &sudo_destination_directory)
        },
    )
    .await;

    if let Some(job_id) = &job_id {
        cleanup_operation_state.clear_cancel(job_id);
        cleanup_operation_state.clear_pause(job_id);
    }

    result
}

#[tauri::command]
pub async fn cancel_file_operation(
    job_id: String,
    operation_state: tauri::State<'_, FileOperationState>,
) -> Result<(), FsError> {
    if job_id.trim().is_empty() {
        return Err(FsError::new(
            "invalid_job_id",
            "Unable to cancel an operation without a job id.",
            None,
        ));
    }

    operation_state.request_cancel(&job_id);
    Ok(())
}

#[tauri::command]
pub async fn pause_file_operation(
    job_id: String,
    operation_state: tauri::State<'_, FileOperationState>,
) -> Result<(), FsError> {
    if job_id.trim().is_empty() {
        return Err(FsError::new(
            "invalid_job_id",
            "Unable to pause an operation without a job id.",
            None,
        ));
    }

    operation_state.request_pause(&job_id);
    Ok(())
}

#[tauri::command]
pub async fn resume_file_operation(
    job_id: String,
    operation_state: tauri::State<'_, FileOperationState>,
) -> Result<(), FsError> {
    if job_id.trim().is_empty() {
        return Err(FsError::new(
            "invalid_job_id",
            "Unable to resume an operation without a job id.",
            None,
        ));
    }

    operation_state.request_resume(&job_id);
    Ok(())
}

#[tauri::command]
pub async fn add_remote_volume(
    config: RemoteVolumeConfig,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<RemoteVolumeInfo, FsError> {
    check_remote(&config).await?;
    remotes.add(config)
}

#[tauri::command]
pub async fn remove_remote_volume(
    id: String,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<bool, FsError> {
    remotes.remove(&id)
}

#[tauri::command]
pub async fn list_remote_volumes(
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<RemoteVolumeInfo>, FsError> {
    remotes.list()
}

#[tauri::command]
pub async fn open_with_default_app(path: String) -> Result<(), FsError> {
    let path = PathBuf::from(path);

    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|error| {
        FsError::new(
            "open_failed",
            format!("Unable to open item with the default app: {error}"),
            Some(path.to_string_lossy().into_owned()),
        )
    })
}

#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> Result<(), FsError> {
    let path = PathBuf::from(path);

    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|error| {
        FsError::new(
            "reveal_failed",
            format!("Unable to reveal item in the file manager: {error}"),
            Some(path.to_string_lossy().into_owned()),
        )
    })
}

#[derive(Debug, Clone, Default)]
struct ProgressSnapshot {
    processed_bytes: u64,
    total_bytes: u64,
    processed_entries: u64,
    total_entries: u64,
    current_path: Option<String>,
    current_bytes: u64,
    current_total_bytes: u64,
}

impl From<archive::ArchiveProgress> for ProgressSnapshot {
    fn from(progress: archive::ArchiveProgress) -> Self {
        Self {
            processed_bytes: progress.processed_bytes,
            total_bytes: progress.total_bytes,
            processed_entries: progress.processed_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path,
            current_bytes: progress.current_bytes,
            current_total_bytes: progress.current_total_bytes,
        }
    }
}

impl From<operations::OperationProgress> for ProgressSnapshot {
    fn from(progress: operations::OperationProgress) -> Self {
        Self {
            processed_bytes: progress.processed_bytes,
            total_bytes: progress.total_bytes,
            processed_entries: progress.processed_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path,
            current_bytes: progress.current_bytes,
            current_total_bytes: progress.current_total_bytes,
        }
    }
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

fn emit_file_operation_progress<P>(
    app: &AppHandle,
    job_id: &Option<String>,
    operation: &str,
    status: &str,
    progress: P,
) where
    P: Into<ProgressSnapshot>,
{
    let Some(job_id) = job_id else {
        return;
    };

    let progress = progress.into();
    let _ = app.emit(
        "file-operation-progress",
        FileOperationProgress {
            job_id: job_id.clone(),
            operation: operation.to_string(),
            status: status.to_string(),
            processed_bytes: progress.processed_bytes,
            total_bytes: progress.total_bytes,
            processed_entries: progress.processed_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path,
            current_bytes: progress.current_bytes,
            current_total_bytes: progress.current_total_bytes,
        },
    );
}

fn emit_transfer_operation_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    operation: &str,
    status: &str,
    progress: operations::OperationProgress,
) {
    emit_file_operation_progress(app, job_id, operation, status, progress);
}

fn emit_file_operation_status(
    app: &AppHandle,
    job_id: &Option<String>,
    operation: &str,
    status: &str,
) {
    emit_file_operation_progress(app, job_id, operation, status, ProgressSnapshot::default());
}

fn cross_provider_error(message: &str, volume_id: &str, path: &str) -> FsError {
    FsError::new(
        "cross_provider_operation",
        message,
        Some(crate::fs::remote::format_remote_uri(volume_id, path)),
    )
}

fn expand_local_path(path: &str) -> FsResult<PathBuf> {
    let trimmed = path.trim();

    if trimmed.is_empty() || trimmed == "~" {
        return LocalFileProvider::home_dir();
    }

    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Ok(LocalFileProvider::home_dir()?.join(rest));
    }

    Ok(PathBuf::from(trimmed))
}

#[cfg(unix)]
fn local_volume_identity(path: &Path) -> FsResult<String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| FsError::io("Unable to read volume metadata", path, error))?;

    Ok(metadata.dev().to_string())
}

#[cfg(windows)]
fn local_volume_identity(path: &Path) -> FsResult<String> {
    use std::path::Component;

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                FsError::new(
                    "volume_lookup_failed",
                    format!("Unable to resolve current directory: {error}"),
                    Some(path.to_string_lossy().into_owned()),
                )
            })?
            .join(path)
    };

    Ok(absolute
        .components()
        .find_map(|component| match component {
            Component::Prefix(prefix) => {
                Some(prefix.as_os_str().to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .unwrap_or_default())
}

#[cfg(not(any(unix, windows)))]
fn local_volume_identity(path: &Path) -> FsResult<String> {
    Ok(path
        .ancestors()
        .last()
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned())
}

async fn run_local_with_sudo<T, F, S>(
    sudo_password: Option<String>,
    task: F,
    sudo_task: S,
) -> FsResult<T>
where
    T: Send + 'static,
    F: FnOnce(LocalFileProvider) -> FsResult<T> + Send + 'static,
    S: FnOnce(String) -> FsResult<T> + Send + 'static,
{
    if let Some(password) = sudo_password {
        return run_sudo(move || sudo_task(password)).await;
    }

    run_local(task).await
}

async fn run_local<T, F>(task: F) -> FsResult<T>
where
    T: Send + 'static,
    F: FnOnce(LocalFileProvider) -> FsResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || task(LocalFileProvider::new()))
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("File task failed: {error}"),
                None,
            )
        })?
}

async fn run_sudo<T, F>(task: F) -> FsResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> FsResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Elevated file task failed: {error}"),
                None,
            )
        })?
}

fn list_system_volumes() -> FsResult<Vec<VolumeEntry>> {
    #[cfg(target_os = "macos")]
    {
        return list_macos_volumes();
    }

    #[cfg(target_os = "linux")]
    {
        return list_linux_volumes();
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(target_os = "macos")]
fn list_macos_volumes() -> FsResult<Vec<VolumeEntry>> {
    let volumes_dir = Path::new("/Volumes");
    let entries = match std::fs::read_dir(volumes_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FsError::io("Read mounted volumes", volumes_dir, error)),
    };

    let root = std::fs::canonicalize("/").ok();
    let mut volumes = Vec::new();

    for entry in entries {
        let entry =
            entry.map_err(|error| FsError::io("Read mounted volume", volumes_dir, error))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        if let (Some(root), Ok(canonical_path)) = (root.as_ref(), std::fs::canonicalize(&path)) {
            if &canonical_path == root {
                continue;
            }
        }

        let name = entry.file_name().to_string_lossy().into_owned();

        if name.is_empty() {
            continue;
        }

        volumes.push(VolumeEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            device_path: None,
            detail: capacity_detail(&path),
            is_removable: true,
            is_mounted: true,
        });
    }

    sort_volumes(&mut volumes);
    Ok(volumes)
}

#[cfg(target_os = "linux")]
fn list_linux_volumes() -> FsResult<Vec<VolumeEntry>> {
    let mountinfo_path = Path::new("/proc/self/mountinfo");
    let mountinfo = match std::fs::read_to_string(mountinfo_path) {
        Ok(mountinfo) => mountinfo,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FsError::io("Read mount table", mountinfo_path, error)),
    };

    let mount_points_by_device = linux_mount_points_by_device(&mountinfo);
    let mut seen_paths = HashSet::new();
    let mut seen_devices = HashSet::new();
    let mut volumes = Vec::new();

    for line in mountinfo.lines() {
        if let Some(volume) = volume_from_mountinfo_line(line) {
            if let Some(device_path) = volume.device_path.as_ref() {
                seen_devices.insert(device_path.clone());
            }

            if seen_paths.insert(volume.path.clone()) {
                volumes.push(volume);
            }
        }
    }

    for volume in attached_linux_block_volumes(&mount_points_by_device)? {
        let Some(device_path) = volume.device_path.as_ref() else {
            continue;
        };

        if seen_devices.insert(device_path.clone()) {
            if volume.path.is_empty() || seen_paths.insert(volume.path.clone()) {
                volumes.push(volume);
            }
        }
    }

    sort_volumes(&mut volumes);
    Ok(volumes)
}

#[cfg(target_os = "linux")]
fn volume_from_mountinfo_line(line: &str) -> Option<VolumeEntry> {
    let fields: Vec<&str> = line.split(' ').collect();
    let separator_index = fields.iter().position(|field| *field == "-")?;
    let mount_point = decode_mountinfo_escape(fields.get(4)?);
    let fs_type = fields.get(separator_index + 1)?;
    let source = decode_mountinfo_escape(fields.get(separator_index + 2).copied().unwrap_or(""));

    if is_pseudo_filesystem(fs_type) {
        return None;
    }

    let path = PathBuf::from(&mount_point);

    if (!is_user_visible_mount(&path) && !is_removable_device_source(&source)) || !path.is_dir() {
        return None;
    }

    Some(VolumeEntry {
        name: mounted_volume_name(&path, &source),
        path: mount_point,
        device_path: source.starts_with("/dev/").then_some(source),
        detail: capacity_detail(&path),
        is_removable: true,
        is_mounted: true,
    })
}

#[cfg(target_os = "linux")]
fn linux_mount_points_by_device(mountinfo: &str) -> HashMap<String, String> {
    let mut mount_points = HashMap::new();

    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(separator_index) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        let Some(mount_point) = fields.get(4) else {
            continue;
        };
        let source =
            decode_mountinfo_escape(fields.get(separator_index + 2).copied().unwrap_or(""));

        if !source.starts_with("/dev/") {
            continue;
        }

        let device_path = strip_mount_source_suffix(&source);
        mount_points.insert(device_path, decode_mountinfo_escape(mount_point));
    }

    mount_points
}

#[cfg(target_os = "linux")]
fn strip_mount_source_suffix(source: &str) -> String {
    source
        .split_once('[')
        .map(|(device, _)| device.to_string())
        .unwrap_or_else(|| source.to_string())
}

#[cfg(target_os = "linux")]
fn attached_linux_block_volumes(
    mount_points_by_device: &HashMap<String, String>,
) -> FsResult<Vec<VolumeEntry>> {
    let block_dir = Path::new("/sys/class/block");
    let entries = match std::fs::read_dir(block_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FsError::io("Read block devices", block_dir, error)),
    };

    let mut volumes = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| FsError::io("Read block device", block_dir, error))?;
        let device_name = entry.file_name().to_string_lossy().into_owned();

        if should_skip_block_device_name(&device_name) || !is_linux_usb_block_device(&device_name) {
            continue;
        }

        let device_path = format!("/dev/{device_name}");
        let properties = read_udev_properties(&device_name);
        let filesystem_type = properties
            .get("ID_FS_TYPE")
            .filter(|value| !value.is_empty());

        if filesystem_type.is_none() && block_device_has_children(&entry.path()) {
            continue;
        }

        if filesystem_type.is_none() && !mount_points_by_device.contains_key(&device_path) {
            continue;
        }

        let mount_path = mount_points_by_device
            .get(&device_path)
            .filter(|path| Path::new(path.as_str()).is_dir())
            .cloned();
        let size = block_device_size(&device_name);
        let detail = if let Some(path) = mount_path.as_ref() {
            capacity_detail(Path::new(path))
                .or_else(|| size.map(|bytes| format!("{} mounted", format_bytes(bytes))))
        } else {
            size.map(|bytes| format!("{} • Not mounted", format_bytes(bytes)))
                .or_else(|| Some("Not mounted".to_string()))
        };

        volumes.push(VolumeEntry {
            name: attached_volume_name(&device_name, &properties),
            path: mount_path.clone().unwrap_or_default(),
            device_path: Some(device_path),
            detail,
            is_removable: true,
            is_mounted: mount_path.is_some(),
        });
    }

    Ok(volumes)
}

#[cfg(target_os = "linux")]
fn should_skip_block_device_name(device_name: &str) -> bool {
    device_name.starts_with("loop")
        || device_name.starts_with("ram")
        || device_name.starts_with("zram")
        || device_name.starts_with("dm-")
}

#[cfg(target_os = "linux")]
fn block_device_has_children(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .map(|name| entry.path().join("partition").exists() || name.starts_with("dm-"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn is_linux_usb_block_device(device_name: &str) -> bool {
    let properties = read_udev_properties(device_name);

    if properties
        .get("ID_BUS")
        .or_else(|| properties.get("ID_USB_TYPE"))
        .map(|value| value == "usb" || value == "disk")
        .unwrap_or(false)
    {
        return true;
    }

    std::fs::canonicalize(Path::new("/sys/class/block").join(device_name))
        .map(|path| path.to_string_lossy().contains("/usb"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn read_udev_properties(device_name: &str) -> HashMap<String, String> {
    let dev_path = Path::new("/sys/class/block").join(device_name).join("dev");
    let Ok(dev_id) = std::fs::read_to_string(dev_path) else {
        return HashMap::new();
    };
    let data_path = Path::new("/run/udev/data").join(format!("b{}", dev_id.trim()));
    let Ok(data) = std::fs::read_to_string(data_path) else {
        return HashMap::new();
    };

    data.lines()
        .filter_map(|line| line.strip_prefix("E:"))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), decode_udev_value(value)))
        .collect()
}

#[cfg(target_os = "linux")]
fn decode_udev_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1] == b'x'
            && bytes[index + 2].is_ascii_hexdigit()
            && bytes[index + 3].is_ascii_hexdigit()
        {
            if let Some(byte) = hex_byte(bytes[index + 2], bytes[index + 3]) {
                decoded.push(byte);
                index += 4;
                continue;
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(target_os = "linux")]
fn hex_byte(left: u8, right: u8) -> Option<u8> {
    Some(hex_nibble(left)? * 16 + hex_nibble(right)?)
}

#[cfg(target_os = "linux")]
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn block_device_size(device_name: &str) -> Option<u64> {
    let size_path = Path::new("/sys/class/block").join(device_name).join("size");
    let sectors = std::fs::read_to_string(size_path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;

    Some(sectors.saturating_mul(512))
}

#[cfg(target_os = "linux")]
fn attached_volume_name(device_name: &str, properties: &HashMap<String, String>) -> String {
    properties
        .get("ID_FS_LABEL")
        .or_else(|| properties.get("ID_PART_ENTRY_NAME"))
        .or_else(|| properties.get("ID_MODEL"))
        .map(|name| readable_device_name(name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| device_name.to_string())
}

#[cfg(target_os = "linux")]
fn readable_device_name(name: &str) -> String {
    name.trim().replace('_', " ")
}

#[cfg(target_os = "linux")]
fn is_user_visible_mount(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_owned))
        .filter(|part| !part.is_empty() && part != "/")
        .collect();

    match parts.as_slice() {
        [run, media, _user, _volume, ..] if run == "run" && media == "media" => true,
        [media, _volume, ..] if media == "media" => true,
        [mnt, _volume, ..] if mnt == "mnt" => true,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn is_pseudo_filesystem(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "autofs"
            | "bpf"
            | "cgroup"
            | "cgroup2"
            | "configfs"
            | "debugfs"
            | "devpts"
            | "devtmpfs"
            | "efivarfs"
            | "fusectl"
            | "mqueue"
            | "overlay"
            | "proc"
            | "pstore"
            | "securityfs"
            | "squashfs"
            | "sysfs"
            | "tmpfs"
            | "tracefs"
    )
}

#[cfg(target_os = "linux")]
fn is_removable_device_source(source: &str) -> bool {
    if !source.starts_with("/dev/") {
        return false;
    }

    let source_path = std::fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source));
    let Some(device_name) = source_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    is_linux_usb_block_device(device_name)
        || block_device_candidates(device_name)
            .iter()
            .any(|candidate| is_removable_block_device(candidate))
}

#[cfg(target_os = "linux")]
fn block_device_candidates(device_name: &str) -> Vec<String> {
    let mut candidates = vec![device_name.to_string()];
    let base = device_name.trim_end_matches(|character: char| character.is_ascii_digit());

    if base != device_name && !base.is_empty() {
        candidates.push(base.trim_end_matches('p').to_string());
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

#[cfg(target_os = "linux")]
fn is_removable_block_device(device_name: &str) -> bool {
    if device_name.is_empty() {
        return false;
    }

    let removable_path = Path::new("/sys/class/block")
        .join(device_name)
        .join("removable");

    std::fs::read_to_string(removable_path)
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn mounted_volume_name(path: &Path, source: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| source.to_string())
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_escape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            if let Some(byte) = octal_byte(&bytes[index + 1..index + 4]) {
                decoded.push(byte);
                index += 4;
                continue;
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(target_os = "linux")]
fn octal_byte(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 3 || !bytes.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
        return None;
    }

    let value = u16::from(bytes[0] - b'0') * 64
        + u16::from(bytes[1] - b'0') * 8
        + u16::from(bytes[2] - b'0');

    u8::try_from(value).ok()
}

fn sort_volumes(volumes: &mut [VolumeEntry]) {
    volumes.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
}

#[cfg(unix)]
fn capacity_detail(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };

    if result != 0 {
        return None;
    }

    let stats = unsafe { stats.assume_init() };
    let block_size = if stats.f_frsize > 0 {
        stats.f_frsize
    } else {
        stats.f_bsize
    } as u64;
    let available = (stats.f_bavail as u64).saturating_mul(block_size);

    Some(format!("{} available", format_bytes(available)))
}

#[cfg(not(unix))]
fn capacity_detail(_path: &Path) -> Option<String> {
    None
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} bytes");
    }

    let mut value = bytes as f64;
    let mut unit_index = 0;

    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}
