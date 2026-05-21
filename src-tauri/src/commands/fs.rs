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
use crate::open_with::{self, OpenWithContext};
use crate::store::AppStoreState;
use ignore::WalkBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rand::distr::{Alphanumeric, SampleString};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const MEDIA_PREVIEW_MAX_BYTES: u64 = 128 * 1024 * 1024;
const MEDIA_STREAM_MAX_ENTRIES: usize = 256;

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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchOptions {
    #[serde(default = "default_file_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default = "default_true")]
    pub respect_ignore: bool,
    #[serde(default = "default_true")]
    pub include_files: bool,
    #[serde(default = "default_true")]
    pub include_directories: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResult {
    pub name: String,
    pub path: String,
    pub parent_path: String,
    pub kind: String,
    pub score: i64,
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearchOptions {
    #[serde(default = "default_content_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default = "default_true")]
    pub respect_ignore: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default = "default_content_search_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearchResult {
    pub name: String,
    pub path: String,
    pub parent_path: String,
    pub line_number: usize,
    pub line_text: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
    pub bytes_read: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChecksumComparison {
    pub algorithm: String,
    pub left_path: String,
    pub right_path: String,
    pub left_hash: String,
    pub right_hash: String,
    pub left_bytes: u64,
    pub right_bytes: u64,
    pub equal: bool,
}

fn default_true() -> bool {
    true
}

fn default_file_search_limit() -> usize {
    80
}

fn default_content_search_limit() -> usize {
    120
}

fn default_content_search_max_file_bytes() -> u64 {
    2 * 1024 * 1024
}

#[derive(Clone, Default)]
pub struct FileOperationState {
    cancelled_jobs: Arc<Mutex<HashSet<String>>>,
    paused_jobs: Arc<Mutex<HashSet<String>>>,
}

#[derive(Clone)]
struct MediaStreamServer {
    port: u16,
    token: String,
}

#[derive(Clone, Default)]
pub struct MediaStreamState {
    server: Arc<Mutex<Option<MediaStreamServer>>>,
    entries: Arc<Mutex<HashMap<String, PathBuf>>>,
    entry_order: Arc<Mutex<VecDeque<String>>>,
}

impl MediaStreamState {
    fn stream_url_for(&self, path: PathBuf) -> FsResult<String> {
        let metadata = fs::metadata(&path)
            .map_err(|error| FsError::io("Unable to read media metadata", &path, error))?;

        if !metadata.is_file() {
            return Err(FsError::new(
                "media_stream_not_file",
                "Media preview is available for files only.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let server = self.ensure_server()?;
        let id = random_token(32);

        self.entries
            .lock()
            .map_err(|_| media_stream_error("Unable to register media stream."))?
            .insert(id.clone(), path.clone());
        self.prune_entries(&id)?;

        Ok(format!(
            "http://127.0.0.1:{}/media/{}/{}/preview{}",
            server.port,
            server.token,
            id,
            media_url_extension(&path),
        ))
    }

    fn ensure_server(&self) -> FsResult<MediaStreamServer> {
        let mut server = self
            .server
            .lock()
            .map_err(|_| media_stream_error("Unable to start media stream server."))?;

        if let Some(server) = server.clone() {
            return Ok(server);
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
            FsError::new(
                "media_stream_server_unavailable",
                format!("Unable to start media stream server: {error}"),
                None,
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                FsError::new(
                    "media_stream_server_unavailable",
                    format!("Unable to read media stream server address: {error}"),
                    None,
                )
            })?
            .port();
        let token = random_token(32);
        let entries = self.entries.clone();
        let server_token = token.clone();

        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let entries = entries.clone();
                let token = server_token.clone();

                thread::spawn(move || {
                    handle_media_stream_request(stream, entries, token);
                });
            }
        });

        let created = MediaStreamServer { port, token };
        *server = Some(created.clone());
        Ok(created)
    }

    fn prune_entries(&self, id: &str) -> FsResult<()> {
        let mut order = self
            .entry_order
            .lock()
            .map_err(|_| media_stream_error("Unable to prune media stream entries."))?;

        order.push_back(id.to_string());

        while order.len() > MEDIA_STREAM_MAX_ENTRIES {
            if let Some(expired_id) = order.pop_front() {
                if let Ok(mut entries) = self.entries.lock() {
                    entries.remove(&expired_id);
                }
            }
        }

        Ok(())
    }
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

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeMeasureResult {
    pub logical_bytes: u64,
    pub disk_bytes: u64,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub skipped: u64,
}

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
pub async fn search_files(
    root: String,
    query: String,
    options: Option<FileSearchOptions>,
) -> Result<Vec<FileSearchResult>, FsError> {
    run_local(move |_| {
        search_local_files(
            &root,
            &query,
            options.unwrap_or_else(default_search_options),
        )
    })
    .await
}

#[tauri::command]
pub async fn search_content(
    root: String,
    query: String,
    options: Option<ContentSearchOptions>,
) -> Result<Vec<ContentSearchResult>, FsError> {
    run_local(move |_| {
        search_local_content(
            &root,
            &query,
            options.unwrap_or_else(default_content_search_options),
        )
    })
    .await
}

#[tauri::command]
pub async fn get_home_directory() -> Result<String, FsError> {
    LocalFileProvider::home_dir().map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn read_text_preview(
    path: String,
    max_bytes: Option<usize>,
) -> Result<TextPreview, FsError> {
    run_local(move |_| read_local_text_preview(&path, max_bytes.unwrap_or(96 * 1024))).await
}

#[tauri::command]
pub async fn read_media_preview(
    path: String,
    max_bytes: Option<u64>,
) -> Result<tauri::ipc::Response, FsError> {
    let bytes = run_local(move |_| {
        read_local_media_preview(&path, max_bytes.unwrap_or(MEDIA_PREVIEW_MAX_BYTES))
    })
    .await?;

    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn create_media_stream_url(
    path: String,
    media_state: tauri::State<'_, MediaStreamState>,
) -> Result<String, FsError> {
    if parse_remote_path(&path).is_some() {
        return Err(FsError::new(
            "media_stream_remote_unsupported",
            "Remote media preview is not supported yet.",
            Some(path),
        ));
    }

    let path = expand_local_path(&path)?;
    media_state.stream_url_for(path)
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
pub async fn mount_volume(device_path: String) -> Result<VolumeEntry, FsError> {
    let error_path = device_path.clone();

    tauri::async_runtime::spawn_blocking(move || mount_system_volume(&device_path))
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Volume mount failed: {error}"),
                Some(error_path),
            )
        })?
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
pub async fn compare_file_checksums(
    left_path: String,
    right_path: String,
) -> Result<FileChecksumComparison, FsError> {
    if archive::is_archive_uri(&left_path)
        || archive::is_archive_uri(&right_path)
        || parse_remote_path(&left_path).is_some()
        || parse_remote_path(&right_path).is_some()
    {
        return Err(FsError::new(
            "checksum_unsupported",
            "Checksum comparison is available for local files only.",
            None,
        ));
    }

    run_local(move |_| compare_local_file_checksums(&left_path, &right_path)).await
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
        if archive::is_archive_uri(&path) {
            return Err(archive_read_only_error(&path));
        } else if let Some(remote_path) = parse_remote_path(&path) {
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
                return Err(cross_provider_error(
                    "Copying from a remote volume to a local path is not implemented yet.",
                    &remote_from.volume_id,
                    &remote_from.path,
                ));
            }
            (None, None, None, Some(remote_to)) => {
                return Err(cross_provider_error(
                    "Copying from a local path to a remote volume is not implemented yet.",
                    &remote_to.volume_id,
                    &remote_to.path,
                ));
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
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }

        if let Some(remote_path) = parse_remote_path(path) {
            return Err(cross_provider_error(
                "Creating archives from remote volumes is not implemented yet.",
                &remote_path.volume_id,
                &remote_path.path,
            ));
        }
    }

    if archive::is_archive_uri(&destination) {
        return Err(archive_read_only_error(&destination));
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
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }

        if let Some(remote_path) = parse_remote_path(path) {
            return Err(cross_provider_error(
                "Extracting zip archives from remote volumes is not implemented yet.",
                &remote_path.volume_id,
                &remote_path.path,
            ));
        }
    }

    if archive::is_archive_uri(&destination_directory) {
        return Err(archive_read_only_error(&destination_directory));
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
pub async fn measure_items_size(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    job_id: Option<String>,
) -> Result<SizeMeasureResult, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if paths.is_empty() {
        return Ok(SizeMeasureResult::default());
    }

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(FsError::new(
                "unsupported_size_measure",
                "Folder size measurement is available for local files and folders only.",
                Some(path.clone()),
            ));
        }

        if let Some(remote_path) = parse_remote_path(path) {
            return Err(cross_provider_error(
                "Measuring remote folder sizes is not implemented yet.",
                &remote_path.volume_id,
                &remote_path.path,
            ));
        }
    }

    let local_paths = paths
        .iter()
        .map(|path| expand_local_path(path))
        .collect::<FsResult<Vec<_>>>()?;

    let measure_app = app.clone();
    let measure_job_id = job_id.clone();
    let measure_operation_state = operation_state.inner().clone();

    run_local(move |_| {
        measure_local_items_size(
            &measure_app,
            &measure_operation_state,
            &measure_job_id,
            local_paths,
        )
    })
    .await
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
pub async fn open_with_default_app(
    path: String,
    store: tauri::State<'_, AppStoreState>,
) -> Result<(), FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;

    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        let materialized_path =
            run_local(move |_| archive::materialize_archive_file(&archive_path)).await?;
        return open_with::open_with_default(&materialized_path, remembered);
    }

    open_with::open_with_default(&PathBuf::from(path), remembered)
}

#[tauri::command]
pub async fn list_open_with_apps(
    path: String,
    store: tauri::State<'_, AppStoreState>,
) -> Result<OpenWithContext, FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;

    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        let materialized_path =
            run_local(move |_| archive::materialize_archive_file(&archive_path)).await?;
        return open_with::open_with_context(&materialized_path, remembered);
    }

    open_with::open_with_context(&PathBuf::from(path), remembered)
}

#[tauri::command]
pub async fn open_with_app(
    path: String,
    app_id: String,
    remember: bool,
    store: tauri::State<'_, AppStoreState>,
) -> Result<(), FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;
    let materialized_path = if let Some(archive_path) = archive::parse_archive_uri(&path) {
        run_local(move |_| archive::materialize_archive_file(&archive_path)).await?
    } else {
        PathBuf::from(&path)
    };
    let context = open_with::open_with_context(&materialized_path, remembered)?;
    let Some(app) = context.apps.iter().find(|app| app.id == app_id) else {
        return Err(FsError::new(
            "open_with_app_not_found",
            "The selected app is no longer available.",
            Some(path),
        ));
    };
    let app_name = app.name.clone();

    open_with::open_with_app_id(&materialized_path, &app_id)?;

    if remember {
        store.save_open_with_default(&file_type.key, &app_id, &app_name)?;
    } else {
        store.clear_open_with_default(&file_type.key)?;
    }

    Ok(())
}

#[tauri::command]
pub async fn run_custom_tool(
    command: String,
    paths: Vec<String>,
    cwd: Option<String>,
) -> Result<(), FsError> {
    run_local(move |_| run_local_custom_tool(&command, &paths, cwd.as_deref())).await
}

#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> Result<(), FsError> {
    let path = archive::parse_archive_uri(&path)
        .map(|archive_path| archive_path.archive_path)
        .unwrap_or_else(|| PathBuf::from(path));

    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|error| {
        FsError::new(
            "reveal_failed",
            format!("Unable to reveal item in the file manager: {error}"),
            Some(path.to_string_lossy().into_owned()),
        )
    })
}

#[derive(Debug, Default)]
struct SizeMeasureAccumulator {
    logical_bytes: u64,
    disk_bytes: u64,
    files: u64,
    directories: u64,
    symlinks: u64,
    skipped: u64,
    processed_entries: u64,
}

impl From<SizeMeasureAccumulator> for SizeMeasureResult {
    fn from(value: SizeMeasureAccumulator) -> Self {
        Self {
            logical_bytes: value.logical_bytes,
            disk_bytes: value.disk_bytes,
            files: value.files,
            directories: value.directories,
            symlinks: value.symlinks,
            skipped: value.skipped,
        }
    }
}

fn measure_local_items_size(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    roots: Vec<PathBuf>,
) -> FsResult<SizeMeasureResult> {
    let mut accumulator = SizeMeasureAccumulator::default();
    let mut stack = roots.into_iter().rev().collect::<Vec<_>>();
    #[cfg(unix)]
    let mut seen_directories = HashSet::new();
    #[cfg(not(unix))]
    let mut seen_directories = ();
    #[cfg(unix)]
    let mut seen_regular_files = HashSet::new();
    #[cfg(not(unix))]
    let mut seen_regular_files = ();

    emit_size_measure_progress(app, job_id, &accumulator, None);

    while let Some(path) = stack.pop() {
        operation_state.checkpoint(job_id, Some(&path))?;

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                accumulator.skipped = accumulator.skipped.saturating_add(1);
                accumulator.processed_entries = accumulator.processed_entries.saturating_add(1);
                maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
                continue;
            }
        };

        accumulator.processed_entries = accumulator.processed_entries.saturating_add(1);
        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            accumulator.symlinks = accumulator.symlinks.saturating_add(1);
            accumulator.logical_bytes = accumulator.logical_bytes.saturating_add(metadata.len());
            accumulator.disk_bytes = accumulator
                .disk_bytes
                .saturating_add(metadata_disk_usage_bytes(&metadata));
            maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
            continue;
        }

        if metadata.is_file() {
            accumulator.files = accumulator.files.saturating_add(1);

            if should_count_regular_file(&metadata, &mut seen_regular_files) {
                accumulator.logical_bytes =
                    accumulator.logical_bytes.saturating_add(metadata.len());
                accumulator.disk_bytes = accumulator
                    .disk_bytes
                    .saturating_add(metadata_disk_usage_bytes(&metadata));
            }

            maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
            continue;
        }

        if metadata.is_dir() {
            if !should_count_directory(&metadata, &mut seen_directories) {
                maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
                continue;
            }

            accumulator.directories = accumulator.directories.saturating_add(1);
            accumulator.disk_bytes = accumulator
                .disk_bytes
                .saturating_add(metadata_disk_usage_bytes(&metadata));

            match fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        operation_state.checkpoint(job_id, Some(&path))?;

                        match entry {
                            Ok(entry) => stack.push(entry.path()),
                            Err(_) => {
                                accumulator.skipped = accumulator.skipped.saturating_add(1);
                            }
                        }
                    }
                }
                Err(_) => {
                    accumulator.skipped = accumulator.skipped.saturating_add(1);
                }
            }

            maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
            continue;
        }

        accumulator.files = accumulator.files.saturating_add(1);
        accumulator.logical_bytes = accumulator.logical_bytes.saturating_add(metadata.len());
        accumulator.disk_bytes = accumulator
            .disk_bytes
            .saturating_add(metadata_disk_usage_bytes(&metadata));
        maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
    }

    emit_size_measure_progress(app, job_id, &accumulator, None);
    Ok(accumulator.into())
}

fn maybe_emit_size_measure_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    accumulator: &SizeMeasureAccumulator,
    current_path: &Path,
) {
    if accumulator.processed_entries % 128 != 0 {
        return;
    }

    emit_size_measure_progress(app, job_id, accumulator, Some(current_path));
}

fn emit_size_measure_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    accumulator: &SizeMeasureAccumulator,
    current_path: Option<&Path>,
) {
    emit_file_operation_progress(
        app,
        job_id,
        "measure",
        "running",
        ProgressSnapshot {
            processed_bytes: accumulator.logical_bytes,
            processed_entries: accumulator.processed_entries,
            current_path: current_path.map(|path| path.to_string_lossy().into_owned()),
            ..ProgressSnapshot::default()
        },
    );
}

#[cfg(unix)]
fn should_count_directory(
    metadata: &fs::Metadata,
    seen_directories: &mut HashSet<(u64, u64)>,
) -> bool {
    seen_directories.insert((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn should_count_directory(metadata: &fs::Metadata, seen_directories: &mut ()) -> bool {
    let _ = (metadata, seen_directories);
    true
}

#[cfg(unix)]
fn should_count_regular_file(
    metadata: &fs::Metadata,
    seen_regular_files: &mut HashSet<(u64, u64)>,
) -> bool {
    metadata.nlink() <= 1 || seen_regular_files.insert((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn should_count_regular_file(metadata: &fs::Metadata, seen_regular_files: &mut ()) -> bool {
    let _ = (metadata, seen_regular_files);
    true
}

#[cfg(unix)]
fn metadata_disk_usage_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn metadata_disk_usage_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
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

fn archive_read_only_error(path: &str) -> FsError {
    FsError::new(
        "archive_read_only",
        "Archive browsing is read-only. Copy items out of the archive instead.",
        Some(path.to_string()),
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

fn default_search_options() -> FileSearchOptions {
    FileSearchOptions {
        limit: default_file_search_limit(),
        include_hidden: false,
        respect_ignore: true,
        include_files: true,
        include_directories: true,
        follow_symlinks: false,
        max_depth: None,
    }
}

fn default_content_search_options() -> ContentSearchOptions {
    ContentSearchOptions {
        limit: default_content_search_limit(),
        include_hidden: false,
        respect_ignore: true,
        case_sensitive: false,
        regex: false,
        max_file_bytes: default_content_search_max_file_bytes(),
        max_depth: None,
    }
}

fn expand_local_search_root(root: &str) -> FsResult<PathBuf> {
    let trimmed = root.trim();

    if trimmed.is_empty() || trimmed == "~" {
        return LocalFileProvider::home_dir();
    }

    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Ok(LocalFileProvider::home_dir()?.join(rest));
    }

    if archive::is_archive_uri(trimmed) || parse_remote_path(trimmed).is_some() {
        return Err(FsError::new(
            "unsupported_search_root",
            "Fuzzy file search currently supports local folders only.",
            Some(trimmed.to_string()),
        ));
    }

    Ok(PathBuf::from(trimmed))
}

fn configure_walk_builder(
    root_path: &Path,
    include_hidden: bool,
    respect_ignore: bool,
    follow_symlinks: bool,
    max_depth: Option<usize>,
) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root_path);
    builder
        .hidden(!include_hidden)
        .follow_links(follow_symlinks)
        .min_depth(Some(1));

    if let Some(max_depth) = max_depth {
        builder.max_depth(Some(max_depth.max(1)));
    }

    if !respect_ignore {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }

    builder
}

fn search_result_kind(metadata: &fs::Metadata, is_symlink: bool) -> &'static str {
    if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else if is_symlink {
        "symlink"
    } else {
        "other"
    }
}

fn search_local_files(
    root: &str,
    query: &str,
    options: FileSearchOptions,
) -> FsResult<Vec<FileSearchResult>> {
    let root_path = expand_local_search_root(root)?;
    let root_metadata = fs::metadata(&root_path)
        .map_err(|error| FsError::io("Unable to read search root", &root_path, error))?;

    if !root_metadata.is_dir() {
        return Err(FsError::new(
            "search_root_not_directory",
            "Search root must be a local folder.",
            Some(root_path.to_string_lossy().into_owned()),
        ));
    }

    let limit = options.limit.clamp(1, 500);
    let query = query.trim();
    let builder = configure_walk_builder(
        &root_path,
        options.include_hidden,
        options.respect_ignore,
        options.follow_symlinks,
        options.max_depth,
    );

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut results = Vec::new();
    let mut haystack_buf = Vec::new();

    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();

        if path == root_path {
            continue;
        }

        let symlink_metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let is_symlink = symlink_metadata.file_type().is_symlink();
        let metadata = if is_symlink {
            fs::metadata(path).unwrap_or(symlink_metadata)
        } else {
            symlink_metadata
        };
        let kind = search_result_kind(&metadata, is_symlink);

        if (kind == "directory" && !options.include_directories)
            || (kind != "directory" && !options.include_files)
        {
            continue;
        }

        let candidate = path
            .strip_prefix(&root_path)
            .unwrap_or(path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let score = if query.is_empty() {
            0
        } else if let Some(score) = pattern.score(
            Utf32Str::new(candidate.as_str(), &mut haystack_buf),
            &mut matcher,
        ) {
            score
        } else {
            continue;
        };
        let name = path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy()
            .into_owned();
        let parent_path = path
            .parent()
            .unwrap_or(&root_path)
            .to_string_lossy()
            .into_owned();

        results.push(FileSearchResult {
            name,
            path: path.to_string_lossy().into_owned(),
            parent_path,
            kind: kind.to_string(),
            score: i64::from(score),
            size: metadata.is_file().then_some(metadata.len()),
            modified_at: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs()),
        });
    }

    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    results.truncate(limit);
    Ok(results)
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

fn read_local_text_preview(path: &str, max_bytes: usize) -> FsResult<TextPreview> {
    let path = expand_local_search_root(path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| FsError::io("Unable to read text preview metadata", &path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "preview_not_file",
            "Text preview is available for files only.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let byte_limit = max_bytes.clamp(4 * 1024, 512 * 1024);
    let bytes = fs::read(&path)
        .map_err(|error| FsError::io("Unable to read text preview", &path, error))?;
    let truncated = bytes.len() > byte_limit;
    let bytes = &bytes[..bytes.len().min(byte_limit)];

    if is_probably_binary(bytes) {
        return Err(FsError::new(
            "preview_binary_file",
            "This file appears to be binary.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(TextPreview {
        text: String::from_utf8_lossy(bytes).into_owned(),
        truncated,
        bytes_read: bytes.len(),
    })
}

fn compare_local_file_checksums(
    left_path: &str,
    right_path: &str,
) -> FsResult<FileChecksumComparison> {
    let left = expand_local_path(left_path)?;
    let right = expand_local_path(right_path)?;
    let (left_hash, left_bytes) = file_sha256(&left)?;
    let (right_hash, right_bytes) = file_sha256(&right)?;

    Ok(FileChecksumComparison {
        algorithm: "SHA-256".to_string(),
        left_path: left.to_string_lossy().into_owned(),
        right_path: right.to_string_lossy().into_owned(),
        equal: left_hash == right_hash,
        left_hash,
        right_hash,
        left_bytes,
        right_bytes,
    })
}

fn file_sha256(path: &Path) -> FsResult<(String, u64)> {
    let metadata = fs::metadata(path)
        .map_err(|error| FsError::io("Unable to read checksum metadata", path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "checksum_not_file",
            "Checksum comparison is available for files only.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let mut file = fs::File::open(path)
        .map_err(|error| FsError::io("Unable to read file checksum", path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes_read = 0_u64;

    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| FsError::io("Unable to read file checksum", path, error))?;

        if count == 0 {
            break;
        }

        hasher.update(&buffer[..count]);
        bytes_read = bytes_read.saturating_add(count as u64);
    }

    Ok((hex_string(&hasher.finalize()), bytes_read))
}

fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }

    output
}

fn read_local_media_preview(path: &str, max_bytes: u64) -> FsResult<Vec<u8>> {
    let path = expand_local_path(path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| FsError::io("Unable to read media preview metadata", &path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "preview_not_file",
            "Media preview is available for files only.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let byte_limit = max_bytes.clamp(1024 * 1024, MEDIA_PREVIEW_MAX_BYTES);

    if metadata.len() > byte_limit {
        return Err(FsError::new(
            "preview_file_too_large",
            "This media file is too large for inline preview.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    fs::read(&path).map_err(|error| FsError::io("Unable to read media preview", &path, error))
}

struct MediaStreamRequest {
    method: String,
    path: String,
    range: Option<String>,
}

fn handle_media_stream_request(
    mut stream: TcpStream,
    entries: Arc<Mutex<HashMap<String, PathBuf>>>,
    token: String,
) {
    let request = match read_media_stream_request(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_media_stream_error(&mut stream, "400 Bad Request", "Bad request");
            return;
        }
    };

    if request.method == "OPTIONS" {
        let _ = write_media_stream_options_response(&mut stream);
        return;
    }

    if request.method != "GET" && request.method != "HEAD" {
        let _ =
            write_media_stream_error(&mut stream, "405 Method Not Allowed", "Method not allowed");
        return;
    }

    let prefix = format!("/media/{token}/");
    let Some(rest) = request.path.strip_prefix(&prefix) else {
        let _ = write_media_stream_error(&mut stream, "404 Not Found", "Not found");
        return;
    };
    let id = rest.split('/').next().unwrap_or_default();

    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        let _ = write_media_stream_error(&mut stream, "404 Not Found", "Not found");
        return;
    }

    let path = match entries
        .lock()
        .ok()
        .and_then(|entries| entries.get(id).cloned())
    {
        Some(path) => path,
        None => {
            let _ = write_media_stream_error(&mut stream, "404 Not Found", "Not found");
            return;
        }
    };

    if let Err(error) = write_media_stream_file(&mut stream, &request, &path) {
        eprintln!("Unable to stream media preview {}: {error}", path.display());
    }
}

fn read_media_stream_request(stream: &mut TcpStream) -> std::io::Result<MediaStreamRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];

    loop {
        let bytes_read = stream.read(&mut chunk)?;

        if bytes_read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);

        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 64 * 1024 {
            break;
        }
    }

    let request = String::from_utf8_lossy(&buffer);
    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let target = request_parts.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default().to_string();
    let mut range = None;

    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };

        if name.eq_ignore_ascii_case("range") {
            range = Some(value.trim().to_string());
        }
    }

    if method.is_empty() || path.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid media stream request",
        ));
    }

    Ok(MediaStreamRequest {
        method,
        path,
        range,
    })
}

fn write_media_stream_file(
    stream: &mut TcpStream,
    request: &MediaStreamRequest,
    path: &Path,
) -> std::io::Result<()> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let content_type = media_content_type(path);

    if len == 0 {
        write_media_stream_headers(stream, "200 OK", content_type, 0, None)?;
        return Ok(());
    }

    let range = request
        .range
        .as_deref()
        .and_then(|range| parse_media_range(range, len));

    let (status, start, end, content_range) = if let Some((start, end)) = range {
        (
            "206 Partial Content",
            start,
            end,
            Some(format!("bytes {start}-{end}/{len}")),
        )
    } else if request.range.is_some() {
        write_media_stream_range_error(stream, len)?;
        return Ok(());
    } else {
        ("200 OK", 0, len - 1, None)
    };
    let content_length = end - start + 1;

    write_media_stream_headers(
        stream,
        status,
        content_type,
        content_length,
        content_range.as_deref(),
    )?;

    if request.method == "HEAD" {
        return Ok(());
    }

    file.seek(SeekFrom::Start(start))?;
    stream_file_range(stream, &mut file, content_length)
}

fn parse_media_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let (unit, spec) = header.trim().split_once('=')?;

    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }

    let first_range = spec.split(',').next()?.trim();
    let (start, end) = first_range.split_once('-')?;

    if start.is_empty() {
        let suffix_length = end.parse::<u64>().ok()?;

        if suffix_length == 0 {
            return None;
        }

        let start = len.saturating_sub(suffix_length);
        return Some((start, len - 1));
    }

    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().ok()?.min(len - 1)
    };

    if start >= len || end < start {
        return None;
    }

    Some((start, end))
}

fn stream_file_range(
    stream: &mut TcpStream,
    file: &mut fs::File,
    mut remaining: u64,
) -> std::io::Result<()> {
    let mut buffer = [0_u8; 64 * 1024];

    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let bytes_read = file.read(&mut buffer[..limit])?;

        if bytes_read == 0 {
            break;
        }

        if let Err(error) = stream.write_all(&buffer[..bytes_read]) {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }

            return Err(error);
        }

        remaining -= bytes_read as u64;
    }

    Ok(())
}

fn write_media_stream_headers(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    content_length: u64,
    content_range: Option<&str>,
) -> std::io::Result<()> {
    stream.write_all(
        media_stream_header_response(status, content_type, content_length, content_range)
            .as_bytes(),
    )
}

fn media_stream_header_response(
    status: &str,
    content_type: &str,
    content_length: u64,
    content_range: Option<&str>,
) -> String {
    let content_range_header = content_range
        .map(|value| format!("Content-Range: {value}\r\n"))
        .unwrap_or_default();

    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {content_length}\r\n\
         Accept-Ranges: bytes\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Expose-Headers: Accept-Ranges, Content-Length, Content-Range\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         {content_range_header}\r\n",
    )
}

fn write_media_stream_range_error(stream: &mut TcpStream, len: u64) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 416 Range Not Satisfiable\r\n\
         Content-Length: 0\r\n\
         Content-Range: bytes */{len}\r\n\
         Accept-Ranges: bytes\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
    );

    stream.write_all(response.as_bytes())
}

fn write_media_stream_options_response(stream: &mut TcpStream) -> std::io::Result<()> {
    let response = concat!(
        "HTTP/1.1 204 No Content\r\n",
        "Content-Length: 0\r\n",
        "Access-Control-Allow-Origin: *\r\n",
        "Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n",
        "Access-Control-Allow-Headers: Range\r\n",
        "Access-Control-Max-Age: 86400\r\n",
        "Connection: close\r\n\r\n",
    );

    stream.write_all(response.as_bytes())
}

fn write_media_stream_error(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n\
         {body}",
        body.len(),
    );

    stream.write_all(response.as_bytes())
}

fn media_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("ogv") => "video/ogg",
        Some("mpeg" | "mpg") => "video/mpeg",
        Some("3gp") => "video/3gpp",
        Some("3g2") => "video/3gpp2",
        Some("avi") => "video/x-msvideo",
        Some("mkv") => "video/x-matroska",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "alac") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("oga" | "ogg" | "opus") => "audio/ogg",
        Some("weba") => "audio/webm",
        Some("aif" | "aiff") => "audio/aiff",
        Some("wma") => "audio/x-ms-wma",
        _ => "application/octet-stream",
    }
}

fn media_url_extension(path: &Path) -> String {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.to_ascii_lowercase())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 12
                && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
        })
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default()
}

fn media_stream_error(message: &str) -> FsError {
    FsError::new("media_stream_error", message, None)
}

fn random_token(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), len)
}

fn run_local_custom_tool(command: &str, paths: &[String], cwd: Option<&str>) -> FsResult<()> {
    let command = command.trim();

    if command.is_empty() {
        return Err(FsError::new(
            "custom_tool_empty_command",
            "Custom tool command is empty.",
            None,
        ));
    }

    if paths.is_empty() {
        return Err(FsError::new(
            "custom_tool_no_paths",
            "Choose at least one local item before running a custom tool.",
            None,
        ));
    }

    let local_paths = paths
        .iter()
        .map(|path| expand_custom_tool_path(path))
        .collect::<FsResult<Vec<_>>>()?;
    let path_values = local_paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let first_path = local_paths.first().expect("paths checked above");
    let first_path_value = path_values.first().cloned().unwrap_or_default();
    let first_name = first_path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();
    let parent_path = custom_tool_cwd(cwd, first_path)?;
    let parent_value = parent_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tokens = split_custom_tool_command(command)?;
    let mut expanded = expand_custom_tool_tokens(
        &tokens,
        &path_values,
        &first_path_value,
        &first_name,
        &parent_value,
    );

    if !tokens_have_custom_tool_placeholders(&tokens) {
        expanded.extend(path_values.iter().cloned());
    }

    let Some(program) = expanded
        .first()
        .cloned()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(FsError::new(
            "custom_tool_empty_command",
            "Custom tool command is empty.",
            Some(first_path_value),
        ));
    };
    let args = expanded.drain(1..).collect::<Vec<_>>();
    let mut child = Command::new(&program);
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(parent_path) = parent_path.filter(|path| path.is_dir()) {
        child.current_dir(parent_path);
    }

    child.spawn().map_err(|error| {
        FsError::new(
            "custom_tool_spawn_failed",
            format!("Unable to run custom tool: {error}"),
            Some(first_path_value),
        )
    })?;

    Ok(())
}

fn expand_custom_tool_path(path: &str) -> FsResult<PathBuf> {
    if archive::is_archive_uri(path) || parse_remote_path(path).is_some() {
        return Err(FsError::new(
            "custom_tool_unsupported_path",
            "Custom tools can run on local files and folders only.",
            Some(path.to_string()),
        ));
    }

    expand_local_path(path)
}

fn custom_tool_cwd(cwd: Option<&str>, first_path: &Path) -> FsResult<Option<PathBuf>> {
    if let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) {
        if archive::is_archive_uri(cwd) || parse_remote_path(cwd).is_some() {
            return Err(FsError::new(
                "custom_tool_unsupported_path",
                "Custom tools can run from local folders only.",
                Some(cwd.to_string()),
            ));
        }

        return expand_local_path(cwd).map(Some);
    }

    Ok(first_path.parent().map(Path::to_path_buf))
}

fn split_custom_tool_command(command: &str) -> FsResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut token_started = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(quote_char) = quote {
            if ch == quote_char {
                quote = None;
            } else if ch == '\\' && quote_char == '"' {
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    current.push(ch);
                }
            } else {
                current.push(ch);
            }
            token_started = true;
            continue;
        }

        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                token_started = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    current.push(ch);
                }
                token_started = true;
            }
            ch if ch.is_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            ch => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if let Some(quote_char) = quote {
        return Err(FsError::new(
            "custom_tool_invalid_command",
            format!("Custom tool command has an unclosed {quote_char} quote."),
            None,
        ));
    }

    if token_started {
        tokens.push(current);
    }

    if tokens.is_empty() {
        return Err(FsError::new(
            "custom_tool_empty_command",
            "Custom tool command is empty.",
            None,
        ));
    }

    Ok(tokens)
}

fn tokens_have_custom_tool_placeholders(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        token.contains("%path%")
            || token.contains("%paths%")
            || token.contains("%name%")
            || token.contains("%parent%")
    })
}

fn expand_custom_tool_tokens(
    tokens: &[String],
    paths: &[String],
    first_path: &str,
    first_name: &str,
    parent_path: &str,
) -> Vec<String> {
    let joined_paths = paths.join(" ");
    let mut expanded = Vec::new();

    for token in tokens {
        if token == "%paths%" {
            expanded.extend(paths.iter().cloned());
            continue;
        }

        expanded.push(
            token
                .replace("%path%", first_path)
                .replace("%paths%", &joined_paths)
                .replace("%name%", first_name)
                .replace("%parent%", parent_path),
        );
    }

    expanded
}

fn line_text_with_limit(line: &str) -> String {
    const MAX_CHARS: usize = 500;

    if line.chars().count() <= MAX_CHARS {
        return line.to_string();
    }

    line.chars().take(MAX_CHARS).collect::<String>()
}

fn find_plain_match(line: &str, query: &str, case_sensitive: bool) -> Option<(usize, usize)> {
    if case_sensitive {
        return line.find(query).map(|start| (start, start + query.len()));
    }

    let line_lower = line.to_lowercase();
    let query_lower = query.to_lowercase();
    line_lower
        .find(&query_lower)
        .map(|start| (start, start + query_lower.len()))
}

fn search_local_content(
    root: &str,
    query: &str,
    options: ContentSearchOptions,
) -> FsResult<Vec<ContentSearchResult>> {
    let root_path = expand_local_search_root(root)?;
    let root_metadata = fs::metadata(&root_path)
        .map_err(|error| FsError::io("Unable to read search root", &root_path, error))?;

    if !root_metadata.is_dir() {
        return Err(FsError::new(
            "search_root_not_directory",
            "Search root must be a local folder.",
            Some(root_path.to_string_lossy().into_owned()),
        ));
    }

    let query = query.trim();

    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = options.limit.clamp(1, 500);
    let max_file_bytes = options.max_file_bytes.max(1024);
    let matcher = if options.regex {
        Some(
            RegexBuilder::new(query)
                .case_insensitive(!options.case_sensitive)
                .build()
                .map_err(|error| {
                    FsError::new(
                        "invalid_regex",
                        format!("Invalid search regex: {error}"),
                        None,
                    )
                })?,
        )
    } else {
        None
    };
    let builder = configure_walk_builder(
        &root_path,
        options.include_hidden,
        options.respect_ignore,
        false,
        options.max_depth,
    );
    let mut results = Vec::new();

    for entry in builder.build() {
        if results.len() >= limit {
            break;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        if !metadata.is_file() || metadata.len() > max_file_bytes {
            continue;
        }

        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };

        if is_probably_binary(&bytes) {
            continue;
        }

        let content = String::from_utf8_lossy(&bytes);

        for (line_index, line) in content.lines().enumerate() {
            let found = if let Some(regex) = matcher.as_ref() {
                regex
                    .find(line)
                    .map(|match_| (match_.start(), match_.end()))
            } else {
                find_plain_match(line, query, options.case_sensitive)
            };

            let Some((match_start, match_end)) = found else {
                continue;
            };
            let name = path
                .file_name()
                .unwrap_or_else(|| OsStr::new(""))
                .to_string_lossy()
                .into_owned();
            let parent_path = path
                .parent()
                .unwrap_or(&root_path)
                .to_string_lossy()
                .into_owned();

            results.push(ContentSearchResult {
                name,
                path: path.to_string_lossy().into_owned(),
                parent_path,
                line_number: line_index + 1,
                line_text: line_text_with_limit(line),
                match_start,
                match_end,
            });

            if results.len() >= limit {
                break;
            }
        }
    }

    Ok(results)
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

fn mount_system_volume(device_path: &str) -> FsResult<VolumeEntry> {
    #[cfg(target_os = "linux")]
    {
        return mount_linux_volume(device_path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(FsError::new(
            "mount_volume_unsupported",
            "Mounting volumes from Carelo is not supported on this platform yet.",
            Some(device_path.to_string()),
        ))
    }
}

#[cfg(target_os = "linux")]
fn mounted_linux_volume_for_device(device_path: &str) -> FsResult<Option<VolumeEntry>> {
    Ok(list_linux_volumes()?.into_iter().find(|volume| {
        volume.device_path.as_deref() == Some(device_path)
            && volume.is_mounted
            && !volume.path.is_empty()
    }))
}

#[cfg(target_os = "linux")]
fn mount_linux_volume(device_path: &str) -> FsResult<VolumeEntry> {
    let device_path = device_path.trim();

    if !device_path.starts_with("/dev/") {
        return Err(FsError::new(
            "invalid_volume_device",
            "Volume device path must start with /dev/.",
            Some(device_path.to_string()),
        ));
    }

    if let Some(volume) = mounted_linux_volume_for_device(device_path)? {
        return Ok(volume);
    }

    let output = Command::new("udisksctl")
        .args(["mount", "-b", device_path])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            FsError::new(
                "mount_volume_spawn_failed",
                format!("Unable to start udisksctl to mount the volume: {error}"),
                Some(device_path.to_string()),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(FsError::new(
            "mount_volume_failed",
            if detail.is_empty() {
                "Unable to mount the volume.".to_string()
            } else {
                format!("Unable to mount the volume: {detail}")
            },
            Some(device_path.to_string()),
        ));
    }

    for _ in 0..20 {
        if let Some(volume) = mounted_linux_volume_for_device(device_path)? {
            return Ok(volume);
        }

        thread::sleep(Duration::from_millis(150));
    }

    Err(FsError::new(
        "mount_volume_path_not_found",
        "The volume mounted, but Carelo could not find its mount path yet.",
        Some(device_path.to_string()),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_media_ranges() {
        assert_eq!(parse_media_range("bytes=0-1023", 10_000), Some((0, 1023)));
        assert_eq!(parse_media_range("Bytes=500-", 10_000), Some((500, 9999)));
        assert_eq!(
            parse_media_range("bytes=500-20", 10_000),
            None,
            "end before start is not satisfiable"
        );
        assert_eq!(
            parse_media_range("items=0-10", 10_000),
            None,
            "only byte ranges are supported"
        );
    }

    #[test]
    fn parses_suffix_media_ranges() {
        assert_eq!(parse_media_range("bytes=-500", 10_000), Some((9500, 9999)));
        assert_eq!(parse_media_range("bytes=-15000", 10_000), Some((0, 9999)));
        assert_eq!(parse_media_range("bytes=-0", 10_000), None);
    }

    #[test]
    fn sanitizes_media_url_extension() {
        assert_eq!(
            media_url_extension(Path::new("/tmp/example.MP4")),
            ".mp4".to_string()
        );
        assert_eq!(
            media_url_extension(Path::new("/tmp/example.bad-ext")),
            String::new()
        );
    }

    #[test]
    fn media_stream_headers_are_valid_http_headers() {
        let headers = media_stream_header_response(
            "206 Partial Content",
            "video/mp4",
            1024,
            Some("bytes 0-1023/2048"),
        );

        assert!(headers.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(headers.contains("\r\nContent-Type: video/mp4\r\n"));
        assert!(headers.contains("\r\nContent-Range: bytes 0-1023/2048\r\n"));
        assert!(!headers.contains("\r\n Content-Type"));
        assert!(headers.ends_with("\r\n\r\n"));
    }

    #[test]
    fn media_stream_server_serves_byte_ranges() {
        let path = std::env::temp_dir().join(format!("carelo-media-{}.mp4", random_token(10)));
        fs::write(&path, b"0123456789abcdef").expect("write test media file");

        let state = MediaStreamState::default();
        let url = match state.stream_url_for(path.clone()) {
            Ok(url) => url,
            Err(error)
                if error.code == "media_stream_server_unavailable"
                    && error.message.contains("Operation not permitted") =>
            {
                let _ = fs::remove_file(path);
                return;
            }
            Err(error) => panic!("create stream URL: {error:?}"),
        };
        let parsed = url::Url::parse(&url).expect("parse stream URL");
        let port = parsed.port().expect("stream URL includes port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to stream server");
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=4-7\r\nConnection: close\r\n\r\n",
            parsed.path(),
        );

        stream
            .write_all(request.as_bytes())
            .expect("write stream request");

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("read stream response");
        let response = String::from_utf8_lossy(&response);

        assert!(response.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(response.contains("\r\nContent-Type: video/mp4\r\n"));
        assert!(response.contains("\r\nContent-Range: bytes 4-7/16\r\n"));
        assert!(response.ends_with("4567"));

        let _ = fs::remove_file(path);
    }
}
