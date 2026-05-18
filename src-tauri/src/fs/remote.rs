use std::collections::HashMap;
use std::sync::Mutex;

use opendal::{Entry, EntryMode, ErrorKind, Metadata, Operator};
use serde::{Deserialize, Serialize};

use crate::fs::models::{FileEntry, FileEntryKind, FileMetadata, FsError, FsResult, VolumeEntry};

const REMOTE_PATH_PREFIX: &str = "remote://";

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

        Operator::via_iter(self.scheme.to_ascii_lowercase(), options).map_err(|error| {
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
    ensure_same_remote(&from, &to)?;
    let config = state.config(&from.volume_id)?;
    let op = config.operator()?;
    ensure_remote_destination_available(&op, &from, &to, overwrite).await?;
    op.copy(
        &normalize_remote_object_path(&from.path),
        &normalize_remote_object_path(&to.path),
    )
    .await
    .map_err(|error| remote_error("remote_copy_failed", &from, error))
}

pub async fn move_remote_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: RemotePath,
    overwrite: bool,
) -> FsResult<()> {
    ensure_same_remote(&from, &to)?;
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

fn validate_remote_scheme(scheme: &str) -> FsResult<()> {
    match scheme.to_ascii_lowercase().as_str() {
        "b2" | "dropbox" | "fs" | "ftp" | "gdrive" | "memory" | "onedrive" | "s3" | "sftp"
        | "swift" | "webdav" => Ok(()),
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
        "cross_remote_operation",
        "Cross-remote operations are not implemented yet.",
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
    entries.sort_by(|a, b| {
        a.kind
            .sort_rank()
            .cmp(&b.kind.sort_rank())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}
