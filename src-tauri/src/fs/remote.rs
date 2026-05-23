use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use opendal::{Entry, EntryMode, ErrorKind, Metadata, Operator};
use serde::{Deserialize, Serialize};

use crate::fs::models::{FileEntry, FileEntryKind, FileMetadata, FsError, FsResult, VolumeEntry};
use crate::fs::operations::SymlinkMode;
use crate::fs::sftp_mount::{is_password_sftp_config, sftp_password_fs_operator_options};
use crate::fs::smb::{is_smb_scheme, smb_fs_operator_options};

const REMOTE_PATH_PREFIX: &str = "remote://";
const TRANSFER_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
pub struct RemoteVolumeState {
    volumes: Mutex<HashMap<String, RemoteVolumeConfig>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVolumeConfig {
    pub id: String,
    pub name: String,
    pub scheme: String,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub options: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVolumeInfo {
    pub id: String,
    pub name: String,
    pub scheme: String,
    pub path: String,
    pub root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemotePath {
    pub volume_id: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct RemoteFileRead {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteSizeMeasure {
    pub logical_bytes: u64,
    pub disk_bytes: u64,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub skipped: u64,
    pub processed_entries: u64,
}

impl RemoteVolumeState {
    pub fn add(&self, config: RemoteVolumeConfig) -> FsResult<RemoteVolumeInfo> {
        config.validate()?;
        let info = config.info();
        let mut volumes = self.volumes.lock().map_err(lock_error)?;
        volumes.insert(config.id.clone(), config);
        Ok(info)
    }

    pub fn remove(&self, id: &str) -> FsResult<bool> {
        let mut volumes = self.volumes.lock().map_err(lock_error)?;
        Ok(volumes.remove(id).is_some())
    }

    pub fn list(&self) -> FsResult<Vec<RemoteVolumeInfo>> {
        let volumes = self.volumes.lock().map_err(lock_error)?;
        let mut entries: Vec<_> = volumes.values().map(RemoteVolumeConfig::info).collect();
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(entries)
    }

    pub fn config(&self, id: &str) -> FsResult<RemoteVolumeConfig> {
        let volumes = self.volumes.lock().map_err(lock_error)?;
        volumes.get(id).cloned().ok_or_else(|| {
            FsError::new(
                "remote_not_found",
                format!("Remote volume '{id}' is not registered."),
                Some(remote_root_uri(id)),
            )
        })
    }

    pub fn volume_entries(&self) -> FsResult<Vec<VolumeEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .map(|remote| VolumeEntry {
                name: remote.name,
                path: remote.path,
                device_path: None,
                detail: Some(remote.scheme.to_uppercase()),
                is_removable: false,
                is_mounted: true,
            })
            .collect())
    }
}

impl RemoteVolumeConfig {
    fn validate(&self) -> FsResult<()> {
        if self.id.trim().is_empty() {
            return Err(FsError::new(
                "invalid_remote_config",
                "Remote volume id is required.",
                None,
            ));
        }

        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(FsError::new(
                "invalid_remote_config",
                "Remote volume id can only contain letters, numbers, dashes, and underscores.",
                None,
            ));
        }

        if self.name.trim().is_empty() {
            return Err(FsError::new(
                "invalid_remote_config",
                "Remote volume name is required.",
                None,
            ));
        }

        validate_remote_scheme(&self.scheme)?;
        Ok(())
    }

    fn info(&self) -> RemoteVolumeInfo {
        RemoteVolumeInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            scheme: self.scheme.to_ascii_lowercase(),
            path: remote_root_uri(&self.id),
            root: self.root.clone(),
        }
    }

    fn operator(&self) -> FsResult<Operator> {
        self.validate()?;
        let scheme = self.scheme.to_ascii_lowercase();

        if is_smb_scheme(&scheme) {
            let options = smb_fs_operator_options(self)?;
            return Operator::via_iter("fs", options).map_err(|error| {
                FsError::new(
                    "remote_operator_error",
                    format!(
                        "Unable to initialize remote volume '{}': {error}",
                        self.name
                    ),
                    Some(remote_root_uri(&self.id)),
                )
            });
        }

        if is_password_sftp_config(self) {
            let options = sftp_password_fs_operator_options(self)?;
            return Operator::via_iter("fs", options).map_err(|error| {
                FsError::new(
                    "remote_operator_error",
                    format!(
                        "Unable to initialize remote volume '{}': {error}",
                        self.name
                    ),
                    Some(remote_root_uri(&self.id)),
                )
            });
        }

        let mut options: Vec<(String, String)> = self
            .options
            .iter()
            .filter_map(|(key, value)| {
                let key = key.trim();
                if key.is_empty() {
                    return None;
                }
                Some((key.to_string(), value.clone()))
            })
            .collect();

        if let Some(root) = self.root.as_ref().filter(|root| !root.trim().is_empty()) {
            options.push(("root".to_string(), root.clone()));
        }

        Operator::via_iter(scheme, options).map_err(|error| {
            FsError::new(
                "remote_operator_error",
                format!(
                    "Unable to initialize remote volume '{}': {error}",
                    self.name
                ),
                Some(remote_root_uri(&self.id)),
            )
        })
    }
}

pub fn parse_remote_path(path: &str) -> Option<RemotePath> {
    let rest = path.strip_prefix(REMOTE_PATH_PREFIX)?;
    let (volume_id, path) = rest.split_once('/').unwrap_or((rest, ""));

    if volume_id.is_empty() {
        return None;
    }

    Some(RemotePath {
        volume_id: volume_id.to_string(),
        path: normalize_remote_object_path(path),
    })
}

pub fn remote_root_uri(id: &str) -> String {
    format!("{REMOTE_PATH_PREFIX}{id}/")
}

pub async fn check_remote(config: &RemoteVolumeConfig) -> FsResult<()> {
    let op = config.operator()?;
    op.check().await.map_err(|error| {
        FsError::new(
            "remote_check_failed",
            format!(
                "Remote volume '{}' failed its connection check: {error}",
                config.name
            ),
            Some(remote_root_uri(&config.id)),
        )
    })
}

pub async fn list_remote_directory(
    state: &RemoteVolumeState,
    remote_path: RemotePath,
) -> FsResult<Vec<FileEntry>> {
    let config = state.config(&remote_path.volume_id)?;
    let op = config.operator()?;
    let path = normalize_remote_directory_path(&remote_path.path);
    let entries = op
        .list(&path)
        .await
        .map_err(|error| remote_error("remote_list_failed", &remote_path, error))?;
    let mut result = Vec::new();

    for entry in entries {
        if same_remote_path(entry.path(), &path) {
            continue;
        }

        result.push(entry_to_file_entry(&remote_path.volume_id, entry));
    }

    sort_remote_entries(&mut result);
    Ok(result)
}

pub async fn create_remote_folder(state: &RemoteVolumeState, path: RemotePath) -> FsResult<()> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_directory_path(&path.path);
    op.create_dir(&object_path)
        .await
        .map_err(|error| remote_error("remote_create_dir_failed", &path, error))
}

pub async fn rename_remote_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: RemotePath,
) -> FsResult<()> {
    ensure_same_remote(&from, &to)?;
    let config = state.config(&from.volume_id)?;
    let op = config.operator()?;
    op.rename(
        &normalize_remote_object_path(&from.path),
        &normalize_remote_object_path(&to.path),
    )
    .await
    .map_err(|error| remote_error("remote_rename_failed", &from, error))
}

pub async fn delete_remote_item(state: &RemoteVolumeState, path: RemotePath) -> FsResult<()> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_object_path(&path.path);
    let stat = op
        .stat(&object_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &path, error))?;

    if stat.is_dir() {
        op.delete_with(&normalize_remote_directory_path(&path.path))
            .recursive(true)
            .await
            .map_err(|error| remote_error("remote_delete_failed", &path, error))
    } else {
        op.delete(&object_path)
            .await
            .map_err(|error| remote_error("remote_delete_failed", &path, error))
    }
}

pub async fn copy_remote_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: RemotePath,
    overwrite: bool,
) -> FsResult<()> {
    let source_config = state.config(&from.volume_id)?;
    let source_op = source_config.operator()?;
    let source_path = normalize_remote_object_path(&from.path);
    let source = source_op
        .stat(&source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &from, error))?;

    if from.volume_id == to.volume_id {
        ensure_remote_destination_available(&source_op, &from, &to, overwrite).await?;

        if source.is_dir() {
            return copy_remote_directory(&source_op, &from, &to).await;
        }

        let target_path = normalize_remote_object_path(&to.path);

        return match source_op.copy(&source_path, &target_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::Unsupported => {
                copy_remote_file_path_between(&source_op, &source_op, &from, &source_path, &to)
                    .await
            }
            Err(error) => Err(remote_error("remote_copy_failed", &from, error)),
        };
    }

    let target_config = state.config(&to.volume_id)?;
    let target_op = target_config.operator()?;
    let existed_before = remote_path_exists(&target_op, &to).await?;
    ensure_remote_destination_available_for_type(&target_op, source.is_dir(), &to, overwrite)
        .await?;

    let result = if source.is_dir() {
        copy_remote_directory_between(&source_op, &target_op, &from, &to).await
    } else {
        copy_remote_file_path_between(&source_op, &target_op, &from, &source_path, &to).await
    };

    if result.is_err() && !overwrite && !existed_before {
        cleanup_remote_partial_copy(&target_op, &to).await;
    }

    result
}

pub async fn copy_local_to_remote_item(
    state: &RemoteVolumeState,
    from: &Path,
    to: RemotePath,
    overwrite: bool,
    symlink_mode: SymlinkMode,
) -> FsResult<()> {
    let config = state.config(&to.volume_id)?;
    let op = config.operator()?;
    let existed_before = remote_path_exists(&op, &to).await?;
    let result = copy_local_path_to_remote(&op, from, &to, overwrite, symlink_mode).await;

    if result.is_err() && !overwrite && !existed_before {
        cleanup_remote_partial_copy(&op, &to).await;
    }

    result
}

pub async fn copy_remote_to_local_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: &Path,
    overwrite: bool,
) -> FsResult<()> {
    let config = state.config(&from.volume_id)?;
    let op = config.operator()?;
    let source_path = normalize_remote_object_path(&from.path);
    let source = op
        .stat(&source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &from, error))?;
    ensure_local_destination_available(&source, to, overwrite)?;
    let existed_before = local_path_exists(to)?;

    let result = if source.is_dir() {
        copy_remote_directory_to_local(&op, &from, to).await
    } else {
        copy_remote_file_to_local(&op, &from, to, overwrite).await
    };

    if result.is_err() && !overwrite && !existed_before {
        cleanup_local_partial_copy(to);
    }

    result
}

pub async fn move_remote_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: RemotePath,
    overwrite: bool,
) -> FsResult<()> {
    if from.volume_id != to.volume_id {
        copy_remote_item(state, from.clone(), to, overwrite).await?;
        return delete_remote_item(state, from).await;
    }

    let config = state.config(&from.volume_id)?;
    let op = config.operator()?;
    ensure_remote_destination_available(&op, &from, &to, overwrite).await?;
    op.rename(
        &normalize_remote_object_path(&from.path),
        &normalize_remote_object_path(&to.path),
    )
    .await
    .map_err(|error| remote_error("remote_rename_failed", &from, error))
}

pub async fn move_local_to_remote_item(
    state: &RemoteVolumeState,
    from: &Path,
    to: RemotePath,
    overwrite: bool,
    symlink_mode: SymlinkMode,
) -> FsResult<()> {
    copy_local_to_remote_item(state, from, to, overwrite, symlink_mode).await?;
    delete_local_path(from)
}

pub async fn move_remote_to_local_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: &Path,
    overwrite: bool,
) -> FsResult<()> {
    copy_remote_to_local_item(state, from.clone(), to, overwrite).await?;
    delete_remote_item(state, from).await
}

pub async fn stat_remote_item(
    state: &RemoteVolumeState,
    path: RemotePath,
) -> FsResult<FileMetadata> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_object_path(&path.path);
    let metadata = op
        .stat(&object_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &path, error))?;

    Ok(metadata_to_file_metadata(
        &format_remote_uri(&path.volume_id, &object_path),
        &object_path,
        &metadata,
    ))
}

pub async fn read_remote_file_prefix(
    state: &RemoteVolumeState,
    path: RemotePath,
    max_bytes: u64,
) -> FsResult<RemoteFileRead> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_object_path(&path.path);
    let metadata = op
        .stat(&object_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "preview_not_file",
            "Preview is available for files only.",
            Some(format_remote_uri(&path.volume_id, &path.path)),
        ));
    }

    let total_bytes = metadata.content_length();
    let read_bytes = total_bytes.min(max_bytes);
    let bytes = if read_bytes == 0 {
        Vec::new()
    } else {
        op.reader(&object_path)
            .await
            .map_err(|error| remote_error("remote_read_failed", &path, error))?
            .read(0..read_bytes)
            .await
            .map_err(|error| remote_error("remote_read_failed", &path, error))?
            .to_vec()
    };

    Ok(RemoteFileRead {
        bytes,
        truncated: total_bytes > read_bytes,
        total_bytes,
    })
}

pub async fn materialize_remote_file(
    state: &RemoteVolumeState,
    path: RemotePath,
) -> FsResult<PathBuf> {
    let metadata = stat_remote_item(state, path.clone()).await?;

    if metadata.kind == FileEntryKind::Directory {
        return Err(FsError::new(
            "remote_entry_is_directory",
            "Choose a file inside the remote volume.",
            Some(format_remote_uri(&path.volume_id, &path.path)),
        ));
    }

    let target_directory = std::env::temp_dir()
        .join("carelo-remote-open")
        .join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
    fs::create_dir_all(&target_directory).map_err(|error| {
        FsError::io(
            "Unable to create temporary remote file directory",
            &target_directory,
            error,
        )
    })?;

    let file_name = remote_entry_name(&path.path);
    let target = target_directory.join(if file_name.is_empty() {
        "remote-file".to_string()
    } else {
        file_name
    });
    copy_remote_to_local_item(state, path, &target, true).await?;
    Ok(target)
}

pub async fn measure_remote_items_size(
    state: &RemoteVolumeState,
    paths: Vec<RemotePath>,
) -> FsResult<RemoteSizeMeasure> {
    let mut accumulator = RemoteSizeMeasure::default();

    for root in paths {
        let config = state.config(&root.volume_id)?;
        let op = config.operator()?;
        let mut stack = vec![normalize_remote_object_path(&root.path)];

        while let Some(object_path) = stack.pop() {
            let metadata = match op.stat(&object_path).await {
                Ok(metadata) => metadata,
                Err(_) => {
                    accumulator.skipped = accumulator.skipped.saturating_add(1);
                    accumulator.processed_entries = accumulator.processed_entries.saturating_add(1);
                    continue;
                }
            };

            accumulator.processed_entries = accumulator.processed_entries.saturating_add(1);

            match metadata.mode() {
                EntryMode::DIR => {
                    accumulator.directories = accumulator.directories.saturating_add(1);
                    let directory_path = normalize_remote_directory_path(&object_path);
                    let entries = match op.list(&directory_path).await {
                        Ok(entries) => entries,
                        Err(_) => {
                            accumulator.skipped = accumulator.skipped.saturating_add(1);
                            continue;
                        }
                    };

                    for entry in entries {
                        if same_remote_path(entry.path(), &directory_path) {
                            continue;
                        }

                        stack.push(normalize_remote_object_path(entry.path()));
                    }
                }
                EntryMode::FILE => {
                    let bytes = metadata.content_length();
                    accumulator.files = accumulator.files.saturating_add(1);
                    accumulator.logical_bytes = accumulator.logical_bytes.saturating_add(bytes);
                    accumulator.disk_bytes = accumulator.disk_bytes.saturating_add(bytes);
                }
                EntryMode::Unknown => {
                    accumulator.skipped = accumulator.skipped.saturating_add(1);
                }
            }
        }
    }

    Ok(accumulator)
}

fn validate_remote_scheme(scheme: &str) -> FsResult<()> {
    match scheme.to_ascii_lowercase().as_str() {
        "b2" | "cifs" | "dropbox" | "fs" | "ftp" | "gdrive" | "memory" | "onedrive" | "s3"
        | "sftp" | "smb" | "swift" | "webdav" => Ok(()),
        _ => Err(FsError::new(
            "unsupported_remote_scheme",
            format!("Remote scheme '{scheme}' is not enabled in Carelo."),
            None,
        )),
    }
}

async fn ensure_remote_destination_available(
    op: &Operator,
    from: &RemotePath,
    to: &RemotePath,
    overwrite: bool,
) -> FsResult<()> {
    let object_path = normalize_remote_object_path(&to.path);
    let destination = match op.stat(&object_path).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(remote_error("remote_stat_failed", to, error)),
    };

    if !overwrite {
        return if destination.is_some() {
            Err(FsError::new(
                "destination_exists",
                "An item already exists at the destination.",
                Some(format_remote_uri(&to.volume_id, &object_path)),
            ))
        } else {
            Ok(())
        };
    }

    let Some(destination) = destination else {
        return Ok(());
    };

    let source_path = normalize_remote_object_path(&from.path);
    let source = op
        .stat(&source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", from, error))?;

    if source.is_dir() || destination.is_dir() {
        return Err(FsError::new(
            "destination_type_conflict",
            "The existing destination has an incompatible type.",
            Some(format_remote_uri(&to.volume_id, &object_path)),
        ));
    }

    Ok(())
}

async fn ensure_remote_destination_available_for_type(
    op: &Operator,
    source_is_dir: bool,
    to: &RemotePath,
    overwrite: bool,
) -> FsResult<()> {
    let object_path = normalize_remote_object_path(&to.path);
    let destination = match op.stat(&object_path).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(remote_error("remote_stat_failed", to, error)),
    };

    if !overwrite {
        return if destination.is_some() {
            Err(FsError::new(
                "destination_exists",
                "An item already exists at the destination.",
                Some(format_remote_uri(&to.volume_id, &object_path)),
            ))
        } else {
            Ok(())
        };
    }

    if let Some(destination) = destination {
        if source_is_dir || destination.is_dir() {
            return Err(FsError::new(
                "destination_type_conflict",
                "The existing destination has an incompatible type.",
                Some(format_remote_uri(&to.volume_id, &object_path)),
            ));
        }
    }

    Ok(())
}

async fn copy_remote_directory(op: &Operator, from: &RemotePath, to: &RemotePath) -> FsResult<()> {
    copy_remote_directory_between(op, op, from, to).await
}

async fn copy_remote_directory_between(
    source_op: &Operator,
    target_op: &Operator,
    from: &RemotePath,
    to: &RemotePath,
) -> FsResult<()> {
    let source_prefix = normalize_remote_directory_path(&from.path);
    let target_prefix = normalize_remote_directory_path(&to.path);

    target_op
        .create_dir(&target_prefix)
        .await
        .map_err(|error| remote_error("remote_create_dir_failed", to, error))?;

    let entries = source_op
        .list_with(&source_prefix)
        .recursive(true)
        .await
        .map_err(|error| remote_error("remote_list_failed", from, error))?;

    for entry in entries {
        if same_remote_path(entry.path(), &source_prefix) {
            continue;
        }

        let Some(relative_path) = entry.path().strip_prefix(&source_prefix) else {
            continue;
        };
        let target_path = format!("{target_prefix}{relative_path}");

        match entry.metadata().mode() {
            EntryMode::DIR => {
                target_op
                    .create_dir(&normalize_remote_directory_path(&target_path))
                    .await
                    .map_err(|error| remote_error("remote_create_dir_failed", to, error))?;
            }
            EntryMode::FILE => {
                let target_remote = RemotePath {
                    volume_id: to.volume_id.clone(),
                    path: normalize_remote_object_path(&target_path),
                };
                copy_remote_file_path_between(
                    source_op,
                    target_op,
                    from,
                    entry.path(),
                    &target_remote,
                )
                .await?;
            }
            EntryMode::Unknown => {}
        }
    }

    Ok(())
}

async fn copy_remote_file_path_between(
    source_op: &Operator,
    target_op: &Operator,
    from: &RemotePath,
    source_path: &str,
    to: &RemotePath,
) -> FsResult<()> {
    let metadata = source_op
        .stat(source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", from, error))?;
    let reader = source_op
        .reader(source_path)
        .await
        .map_err(|error| remote_error("remote_read_failed", from, error))?;
    let mut writer = target_op
        .writer(&normalize_remote_object_path(&to.path))
        .await
        .map_err(|error| remote_error("remote_write_failed", to, error))?;
    let total_bytes = metadata.content_length();
    let mut offset = 0_u64;

    while offset < total_bytes {
        let end = offset
            .saturating_add(TRANSFER_BUFFER_BYTES as u64)
            .min(total_bytes);
        let bytes = match reader.read(offset..end).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = writer.abort().await;
                return Err(remote_error("remote_read_failed", from, error));
            }
        };

        if let Err(error) = writer.write(bytes).await {
            let _ = writer.abort().await;
            return Err(remote_error("remote_write_failed", to, error));
        }

        offset = end;
    }

    writer
        .close()
        .await
        .map(|_| ())
        .map_err(|error| remote_error("remote_write_failed", to, error))
}

async fn copy_local_path_to_remote(
    op: &Operator,
    from: &Path,
    to: &RemotePath,
    overwrite: bool,
    symlink_mode: SymlinkMode,
) -> FsResult<()> {
    let source = local_source_metadata(from, symlink_mode)?;
    ensure_remote_destination_available_for_type(op, source.is_dir(), to, overwrite).await?;

    if source.is_dir() {
        copy_local_directory_to_remote(op, from, to, overwrite, symlink_mode).await
    } else {
        copy_local_file_to_remote(op, from, to).await
    }
}

async fn copy_local_directory_to_remote(
    op: &Operator,
    from: &Path,
    to: &RemotePath,
    overwrite: bool,
    symlink_mode: SymlinkMode,
) -> FsResult<()> {
    let target_prefix = normalize_remote_directory_path(&to.path);
    let mut visited_directories = HashSet::new();
    remember_local_directory(from, &mut visited_directories)?;

    op.create_dir(&target_prefix)
        .await
        .map_err(|error| remote_error("remote_create_dir_failed", to, error))?;

    let mut pending = vec![(from.to_path_buf(), normalize_remote_object_path(&to.path))];

    while let Some((local_directory, remote_directory)) = pending.pop() {
        for child in fs::read_dir(&local_directory).map_err(|error| {
            FsError::io("Unable to read source directory", &local_directory, error)
        })? {
            let child = child.map_err(|error| {
                FsError::io(
                    "Unable to read source directory entry",
                    &local_directory,
                    error,
                )
            })?;
            let child_path = child.path();
            let child_remote_path = join_remote_child_path(&remote_directory, &child.file_name())?;
            let child_remote = RemotePath {
                volume_id: to.volume_id.clone(),
                path: child_remote_path,
            };
            let child_source = local_source_metadata(&child_path, symlink_mode)?;

            ensure_remote_destination_available_for_type(
                op,
                child_source.is_dir(),
                &child_remote,
                overwrite,
            )
            .await?;

            if child_source.is_dir() {
                remember_local_directory(&child_path, &mut visited_directories)?;
                op.create_dir(&normalize_remote_directory_path(&child_remote.path))
                    .await
                    .map_err(|error| {
                        remote_error("remote_create_dir_failed", &child_remote, error)
                    })?;
                pending.push((child_path, child_remote.path));
            } else {
                copy_local_file_to_remote(op, &child_path, &child_remote).await?;
            }
        }
    }

    Ok(())
}

async fn copy_local_file_to_remote(op: &Operator, from: &Path, to: &RemotePath) -> FsResult<()> {
    let mut reader =
        File::open(from).map_err(|error| FsError::io("Unable to open source file", from, error))?;
    let mut writer = op
        .writer(&normalize_remote_object_path(&to.path))
        .await
        .map_err(|error| remote_error("remote_write_failed", to, error))?;
    let mut buffer = vec![0_u8; TRANSFER_BUFFER_BYTES];

    loop {
        let bytes_read = match reader.read(&mut buffer) {
            Ok(bytes_read) => bytes_read,
            Err(error) => {
                let _ = writer.abort().await;
                return Err(FsError::io("Unable to read source file", from, error));
            }
        };

        if bytes_read == 0 {
            break;
        }

        if let Err(error) = writer.write(buffer[..bytes_read].to_vec()).await {
            let _ = writer.abort().await;
            return Err(remote_error("remote_write_failed", to, error));
        }
    }

    writer
        .close()
        .await
        .map(|_| ())
        .map_err(|error| remote_error("remote_write_failed", to, error))
}

async fn copy_remote_directory_to_local(
    op: &Operator,
    from: &RemotePath,
    to: &Path,
) -> FsResult<()> {
    let source_prefix = normalize_remote_directory_path(&from.path);

    fs::create_dir_all(to)
        .map_err(|error| FsError::io("Unable to create destination directory", to, error))?;

    let entries = op
        .list_with(&source_prefix)
        .recursive(true)
        .await
        .map_err(|error| remote_error("remote_list_failed", from, error))?;

    for entry in entries {
        if same_remote_path(entry.path(), &source_prefix) {
            continue;
        }

        let Some(relative_path) = entry.path().strip_prefix(&source_prefix) else {
            continue;
        };
        let target_path = to.join(relative_path);

        match entry.metadata().mode() {
            EntryMode::DIR => {
                fs::create_dir_all(&target_path).map_err(|error| {
                    FsError::io(
                        "Unable to create destination directory",
                        &target_path,
                        error,
                    )
                })?;
            }
            EntryMode::FILE => {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        FsError::io("Unable to create destination directory", parent, error)
                    })?;
                }
                copy_remote_file_path_to_local(op, from, entry.path(), &target_path, false).await?;
            }
            EntryMode::Unknown => {}
        }
    }

    Ok(())
}

async fn copy_remote_file_to_local(
    op: &Operator,
    from: &RemotePath,
    to: &Path,
    overwrite: bool,
) -> FsResult<()> {
    copy_remote_file_path_to_local(
        op,
        from,
        &normalize_remote_object_path(&from.path),
        to,
        overwrite,
    )
    .await
}

async fn copy_remote_file_path_to_local(
    op: &Operator,
    from: &RemotePath,
    source_path: &str,
    to: &Path,
    overwrite: bool,
) -> FsResult<()> {
    if overwrite && local_path_exists(to)? {
        remove_existing_local_file_like(to)?;
    }

    let metadata = op
        .stat(source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", from, error))?;
    let reader = op
        .reader(source_path)
        .await
        .map_err(|error| remote_error("remote_read_failed", from, error))?;
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|error| FsError::io("Unable to create destination file", to, error))?;
    let total_bytes = metadata.content_length();
    let mut offset = 0_u64;

    while offset < total_bytes {
        let end = offset
            .saturating_add(TRANSFER_BUFFER_BYTES as u64)
            .min(total_bytes);
        let bytes = reader
            .read(offset..end)
            .await
            .map_err(|error| remote_error("remote_read_failed", from, error))?;

        for chunk in bytes.to_io_slice() {
            writer
                .write_all(chunk.as_ref())
                .map_err(|error| FsError::io("Unable to write destination file", to, error))?;
        }

        offset = end;
    }

    writer
        .flush()
        .map_err(|error| FsError::io("Unable to write destination file", to, error))
}

fn local_source_metadata(from: &Path, symlink_mode: SymlinkMode) -> FsResult<fs::Metadata> {
    let symlink_metadata = fs::symlink_metadata(from)
        .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

    if symlink_metadata.file_type().is_symlink() {
        if matches!(symlink_mode, SymlinkMode::Preserve) {
            return Err(FsError::new(
                "symlink_unsupported",
                "Remote volumes do not support preserving symbolic links. Resolve link targets before copying to a remote volume.",
                Some(from.to_string_lossy().into_owned()),
            ));
        }

        return fs::metadata(from)
            .map_err(|error| FsError::io("Unable to read symlink target metadata", from, error));
    }

    Ok(symlink_metadata)
}

fn remember_local_directory(
    path: &Path,
    visited_directories: &mut HashSet<PathBuf>,
) -> FsResult<()> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| FsError::io("Unable to resolve source directory", path, error))?;

    if visited_directories.insert(canonical) {
        Ok(())
    } else {
        Err(FsError::new(
            "directory_cycle",
            "Refusing to transfer a directory cycle.",
            Some(path.to_string_lossy().into_owned()),
        ))
    }
}

fn ensure_local_destination_available(
    source: &Metadata,
    to: &Path,
    overwrite: bool,
) -> FsResult<()> {
    let destination = match fs::symlink_metadata(to) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(FsError::io(
                "Unable to read destination metadata",
                to,
                error,
            ))
        }
    };

    if !overwrite {
        return if destination.is_some() {
            Err(local_destination_exists_error(to))
        } else {
            Ok(())
        };
    }

    if let Some(destination) = destination {
        if source.is_dir() || destination.is_dir() {
            return Err(local_destination_type_error(to));
        }
    }

    Ok(())
}

async fn remote_path_exists(op: &Operator, path: &RemotePath) -> FsResult<bool> {
    match op.stat(&normalize_remote_object_path(&path.path)).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(remote_error("remote_stat_failed", path, error)),
    }
}

fn local_path_exists(path: &Path) -> FsResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(FsError::io(
            "Unable to read destination metadata",
            path,
            error,
        )),
    }
}

fn remove_existing_local_file_like(path: &Path) -> FsResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| FsError::io("Unable to read destination metadata", path, error))?;

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(local_destination_type_error(path));
    }

    fs::remove_file(path)
        .map_err(|error| FsError::io("Unable to replace existing destination", path, error))
}

fn delete_local_path(path: &Path) -> FsResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| FsError::io("Unable to read item before delete", path, error))?;

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
            .map_err(|error| FsError::io("Unable to delete directory", path, error))
    } else {
        fs::remove_file(path).map_err(|error| FsError::io("Unable to delete file", path, error))
    }
}

async fn cleanup_remote_partial_copy(op: &Operator, path: &RemotePath) {
    let object_path = normalize_remote_object_path(&path.path);
    let Ok(metadata) = op.stat(&object_path).await else {
        return;
    };

    let _ = if metadata.is_dir() {
        op.delete_with(&normalize_remote_directory_path(&path.path))
            .recursive(true)
            .await
    } else {
        op.delete(&object_path).await
    };
}

fn cleanup_local_partial_copy(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };

    let _ = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
}

fn join_remote_child_path(parent: &str, child_name: &OsStr) -> FsResult<String> {
    let parent = normalize_remote_object_path(parent);
    let child_name = child_name.to_string_lossy();

    if child_name.is_empty() {
        return Err(FsError::new(
            "invalid_remote_path",
            "Remote item names cannot be empty.",
            None,
        ));
    }

    if parent.is_empty() {
        Ok(child_name.into_owned())
    } else {
        Ok(format!("{parent}/{child_name}"))
    }
}

fn local_destination_exists_error(path: &Path) -> FsError {
    FsError::new(
        "destination_exists",
        "An item already exists at the destination.",
        Some(path.to_string_lossy().into_owned()),
    )
}

fn local_destination_type_error(path: &Path) -> FsError {
    FsError::new(
        "destination_type_conflict",
        "The existing destination has an incompatible type.",
        Some(path.to_string_lossy().into_owned()),
    )
}

fn normalize_remote_object_path(path: &str) -> String {
    path.trim()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn normalize_remote_directory_path(path: &str) -> String {
    let path = normalize_remote_object_path(path);

    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

pub(crate) fn format_remote_uri(volume_id: &str, path: &str) -> String {
    let path = normalize_remote_object_path(path);

    if path.is_empty() {
        remote_root_uri(volume_id)
    } else {
        format!("{REMOTE_PATH_PREFIX}{volume_id}/{path}")
    }
}

fn entry_to_file_entry(volume_id: &str, entry: Entry) -> FileEntry {
    let path = entry.path().to_string();
    let metadata = entry.metadata().clone();
    let normalized_path = normalize_remote_object_path(&path);
    let name = remote_entry_name(&path);
    let is_hidden = name.starts_with('.');
    let kind = entry_kind(&metadata);

    FileEntry {
        name,
        path: format_remote_uri(volume_id, &normalized_path),
        kind,
        size: metadata.is_file().then_some(metadata.content_length()),
        modified_at: modified_seconds(&metadata),
        is_hidden,
        is_symlink: false,
        is_readonly: false,
        tag_color: None,
    }
}

fn metadata_to_file_metadata(path: &str, object_path: &str, metadata: &Metadata) -> FileMetadata {
    FileMetadata {
        path: path.to_string(),
        kind: entry_kind(metadata),
        size: metadata.is_file().then_some(metadata.content_length()),
        modified_at: modified_seconds(metadata),
        created_at: None,
        accessed_at: None,
        is_hidden: remote_entry_name(object_path).starts_with('.'),
        is_symlink: false,
        is_readonly: false,
        permissions: None,
    }
}

fn entry_kind(metadata: &Metadata) -> FileEntryKind {
    match metadata.mode() {
        EntryMode::DIR => FileEntryKind::Directory,
        EntryMode::FILE => FileEntryKind::File,
        EntryMode::Unknown => FileEntryKind::Other,
    }
}

fn remote_entry_name(path: &str) -> String {
    normalize_remote_object_path(path)
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn modified_seconds(metadata: &Metadata) -> Option<u64> {
    metadata.last_modified().and_then(|modified| {
        let modified: std::time::SystemTime = modified.into();
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
    })
}

fn same_remote_path(left: &str, right: &str) -> bool {
    normalize_remote_object_path(left) == normalize_remote_object_path(right)
}

fn ensure_same_remote(from: &RemotePath, to: &RemotePath) -> FsResult<()> {
    if from.volume_id == to.volume_id {
        return Ok(());
    }

    Err(FsError::new(
        "cross_remote_rename",
        "Renaming across remote volumes is not supported. Move the item instead.",
        Some(format_remote_uri(&from.volume_id, &from.path)),
    ))
}

fn remote_error(code: &str, path: &RemotePath, error: opendal::Error) -> FsError {
    FsError::new(
        code,
        error.to_string(),
        Some(format_remote_uri(&path.volume_id, &path.path)),
    )
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> FsError {
    FsError::new(
        "remote_state_lock_failed",
        format!("Remote volume state is unavailable: {error}"),
        None,
    )
}

fn sort_remote_entries(entries: &mut [FileEntry]) {
    entries.sort_by_cached_key(|entry| {
        (
            entry.kind.sort_rank(),
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after UNIX epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "carelo-remote-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn fs_config(id: &str, name: &str, root: &Path) -> RemoteVolumeConfig {
        RemoteVolumeConfig {
            id: id.to_string(),
            name: name.to_string(),
            scheme: "fs".to_string(),
            root: Some(root.to_string_lossy().into_owned()),
            options: HashMap::new(),
        }
    }

    fn add_fs_remote(state: &RemoteVolumeState, root: &Path) {
        state
            .add(fs_config("test", "Test Remote", root))
            .expect("fs remote should be registered");
    }

    fn remote(path: &str) -> RemotePath {
        parse_remote_path(path).expect("remote path should parse")
    }

    #[test]
    fn parses_and_formats_remote_paths() {
        let parsed = parse_remote_path("remote://server-a/folder/file.txt")
            .expect("remote path should parse");

        assert_eq!(parsed.volume_id, "server-a");
        assert_eq!(parsed.path, "folder/file.txt");
        assert_eq!(remote_root_uri("server-a"), "remote://server-a/");
        assert_eq!(
            format_remote_uri("server-a", "/folder/file.txt/"),
            "remote://server-a/folder/file.txt"
        );
        assert!(parse_remote_path("remote:///missing-id").is_none());
        assert!(parse_remote_path("/local/path").is_none());
    }

    #[test]
    fn validates_remote_volume_config_and_lists_sorted_volumes() {
        let state = RemoteVolumeState::default();
        let root = TestDir::new();

        state
            .add(fs_config("zeta", "Zeta", root.path()))
            .expect("valid remote should be added");
        state
            .add(fs_config("alpha", "Alpha", root.path()))
            .expect("valid remote should be added");

        let volumes = state.list().expect("volumes should list");
        assert_eq!(
            volumes
                .iter()
                .map(|volume| volume.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Zeta"]
        );
        assert_eq!(volumes[0].path, "remote://alpha/");
        let volume_entries = state
            .volume_entries()
            .expect("remote volumes should convert to sidebar entries");
        assert_eq!(volume_entries[0].name, "Alpha");
        assert_eq!(volume_entries[0].detail.as_deref(), Some("FS"));
        assert!(volume_entries[0].is_mounted);

        let invalid = state
            .add(RemoteVolumeConfig {
                id: "bad/id".to_string(),
                name: "Bad".to_string(),
                scheme: "fs".to_string(),
                root: Some(root.path().to_string_lossy().into_owned()),
                options: HashMap::new(),
            })
            .expect_err("invalid ids should be rejected");
        assert_eq!(invalid.code, "invalid_remote_config");

        state
            .add(RemoteVolumeConfig {
                id: "smb-share".to_string(),
                name: "SMB Share".to_string(),
                scheme: "smb".to_string(),
                root: None,
                options: HashMap::from([(
                    "endpoint".to_string(),
                    "smb://server/share".to_string(),
                )]),
            })
            .expect("SMB remotes should be accepted");

        let unsupported = state
            .add(RemoteVolumeConfig {
                id: "bad".to_string(),
                name: "Bad".to_string(),
                scheme: "nfs".to_string(),
                root: Some(root.path().to_string_lossy().into_owned()),
                options: HashMap::new(),
            })
            .expect_err("unsupported schemes should be rejected");
        assert_eq!(unsupported.code, "unsupported_remote_scheme");

        assert!(state.remove("zeta").expect("remote should remove"));
        assert!(!state
            .remove("zeta")
            .expect("missing remote should report false"));
    }

    #[test]
    fn lists_and_stats_fs_backed_remote_entries() {
        let state = RemoteVolumeState::default();
        let root = TestDir::new();
        fs::create_dir(root.path().join("Folder")).expect("folder should be created");
        fs::write(root.path().join("Folder").join("nested.md"), b"nested")
            .expect("nested file should be created");
        fs::write(root.path().join("alpha.txt"), b"hello").expect("file should be created");
        add_fs_remote(&state, root.path());

        let entries =
            tauri::async_runtime::block_on(list_remote_directory(&state, remote("remote://test/")))
                .expect("remote root should list");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Folder", "alpha.txt"]
        );
        assert_eq!(entries[0].kind, FileEntryKind::Directory);
        assert_eq!(entries[0].path, "remote://test/Folder");
        assert_eq!(entries[1].kind, FileEntryKind::File);
        assert_eq!(entries[1].size, Some(5));

        let metadata = tauri::async_runtime::block_on(stat_remote_item(
            &state,
            remote("remote://test/alpha.txt"),
        ))
        .expect("remote file should stat");
        assert_eq!(metadata.kind, FileEntryKind::File);
        assert_eq!(metadata.size, Some(5));
        assert_eq!(metadata.path, "remote://test/alpha.txt");
    }

    #[test]
    fn creates_copies_moves_and_deletes_fs_backed_remote_items() {
        let state = RemoteVolumeState::default();
        let root = TestDir::new();
        fs::write(root.path().join("source.txt"), b"hello").expect("file should be created");
        add_fs_remote(&state, root.path());

        tauri::async_runtime::block_on(create_remote_folder(
            &state,
            remote("remote://test/New Folder"),
        ))
        .expect("remote folder should be created");
        assert!(root.path().join("New Folder").is_dir());
        fs::write(root.path().join("New Folder").join("nested.txt"), b"nested")
            .expect("nested file should be created");

        tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://test/New Folder"),
            remote("remote://test/Copied Folder"),
            false,
        ))
        .expect("remote folder should copy recursively");
        assert_eq!(
            fs::read(root.path().join("Copied Folder").join("nested.txt"))
                .expect("copied nested file should be readable"),
            b"nested"
        );

        tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://test/source.txt"),
            remote("remote://test/copy.txt"),
            false,
        ))
        .expect("remote file should copy");
        assert_eq!(
            fs::read(root.path().join("copy.txt")).expect("copy should be readable"),
            b"hello"
        );

        let conflict = tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://test/source.txt"),
            remote("remote://test/copy.txt"),
            false,
        ))
        .expect_err("copy should not overwrite without permission");
        assert_eq!(conflict.code, "destination_exists");

        fs::write(root.path().join("source.txt"), b"updated").expect("file should be updated");
        tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://test/source.txt"),
            remote("remote://test/copy.txt"),
            true,
        ))
        .expect("remote file should overwrite");
        assert_eq!(
            fs::read(root.path().join("copy.txt")).expect("copy should be readable"),
            b"updated"
        );

        tauri::async_runtime::block_on(move_remote_item(
            &state,
            remote("remote://test/copy.txt"),
            remote("remote://test/moved.txt"),
            false,
        ))
        .expect("remote file should move");
        assert!(!root.path().join("copy.txt").exists());
        assert_eq!(
            fs::read(root.path().join("moved.txt")).expect("moved file should be readable"),
            b"updated"
        );

        tauri::async_runtime::block_on(delete_remote_item(
            &state,
            remote("remote://test/New Folder"),
        ))
        .expect("remote folder should delete recursively");
        tauri::async_runtime::block_on(delete_remote_item(
            &state,
            remote("remote://test/moved.txt"),
        ))
        .expect("remote file should delete");
        assert!(!root.path().join("New Folder").exists());
        assert!(!root.path().join("moved.txt").exists());
    }

    #[test]
    fn copies_and_moves_between_local_and_fs_backed_remote_items() {
        let state = RemoteVolumeState::default();
        let remote_root = TestDir::new();
        let local_root = TestDir::new();
        add_fs_remote(&state, remote_root.path());

        let local_file = local_root.path().join("upload.txt");
        fs::write(&local_file, b"local").expect("local file should be created");

        tauri::async_runtime::block_on(copy_local_to_remote_item(
            &state,
            &local_file,
            remote("remote://test/upload.txt"),
            false,
            SymlinkMode::Preserve,
        ))
        .expect("local file should copy to remote");
        assert_eq!(
            fs::read(remote_root.path().join("upload.txt"))
                .expect("remote upload should be readable"),
            b"local"
        );

        let conflict = tauri::async_runtime::block_on(copy_local_to_remote_item(
            &state,
            &local_file,
            remote("remote://test/upload.txt"),
            false,
            SymlinkMode::Preserve,
        ))
        .expect_err("local to remote copy should not overwrite implicitly");
        assert_eq!(conflict.code, "destination_exists");

        fs::write(&local_file, b"updated").expect("local file should be updated");
        tauri::async_runtime::block_on(copy_local_to_remote_item(
            &state,
            &local_file,
            remote("remote://test/upload.txt"),
            true,
            SymlinkMode::Preserve,
        ))
        .expect("local file should overwrite remote file");
        assert_eq!(
            fs::read(remote_root.path().join("upload.txt"))
                .expect("remote upload should be readable"),
            b"updated"
        );

        let local_folder = local_root.path().join("Folder");
        fs::create_dir(&local_folder).expect("local folder should be created");
        fs::write(local_folder.join("nested.txt"), b"nested")
            .expect("nested local file should be created");
        tauri::async_runtime::block_on(copy_local_to_remote_item(
            &state,
            &local_folder,
            remote("remote://test/Uploaded Folder"),
            false,
            SymlinkMode::Preserve,
        ))
        .expect("local folder should copy to remote");
        assert_eq!(
            fs::read(
                remote_root
                    .path()
                    .join("Uploaded Folder")
                    .join("nested.txt")
            )
            .expect("remote nested file should be readable"),
            b"nested"
        );

        let download_folder = local_root.path().join("Downloaded Folder");
        tauri::async_runtime::block_on(copy_remote_to_local_item(
            &state,
            remote("remote://test/Uploaded Folder"),
            &download_folder,
            false,
        ))
        .expect("remote folder should copy to local");
        assert_eq!(
            fs::read(download_folder.join("nested.txt"))
                .expect("downloaded nested file should be readable"),
            b"nested"
        );

        let move_up = local_root.path().join("move-up.txt");
        fs::write(&move_up, b"move up").expect("move source should be created");
        tauri::async_runtime::block_on(move_local_to_remote_item(
            &state,
            &move_up,
            remote("remote://test/move-up.txt"),
            false,
            SymlinkMode::Preserve,
        ))
        .expect("local file should move to remote");
        assert!(!move_up.exists());
        assert_eq!(
            fs::read(remote_root.path().join("move-up.txt"))
                .expect("moved remote file should be readable"),
            b"move up"
        );

        let move_down = local_root.path().join("move-down.txt");
        tauri::async_runtime::block_on(move_remote_to_local_item(
            &state,
            remote("remote://test/move-up.txt"),
            &move_down,
            false,
        ))
        .expect("remote file should move to local");
        assert!(!remote_root.path().join("move-up.txt").exists());
        assert_eq!(
            fs::read(move_down).expect("moved local file should be readable"),
            b"move up"
        );
    }

    #[test]
    fn reads_materializes_and_measures_remote_items() {
        let state = RemoteVolumeState::default();
        let remote_root = TestDir::new();
        fs::create_dir(remote_root.path().join("Folder")).expect("folder should be created");
        fs::write(
            remote_root.path().join("Folder").join("nested.txt"),
            b"nested",
        )
        .expect("nested file should be created");
        fs::write(remote_root.path().join("preview.txt"), b"hello remote")
            .expect("preview file should be created");
        add_fs_remote(&state, remote_root.path());

        let preview = tauri::async_runtime::block_on(read_remote_file_prefix(
            &state,
            remote("remote://test/preview.txt"),
            5,
        ))
        .expect("remote preview should read prefix");
        assert_eq!(preview.bytes, b"hello");
        assert!(preview.truncated);
        assert_eq!(preview.total_bytes, 12);

        let materialized = tauri::async_runtime::block_on(materialize_remote_file(
            &state,
            remote("remote://test/preview.txt"),
        ))
        .expect("remote file should materialize");
        assert_eq!(
            fs::read(&materialized).expect("materialized file should be readable"),
            b"hello remote"
        );
        let _ = fs::remove_dir_all(
            materialized
                .parent()
                .expect("materialized file should have parent"),
        );

        let measure = tauri::async_runtime::block_on(measure_remote_items_size(
            &state,
            vec![remote("remote://test/")],
        ))
        .expect("remote size should be measured");
        assert_eq!(measure.files, 2);
        assert_eq!(measure.directories, 2);
        assert_eq!(measure.logical_bytes, 18);
    }

    #[test]
    fn copies_and_moves_between_remote_volumes() {
        let state = RemoteVolumeState::default();
        let left_root = TestDir::new();
        let right_root = TestDir::new();
        fs::write(left_root.path().join("source.txt"), b"left")
            .expect("left source should be created");
        fs::create_dir(left_root.path().join("Folder")).expect("left folder should be created");
        fs::write(
            left_root.path().join("Folder").join("nested.txt"),
            b"nested",
        )
        .expect("left nested file should be created");
        state
            .add(fs_config("left", "Left", left_root.path()))
            .expect("left remote should be added");
        state
            .add(fs_config("right", "Right", right_root.path()))
            .expect("right remote should be added");

        tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://left/Folder"),
            remote("remote://right/Copied Folder"),
            false,
        ))
        .expect("cross-remote folder should copy recursively");
        assert_eq!(
            fs::read(right_root.path().join("Copied Folder").join("nested.txt"))
                .expect("cross-remote nested file should be readable"),
            b"nested"
        );

        tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://left/source.txt"),
            remote("remote://right/copy.txt"),
            false,
        ))
        .expect("cross-remote file should copy");
        assert_eq!(
            fs::read(right_root.path().join("copy.txt"))
                .expect("cross-remote copied file should be readable"),
            b"left"
        );

        let conflict = tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://left/source.txt"),
            remote("remote://right/copy.txt"),
            false,
        ))
        .expect_err("cross-remote copy should not overwrite implicitly");
        assert_eq!(conflict.code, "destination_exists");

        fs::write(left_root.path().join("source.txt"), b"updated")
            .expect("left source should update");
        tauri::async_runtime::block_on(copy_remote_item(
            &state,
            remote("remote://left/source.txt"),
            remote("remote://right/copy.txt"),
            true,
        ))
        .expect("cross-remote file should overwrite");
        assert_eq!(
            fs::read(right_root.path().join("copy.txt"))
                .expect("cross-remote overwritten file should be readable"),
            b"updated"
        );

        tauri::async_runtime::block_on(move_remote_item(
            &state,
            remote("remote://left/source.txt"),
            remote("remote://right/moved.txt"),
            false,
        ))
        .expect("cross-remote file should move");
        assert!(!left_root.path().join("source.txt").exists());
        assert_eq!(
            fs::read(right_root.path().join("moved.txt"))
                .expect("cross-remote moved file should be readable"),
            b"updated"
        );
    }
}
