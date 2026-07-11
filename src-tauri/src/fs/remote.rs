use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::TryStreamExt;
use opendal::{Entry, EntryMode, ErrorKind, Metadata, Operator, Writer as RemoteWriter};
use openssh::{KnownHosts, SessionBuilder};
use openssh_sftp_client::metadata::{MetaData as SftpMetadata, Permissions as SftpPermissions};
use openssh_sftp_client::{Sftp, SftpOptions};
use serde::{Deserialize, Serialize};

use crate::fs::local::{file_permissions_from_unix_mode, permissions_for_metadata};
use crate::fs::models::{
    FileEntry, FileEntryKind, FileMetadata, FilePermissions, FsError, FsResult,
    RemoteVolumeCapabilities, RemoteVolumeHealth, VolumeEntry,
};
use crate::fs::operations::{LocalReplacementStage, SymlinkMode};
use crate::fs::sftp_mount::{
    is_password_sftp_config, sftp_password_fs_operator_options, unmount_sftp_password,
};
use crate::fs::smb::{is_smb_scheme, smb_fs_operator_options, unmount_smb};

const REMOTE_PATH_PREFIX: &str = "remote://";
const TRANSFER_BUFFER_BYTES: usize = 256 * 1024;
const REMOTE_SEARCH_READ_CHUNK_BYTES: usize = 256 * 1024;
const REMOTE_METADATA_CACHE_TTL: Duration = Duration::from_secs(2);
const REMOTE_METADATA_CACHE_MAX_ENTRIES: usize = 512;

#[derive(Debug, Default)]
pub struct RemoteVolumeState {
    volumes: Mutex<HashMap<String, RemoteVolumeConfig>>,
    health: Mutex<HashMap<String, RemoteVolumeHealth>>,
    active_ids: Mutex<HashSet<String>>,
    cache: Mutex<RemoteMetadataCache>,
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
    pub capabilities: RemoteVolumeCapabilities,
    pub health: RemoteVolumeHealth,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteReleaseResult {
    pub id: String,
    pub released: bool,
    pub message: Option<String>,
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
struct RemoteMetadataDetails {
    created_at: Option<u64>,
    accessed_at: Option<u64>,
    is_readonly: Option<bool>,
    permissions: Option<FilePermissions>,
}

#[derive(Debug, Default)]
struct RemoteMetadataCache {
    directories: HashMap<String, CachedRemoteValue<Vec<FileEntry>>>,
    metadata: HashMap<String, CachedRemoteValue<FileMetadata>>,
}

#[derive(Debug, Clone)]
struct CachedRemoteValue<T> {
    value: T,
    created_at: Instant,
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
        let info = config.info(self.health_for(&config.id)?);
        let id = config.id.clone();
        let mut volumes = self.volumes.lock().map_err(lock_error)?;
        volumes.insert(config.id.clone(), config);
        drop(volumes);
        self.invalidate_cache_for_volume(&id)?;
        Ok(info)
    }

    pub fn remove(&self, id: &str) -> FsResult<bool> {
        let mut volumes = self.volumes.lock().map_err(lock_error)?;
        let removed = volumes.remove(id).is_some();
        drop(volumes);
        self.health.lock().map_err(lock_error)?.remove(id);
        self.active_ids.lock().map_err(lock_error)?.remove(id);
        self.invalidate_cache_for_volume(id)?;
        Ok(removed)
    }

    pub fn list(&self) -> FsResult<Vec<RemoteVolumeInfo>> {
        let volumes = self.volumes.lock().map_err(lock_error)?;
        let health = self.health.lock().map_err(lock_error)?;
        let mut entries: Vec<_> = volumes
            .values()
            .map(|config| config.info(health_for_id(&health, &config.id)))
            .collect();
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
            .map(|remote| {
                let detail = remote_sidebar_detail(&remote);

                VolumeEntry {
                    name: remote.name,
                    path: remote.path,
                    device_path: None,
                    detail: Some(detail),
                    is_removable: false,
                    is_mounted: true,
                    is_encrypted: false,
                    needs_unlock: false,
                    capabilities: Some(remote.capabilities),
                    health: Some(remote.health),
                }
            })
            .collect())
    }

    pub fn health_for(&self, id: &str) -> FsResult<RemoteVolumeHealth> {
        let health = self.health.lock().map_err(lock_error)?;
        Ok(health_for_id(&health, id))
    }

    pub fn set_health(&self, id: &str, health: RemoteVolumeHealth) -> FsResult<()> {
        self.health
            .lock()
            .map_err(lock_error)?
            .insert(id.to_string(), health);
        Ok(())
    }

    pub fn set_active_ids(&self, ids: HashSet<String>) -> FsResult<Vec<RemoteVolumeConfig>> {
        let volumes = self.volumes.lock().map_err(lock_error)?;
        let next_active: HashSet<String> = ids
            .into_iter()
            .filter(|id| volumes.contains_key(id))
            .collect();
        let mut active_ids = self.active_ids.lock().map_err(lock_error)?;
        let released = active_ids
            .difference(&next_active)
            .filter_map(|id| volumes.get(id).cloned())
            .collect::<Vec<_>>();

        *active_ids = next_active;
        Ok(released)
    }

    pub fn is_active(&self, id: &str) -> FsResult<bool> {
        Ok(self.active_ids.lock().map_err(lock_error)?.contains(id))
    }

    pub fn mark_idle(&self, id: &str, message: impl Into<String>) -> FsResult<()> {
        self.set_health(
            id,
            RemoteVolumeHealth {
                status: "idle".to_string(),
                message: Some(message.into()),
                checked_at: Some(current_unix_seconds()),
            },
        )
    }

    fn cached_directory(&self, path: &RemotePath) -> FsResult<Option<Vec<FileEntry>>> {
        let key = remote_cache_key(path);
        let cache = self.cache.lock().map_err(lock_error)?;
        Ok(cache
            .directories
            .get(&key)
            .filter(|entry| entry.created_at.elapsed() < REMOTE_METADATA_CACHE_TTL)
            .map(|entry| entry.value.clone()))
    }

    fn cache_directory(&self, path: &RemotePath, entries: Vec<FileEntry>) -> FsResult<()> {
        let key = remote_cache_key(path);
        let mut cache = self.cache.lock().map_err(lock_error)?;
        cache.directories.insert(
            key,
            CachedRemoteValue {
                value: entries,
                created_at: Instant::now(),
            },
        );
        trim_remote_cache(&mut cache);
        Ok(())
    }

    fn cached_metadata(&self, path: &RemotePath) -> FsResult<Option<FileMetadata>> {
        let key = remote_cache_key(path);
        let cache = self.cache.lock().map_err(lock_error)?;
        Ok(cache
            .metadata
            .get(&key)
            .filter(|entry| entry.created_at.elapsed() < REMOTE_METADATA_CACHE_TTL)
            .map(|entry| entry.value.clone()))
    }

    fn cache_metadata(&self, path: &RemotePath, metadata: FileMetadata) -> FsResult<()> {
        let key = remote_cache_key(path);
        let mut cache = self.cache.lock().map_err(lock_error)?;
        cache.metadata.insert(
            key,
            CachedRemoteValue {
                value: metadata,
                created_at: Instant::now(),
            },
        );
        trim_remote_cache(&mut cache);
        Ok(())
    }

    pub(crate) fn invalidate_cache_for_path(&self, path: &RemotePath) -> FsResult<()> {
        let object_path = normalize_remote_object_path(&path.path);
        let path_key = remote_cache_key_parts(&path.volume_id, &object_path);
        let subtree_prefix = if object_path.is_empty() {
            format!("{}\0", path.volume_id)
        } else {
            format!("{}\0{object_path}/", path.volume_id)
        };
        let mut keys = vec![path_key];

        if let Some(parent) = remote_parent_object_path(&object_path) {
            keys.push(remote_cache_key_parts(&path.volume_id, &parent));
        }

        let mut cache = self.cache.lock().map_err(lock_error)?;
        for key in keys {
            cache.directories.remove(&key);
            cache.metadata.remove(&key);
        }
        cache
            .directories
            .retain(|key, _| !key.starts_with(&subtree_prefix));
        cache
            .metadata
            .retain(|key, _| !key.starts_with(&subtree_prefix));

        Ok(())
    }

    fn invalidate_cache_for_volume(&self, volume_id: &str) -> FsResult<()> {
        let prefix = format!("{volume_id}\0");
        let mut cache = self.cache.lock().map_err(lock_error)?;
        cache.directories.retain(|key, _| !key.starts_with(&prefix));
        cache.metadata.retain(|key, _| !key.starts_with(&prefix));
        Ok(())
    }
}

fn remote_cache_key(path: &RemotePath) -> String {
    remote_cache_key_parts(&path.volume_id, &path.path)
}

fn remote_cache_key_parts(volume_id: &str, path: &str) -> String {
    format!("{volume_id}\0{}", normalize_remote_object_path(path))
}

fn remote_parent_object_path(path: &str) -> Option<String> {
    let path = normalize_remote_object_path(path);

    if path.is_empty() {
        return None;
    }

    Path::new(&path)
        .parent()
        .map(|parent| normalize_remote_object_path(&parent.to_string_lossy()))
}

fn trim_remote_cache(cache: &mut RemoteMetadataCache) {
    trim_remote_cache_map(&mut cache.directories);
    trim_remote_cache_map(&mut cache.metadata);
}

fn trim_remote_cache_map<T>(map: &mut HashMap<String, CachedRemoteValue<T>>) {
    if map.len() <= REMOTE_METADATA_CACHE_MAX_ENTRIES {
        return;
    }

    let mut entries = map
        .iter()
        .map(|(key, value)| (key.clone(), value.created_at))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(_, created_at)| *created_at);

    let remove_count = map.len().saturating_sub(REMOTE_METADATA_CACHE_MAX_ENTRIES);
    for (key, _) in entries.into_iter().take(remove_count) {
        map.remove(&key);
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

    fn info(&self, health: RemoteVolumeHealth) -> RemoteVolumeInfo {
        RemoteVolumeInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            scheme: self.scheme.to_ascii_lowercase(),
            path: remote_root_uri(&self.id),
            root: self.root.clone(),
            capabilities: self.capabilities(),
            health,
        }
    }

    pub fn capabilities(&self) -> RemoteVolumeCapabilities {
        let scheme = self.scheme.to_ascii_lowercase();
        let is_mount_backed =
            scheme == "fs" || is_smb_scheme(&scheme) || is_password_sftp_config(self);
        let is_object_store = matches!(
            scheme.as_str(),
            "b2" | "dropbox" | "gdrive" | "memory" | "onedrive" | "s3" | "swift"
        );
        let is_webdav = scheme == "webdav";
        let is_sftp = scheme == "sftp";

        RemoteVolumeCapabilities {
            can_read: true,
            can_write: true,
            can_create_folders: !is_object_store || matches!(scheme.as_str(), "memory" | "s3"),
            can_rename: !is_object_store,
            can_delete: true,
            can_recursive_delete: true,
            can_server_side_copy: is_mount_backed
                || scheme == "s3"
                || (is_sftp
                    && self
                        .options
                        .get("enable_copy")
                        .map(|value| value.eq_ignore_ascii_case("true"))
                        .unwrap_or(false))
                || (is_webdav
                    && !self
                        .options
                        .get("disable_copy")
                        .map(|value| value.eq_ignore_ascii_case("true"))
                        .unwrap_or(false)),
            can_stream_media: true,
            can_search_filenames: true,
            can_search_content: true,
            has_posix_permissions: is_mount_backed || is_sftp,
            has_owner_group: is_mount_backed || is_sftp,
            has_symlinks: is_mount_backed,
            is_mount_backed,
            needs_mount: is_smb_scheme(&scheme) || is_password_sftp_config(self),
        }
    }

    fn operator(&self) -> FsResult<Operator> {
        self.operator_with_local_root()
            .map(|(operator, _)| operator)
    }

    fn operator_with_local_root(&self) -> FsResult<(Operator, Option<PathBuf>)> {
        self.validate()?;
        let scheme = self.scheme.to_ascii_lowercase();

        if let Some(options) = self.fs_operator_options(&scheme)? {
            let local_root = fs_root_from_options(&options);
            let operator = Operator::via_iter("fs", options).map_err(|error| {
                FsError::new(
                    "remote_operator_error",
                    format!(
                        "Unable to initialize remote volume '{}': {error}",
                        self.name
                    ),
                    Some(remote_root_uri(&self.id)),
                )
            })?;
            return Ok((operator, local_root));
        }

        let options = self.generic_operator_options();

        let operator = Operator::via_iter(scheme, options).map_err(|error| {
            FsError::new(
                "remote_operator_error",
                format!(
                    "Unable to initialize remote volume '{}': {error}",
                    self.name
                ),
                Some(remote_root_uri(&self.id)),
            )
        })?;

        Ok((operator, None))
    }

    fn fs_operator_options(&self, scheme: &str) -> FsResult<Option<Vec<(String, String)>>> {
        if is_smb_scheme(scheme) {
            return smb_fs_operator_options(self).map(Some);
        }

        if is_password_sftp_config(self) {
            return sftp_password_fs_operator_options(self).map(Some);
        }

        if scheme == "fs" {
            return Ok(Some(self.generic_operator_options()));
        }

        Ok(None)
    }

    fn generic_operator_options(&self) -> Vec<(String, String)> {
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

        options
    }
}

fn fs_root_from_options(options: &[(String, String)]) -> Option<PathBuf> {
    options
        .iter()
        .rev()
        .find(|(key, value)| key.eq_ignore_ascii_case("root") && !value.trim().is_empty())
        .map(|(_, value)| PathBuf::from(value))
}

fn health_for_id(health: &HashMap<String, RemoteVolumeHealth>, id: &str) -> RemoteVolumeHealth {
    health
        .get(id)
        .cloned()
        .unwrap_or_else(remote_health_unknown)
}

fn remote_health_unknown() -> RemoteVolumeHealth {
    RemoteVolumeHealth {
        status: "unknown".to_string(),
        message: None,
        checked_at: None,
    }
}

fn remote_health_connected() -> RemoteVolumeHealth {
    RemoteVolumeHealth {
        status: "connected".to_string(),
        message: None,
        checked_at: Some(current_unix_seconds()),
    }
}

fn remote_health_error(error: &FsError) -> RemoteVolumeHealth {
    RemoteVolumeHealth {
        status: remote_error_health_status(error).to_string(),
        message: Some(error.message.clone()),
        checked_at: Some(current_unix_seconds()),
    }
}

fn remote_error_health_status(error: &FsError) -> &'static str {
    let text = format!("{} {}", error.code, error.message).to_ascii_lowercase();

    if text.contains("auth")
        || text.contains("credential")
        || text.contains("password")
        || text.contains("permission denied")
        || text.contains("denied")
    {
        "authRequired"
    } else {
        "error"
    }
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn remote_sidebar_detail(remote: &RemoteVolumeInfo) -> String {
    let protocol = remote.scheme.to_uppercase();
    let status = match remote.health.status.as_str() {
        "connected" => "Connected",
        "idle" => "Idle",
        "checking" => "Checking",
        "authRequired" => "Auth required",
        "error" => "Offline",
        _ => "Not checked",
    };

    format!("{protocol} • {status}")
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

pub async fn check_registered_remote(
    state: &RemoteVolumeState,
    id: String,
) -> FsResult<RemoteVolumeInfo> {
    let config = state.config(&id)?;
    state.set_health(
        &id,
        RemoteVolumeHealth {
            status: "checking".to_string(),
            message: None,
            checked_at: None,
        },
    )?;

    let health = match check_remote(&config).await {
        Ok(()) => remote_health_connected(),
        Err(error) => remote_health_error(&error),
    };

    state.set_health(&id, health.clone())?;
    Ok(config.info(health))
}

pub fn release_remote_volume_resources(config: &RemoteVolumeConfig) -> RemoteReleaseResult {
    let result = if is_smb_scheme(&config.scheme) {
        unmount_smb(config)
    } else if is_password_sftp_config(config) {
        unmount_sftp_password(config)
    } else {
        Ok(false)
    };

    match result {
        Ok(released) => RemoteReleaseResult {
            id: config.id.clone(),
            released,
            message: None,
        },
        Err(error) => RemoteReleaseResult {
            id: config.id.clone(),
            released: false,
            message: Some(error.message),
        },
    }
}

pub async fn list_remote_directory(
    state: &RemoteVolumeState,
    remote_path: RemotePath,
) -> FsResult<Vec<FileEntry>> {
    if let Some(entries) = state.cached_directory(&remote_path)? {
        return Ok(entries);
    }

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
    state.cache_directory(&remote_path, result.clone())?;
    Ok(result)
}

pub async fn create_remote_folder(state: &RemoteVolumeState, path: RemotePath) -> FsResult<()> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_directory_path(&path.path);
    let result = op
        .create_dir(&object_path)
        .await
        .map_err(|error| remote_error("remote_create_dir_failed", &path, error));

    if result.is_ok() {
        state.invalidate_cache_for_path(&path)?;
    }

    result
}

pub async fn create_remote_file(state: &RemoteVolumeState, path: RemotePath) -> FsResult<()> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_object_path(&path.path);

    // Refuse to clobber an existing object (mirrors the local create_new path).
    if op.stat(&object_path).await.is_ok() {
        return Err(FsError::new(
            "remote_create_file_failed",
            "A file with that name already exists.",
            Some(format_remote_uri(&path.volume_id, &path.path)),
        ));
    }

    let result = op
        .write(&object_path, Vec::<u8>::new())
        .await
        .map(|_| ())
        .map_err(|error| remote_error("remote_create_file_failed", &path, error));

    if result.is_ok() {
        state.invalidate_cache_for_path(&path)?;
    }

    result
}

pub async fn rename_remote_item(
    state: &RemoteVolumeState,
    from: RemotePath,
    to: RemotePath,
) -> FsResult<()> {
    ensure_same_remote(&from, &to)?;
    let config = state.config(&from.volume_id)?;
    let op = config.operator()?;
    ensure_remote_directory_target_not_descendant(&op, &from, &to).await?;
    let result = op
        .rename(
            &normalize_remote_object_path(&from.path),
            &normalize_remote_object_path(&to.path),
        )
        .await
        .map_err(|error| remote_error("remote_rename_failed", &from, error));

    if result.is_ok() {
        state.invalidate_cache_for_path(&from)?;
        state.invalidate_cache_for_path(&to)?;
    }

    result
}

pub async fn delete_remote_item(state: &RemoteVolumeState, path: RemotePath) -> FsResult<()> {
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_object_path(&path.path);
    let stat = op
        .stat(&object_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &path, error))?;

    let result = if stat.is_dir() {
        op.delete_with(&normalize_remote_directory_path(&path.path))
            .recursive(true)
            .await
            .map_err(|error| remote_error("remote_delete_failed", &path, error))
    } else {
        op.delete(&object_path)
            .await
            .map_err(|error| remote_error("remote_delete_failed", &path, error))
    };

    if result.is_ok() {
        state.invalidate_cache_for_path(&path)?;
    }

    result
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
        if source.is_dir() {
            ensure_remote_target_not_descendant(&from, &to)?;
        }
        let destination_existed =
            ensure_remote_destination_available(&source_op, &from, &to, overwrite).await?;

        let result = if source.is_dir() {
            copy_remote_directory(&source_op, &from, &to).await
        } else {
            let target_path = normalize_remote_object_path(&to.path);
            let capabilities = source_op.info().full_capability();
            let copy = if !destination_existed && capabilities.copy_with_if_not_exists {
                source_op
                    .copy_with(&source_path, &target_path)
                    .if_not_exists(true)
                    .await
            } else {
                source_op.copy(&source_path, &target_path).await
            };

            match copy {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::Unsupported => {
                    copy_remote_file_path_between(
                        &source_op,
                        &source_op,
                        &from,
                        &source_path,
                        &to,
                        destination_existed && overwrite,
                    )
                    .await
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::AlreadyExists | ErrorKind::ConditionNotMatch
                    ) =>
                {
                    Err(remote_destination_exists_error(&to))
                }
                Err(error) => Err(remote_error("remote_copy_failed", &from, error)),
            }
        };

        if result.is_ok() {
            state.invalidate_cache_for_path(&to)?;
        }

        return result;
    }

    let target_config = state.config(&to.volume_id)?;
    let target_op = target_config.operator()?;
    let existed_before =
        ensure_remote_destination_available_for_type(&target_op, source.is_dir(), &to, overwrite)
            .await?;

    let result = if source.is_dir() {
        copy_remote_directory_between(&source_op, &target_op, &from, &to).await
    } else {
        copy_remote_file_path_between(
            &source_op,
            &target_op,
            &from,
            &source_path,
            &to,
            existed_before && overwrite,
        )
        .await
    };

    if result.is_ok() {
        state.invalidate_cache_for_path(&to)?;
    } else if !overwrite && !existed_before {
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

    if result.is_ok() {
        state.invalidate_cache_for_path(&to)?;
    } else if !overwrite && !existed_before {
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
    ensure_remote_directory_target_not_descendant(&op, &from, &to).await?;
    ensure_remote_destination_available(&op, &from, &to, overwrite).await?;
    let result = op
        .rename(
            &normalize_remote_object_path(&from.path),
            &normalize_remote_object_path(&to.path),
        )
        .await
        .map_err(|error| remote_error("remote_rename_failed", &from, error));

    if result.is_ok() {
        state.invalidate_cache_for_path(&from)?;
        state.invalidate_cache_for_path(&to)?;
    }

    result
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
    if let Some(metadata) = state.cached_metadata(&path)? {
        return Ok(metadata);
    }

    let config = state.config(&path.volume_id)?;
    let (op, local_root) = config.operator_with_local_root()?;
    let object_path = normalize_remote_object_path(&path.path);
    let metadata = op
        .stat(&object_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &path, error))?;
    let details = if let Some(details) = local_root
        .as_deref()
        .and_then(|root| local_metadata_for_remote_object(root, &object_path))
        .map(remote_metadata_details_from_local)
    {
        Some(details)
    } else {
        direct_sftp_metadata_details(&config, &object_path).await
    };

    let result = metadata_to_file_metadata(
        &format_remote_uri(&path.volume_id, &object_path),
        &object_path,
        &metadata,
        details.as_ref(),
    );
    state.cache_metadata(&path, result.clone())?;
    Ok(result)
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

pub(crate) async fn read_remote_file_prefix_for_search(
    state: &RemoteVolumeState,
    path: RemotePath,
    max_bytes: u64,
    checkpoint: impl FnMut() -> FsResult<()>,
) -> FsResult<RemoteFileRead> {
    read_remote_search_file_streamed(state, path, max_bytes, checkpoint).await
}

async fn read_remote_search_file_streamed(
    state: &RemoteVolumeState,
    path: RemotePath,
    max_bytes: u64,
    mut checkpoint: impl FnMut() -> FsResult<()>,
) -> FsResult<RemoteFileRead> {
    checkpoint()?;
    let config = state.config(&path.volume_id)?;
    let op = config.operator()?;
    let object_path = normalize_remote_object_path(&path.path);
    let metadata = op
        .stat(&object_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", &path, error))?;
    checkpoint()?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "preview_not_file",
            "Preview is available for files only.",
            Some(format_remote_uri(&path.volume_id, &path.path)),
        ));
    }

    let total_bytes = metadata.content_length();
    if total_bytes > max_bytes {
        return Ok(RemoteFileRead {
            bytes: Vec::new(),
            truncated: true,
            total_bytes,
        });
    }

    let read_bytes = total_bytes;
    let mut bytes = Vec::new();

    if read_bytes > 0 {
        let reader = op
            .reader_with(&object_path)
            .chunk(REMOTE_SEARCH_READ_CHUNK_BYTES)
            .await
            .map_err(|error| remote_error("remote_read_failed", &path, error))?;
        let mut stream = reader
            .into_stream(0..read_bytes)
            .await
            .map_err(|error| remote_error("remote_read_failed", &path, error))?;

        loop {
            checkpoint()?;
            let Some(chunk) = stream
                .try_next()
                .await
                .map_err(|error| remote_error("remote_read_failed", &path, error))?
            else {
                break;
            };

            for part in chunk.to_io_slice() {
                bytes.extend_from_slice(part.as_ref());
            }
            checkpoint()?;
        }

        validate_remote_read_length(
            &path.volume_id,
            &object_path,
            read_bytes,
            bytes.len() as u64,
        )?;
    }

    Ok(RemoteFileRead {
        bytes,
        truncated: false,
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
) -> FsResult<bool> {
    let object_path = normalize_remote_object_path(&to.path);
    let destination = match op.stat(&object_path).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(remote_error("remote_stat_failed", to, error)),
    };

    if !overwrite {
        return if destination.is_some() {
            Err(remote_destination_exists_error(to))
        } else {
            Ok(false)
        };
    }

    let Some(destination) = destination else {
        return Ok(false);
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

    Ok(true)
}

async fn ensure_remote_destination_available_for_type(
    op: &Operator,
    source_is_dir: bool,
    to: &RemotePath,
    overwrite: bool,
) -> FsResult<bool> {
    let object_path = normalize_remote_object_path(&to.path);
    let destination = match op.stat(&object_path).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(remote_error("remote_stat_failed", to, error)),
    };

    if !overwrite {
        return if destination.is_some() {
            Err(remote_destination_exists_error(to))
        } else {
            Ok(false)
        };
    }

    let destination_exists = destination.is_some();
    if let Some(destination) = destination {
        if source_is_dir || destination.is_dir() {
            return Err(FsError::new(
                "destination_type_conflict",
                "The existing destination has an incompatible type.",
                Some(format_remote_uri(&to.volume_id, &object_path)),
            ));
        }
    }

    Ok(destination_exists)
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
                    false,
                )
                .await?;
            }
            EntryMode::Unknown => {
                return Err(unknown_remote_entry_type_error(
                    &from.volume_id,
                    entry.path(),
                ));
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteWriteHookPoint {
    Write,
    Close,
    Commit,
}

async fn open_remote_file_writer(
    op: &Operator,
    to: &RemotePath,
    replace_existing: bool,
) -> FsResult<(RemoteWriter, Option<String>)> {
    let capabilities = op.info().full_capability();
    let destination_path = normalize_remote_object_path(&to.path);

    if !replace_existing {
        let result = if capabilities.write_with_if_not_exists {
            op.writer_with(&destination_path).if_not_exists(true).await
        } else {
            op.writer(&destination_path).await
        };

        return result
            .map(|writer| (writer, None))
            .map_err(|error| remote_write_open_error(to, error));
    }

    if !capabilities.rename {
        let detail = if capabilities.copy {
            "The provider advertises server-side copy, but OpenDAL does not guarantee that replacing an existing object by copy is atomic."
        } else {
            "The provider advertises neither remote rename nor another atomic replacement operation."
        };
        return Err(FsError::new(
            "remote_safe_overwrite_unsupported",
            format!(
                "This remote cannot safely replace an existing file. {detail} The original file was left untouched."
            ),
            Some(format_remote_uri(&to.volume_id, &to.path)),
        ));
    }

    for _ in 0..8 {
        let stage_path = remote_stage_path(&destination_path);

        if !capabilities.write_with_if_not_exists {
            match op.stat(&stage_path).await {
                Ok(_) => continue,
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(remote_error("remote_stat_failed", to, error)),
            }
        }

        let result = if capabilities.write_with_if_not_exists {
            op.writer_with(&stage_path).if_not_exists(true).await
        } else {
            op.writer(&stage_path).await
        };

        match result {
            Ok(writer) => return Ok((writer, Some(stage_path))),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::AlreadyExists | ErrorKind::ConditionNotMatch
                ) =>
            {
                continue;
            }
            Err(error) => {
                cleanup_remote_stage(op, &stage_path).await;
                return Err(remote_error("remote_write_failed", to, error));
            }
        }
    }

    Err(FsError::new(
        "remote_stage_collision",
        "Unable to reserve a unique remote staging file for the overwrite.",
        Some(format_remote_uri(&to.volume_id, &to.path)),
    ))
}

async fn finish_remote_file_write<H>(
    op: &Operator,
    mut writer: RemoteWriter,
    stage_path: Option<&str>,
    to: &RemotePath,
    expected_bytes: u64,
    hook: &mut H,
) -> FsResult<()>
where
    H: FnMut(RemoteWriteHookPoint) -> FsResult<()>,
{
    if let Err(error) = hook(RemoteWriteHookPoint::Close) {
        abort_remote_file_write(op, &mut writer, stage_path).await;
        return Err(error);
    }

    let closed = match writer.close().await {
        Ok(metadata) => metadata,
        Err(error) => {
            let _ = writer.abort().await;
            if let Some(stage_path) = stage_path {
                cleanup_remote_stage(op, stage_path).await;
            }
            return Err(remote_error("remote_write_failed", to, error));
        }
    };

    if closed.content_length() != expected_bytes {
        if let Some(stage_path) = stage_path {
            cleanup_remote_stage(op, stage_path).await;
        }
        return Err(remote_write_length_error(
            to,
            expected_bytes,
            closed.content_length(),
        ));
    }

    let written_path = stage_path.unwrap_or(to.path.as_str());
    let written_path = normalize_remote_object_path(written_path);
    let stored = match op.stat(&written_path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            if let Some(stage_path) = stage_path {
                cleanup_remote_stage(op, stage_path).await;
            }
            return Err(remote_error("remote_stat_failed", to, error));
        }
    };

    if !stored.is_file() || stored.content_length() != expected_bytes {
        if let Some(stage_path) = stage_path {
            cleanup_remote_stage(op, stage_path).await;
        }
        return Err(remote_write_length_error(
            to,
            expected_bytes,
            stored.content_length(),
        ));
    }

    if let Some(stage_path) = stage_path {
        if let Err(error) = hook(RemoteWriteHookPoint::Commit) {
            cleanup_remote_stage(op, stage_path).await;
            return Err(error);
        }

        if let Err(error) = op
            .rename(stage_path, &normalize_remote_object_path(&to.path))
            .await
        {
            cleanup_remote_stage(op, stage_path).await;
            return Err(remote_error("remote_overwrite_commit_failed", to, error));
        }
    }

    Ok(())
}

async fn abort_remote_file_write(
    op: &Operator,
    writer: &mut RemoteWriter,
    stage_path: Option<&str>,
) {
    let _ = writer.abort().await;
    if let Some(stage_path) = stage_path {
        cleanup_remote_stage(op, stage_path).await;
    }
}

async fn cleanup_remote_stage(op: &Operator, stage_path: &str) {
    let _ = op.delete(stage_path).await;
}

fn remote_stage_path(destination_path: &str) -> String {
    let token = rand::random::<[u8; 16]>();
    let token = token
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let stage_name = format!(".carelo-stage-{token}");

    match destination_path.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/{stage_name}"),
        _ => stage_name,
    }
}

fn remote_write_open_error(to: &RemotePath, error: opendal::Error) -> FsError {
    if matches!(
        error.kind(),
        ErrorKind::AlreadyExists | ErrorKind::ConditionNotMatch
    ) {
        remote_destination_exists_error(to)
    } else {
        remote_error("remote_write_failed", to, error)
    }
}

fn remote_write_length_error(to: &RemotePath, expected_bytes: u64, actual_bytes: u64) -> FsError {
    FsError::new(
        "remote_write_incomplete",
        format!(
            "Remote write length mismatch: expected {expected_bytes} bytes, stored {actual_bytes}."
        ),
        Some(format_remote_uri(&to.volume_id, &to.path)),
    )
}

async fn copy_remote_file_path_between(
    source_op: &Operator,
    target_op: &Operator,
    from: &RemotePath,
    source_path: &str,
    to: &RemotePath,
    replace_existing: bool,
) -> FsResult<()> {
    copy_remote_file_path_between_with_hook(
        source_op,
        target_op,
        from,
        source_path,
        to,
        replace_existing,
        |_| Ok(()),
    )
    .await
}

async fn copy_remote_file_path_between_with_hook<H>(
    source_op: &Operator,
    target_op: &Operator,
    from: &RemotePath,
    source_path: &str,
    to: &RemotePath,
    replace_existing: bool,
    mut hook: H,
) -> FsResult<()>
where
    H: FnMut(RemoteWriteHookPoint) -> FsResult<()>,
{
    let metadata = source_op
        .stat(source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", from, error))?;
    let reader = source_op
        .reader(source_path)
        .await
        .map_err(|error| remote_error("remote_read_failed", from, error))?;
    let (mut writer, stage_path) = open_remote_file_writer(target_op, to, replace_existing).await?;
    let total_bytes = metadata.content_length();
    let mut offset = 0_u64;

    while offset < total_bytes {
        let end = offset
            .saturating_add(TRANSFER_BUFFER_BYTES as u64)
            .min(total_bytes);
        let bytes = match reader.read(offset..end).await {
            Ok(bytes) => bytes,
            Err(error) => {
                abort_remote_file_write(target_op, &mut writer, stage_path.as_deref()).await;
                return Err(remote_error("remote_read_failed", from, error));
            }
        };
        let expected_bytes = end.saturating_sub(offset);

        if let Err(error) = validate_remote_read_length(
            &from.volume_id,
            source_path,
            expected_bytes,
            bytes.len() as u64,
        ) {
            abort_remote_file_write(target_op, &mut writer, stage_path.as_deref()).await;
            return Err(error);
        }

        if let Err(error) = hook(RemoteWriteHookPoint::Write) {
            abort_remote_file_write(target_op, &mut writer, stage_path.as_deref()).await;
            return Err(error);
        }

        if let Err(error) = writer.write(bytes).await {
            abort_remote_file_write(target_op, &mut writer, stage_path.as_deref()).await;
            return Err(remote_error("remote_write_failed", to, error));
        }

        offset = end;
    }

    finish_remote_file_write(
        target_op,
        writer,
        stage_path.as_deref(),
        to,
        total_bytes,
        &mut hook,
    )
    .await
}

async fn copy_local_path_to_remote(
    op: &Operator,
    from: &Path,
    to: &RemotePath,
    overwrite: bool,
    symlink_mode: SymlinkMode,
) -> FsResult<()> {
    let source = local_source_metadata(from, symlink_mode)?;
    let destination_existed =
        ensure_remote_destination_available_for_type(op, source.is_dir(), to, overwrite).await?;

    if source.is_dir() {
        copy_local_directory_to_remote(op, from, to, overwrite, symlink_mode).await
    } else {
        copy_local_file_to_remote(op, from, to, destination_existed && overwrite).await
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
                copy_local_file_to_remote(op, &child_path, &child_remote, false).await?;
            }
        }
    }

    Ok(())
}

async fn copy_local_file_to_remote(
    op: &Operator,
    from: &Path,
    to: &RemotePath,
    replace_existing: bool,
) -> FsResult<()> {
    copy_local_file_to_remote_with_hook(op, from, to, replace_existing, |_| Ok(())).await
}

async fn copy_local_file_to_remote_with_hook<H>(
    op: &Operator,
    from: &Path,
    to: &RemotePath,
    replace_existing: bool,
    mut hook: H,
) -> FsResult<()>
where
    H: FnMut(RemoteWriteHookPoint) -> FsResult<()>,
{
    let expected_bytes = fs::metadata(from)
        .map_err(|error| FsError::io("Unable to read source metadata", from, error))?
        .len();
    let mut reader =
        File::open(from).map_err(|error| FsError::io("Unable to open source file", from, error))?;
    let (mut writer, stage_path) = open_remote_file_writer(op, to, replace_existing).await?;
    let mut buffer = vec![0_u8; TRANSFER_BUFFER_BYTES];
    let mut written_bytes = 0_u64;

    loop {
        let bytes_read = match reader.read(&mut buffer) {
            Ok(bytes_read) => bytes_read,
            Err(error) => {
                abort_remote_file_write(op, &mut writer, stage_path.as_deref()).await;
                return Err(FsError::io("Unable to read source file", from, error));
            }
        };

        if bytes_read == 0 {
            break;
        }

        if let Err(error) = hook(RemoteWriteHookPoint::Write) {
            abort_remote_file_write(op, &mut writer, stage_path.as_deref()).await;
            return Err(error);
        }

        if let Err(error) = writer.write(buffer[..bytes_read].to_vec()).await {
            abort_remote_file_write(op, &mut writer, stage_path.as_deref()).await;
            return Err(remote_error("remote_write_failed", to, error));
        }

        written_bytes = written_bytes.saturating_add(bytes_read as u64);
    }

    if written_bytes != expected_bytes {
        abort_remote_file_write(op, &mut writer, stage_path.as_deref()).await;
        return Err(remote_write_length_error(to, expected_bytes, written_bytes));
    }

    finish_remote_file_write(
        op,
        writer,
        stage_path.as_deref(),
        to,
        expected_bytes,
        &mut hook,
    )
    .await
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
            EntryMode::Unknown => {
                return Err(unknown_remote_entry_type_error(
                    &from.volume_id,
                    entry.path(),
                ));
            }
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
    copy_remote_file_path_to_local_with_read_hook(op, from, source_path, to, overwrite, || Ok(()))
        .await
}

async fn copy_remote_file_path_to_local_with_read_hook<H>(
    op: &Operator,
    from: &RemotePath,
    source_path: &str,
    to: &Path,
    overwrite: bool,
    mut before_read: H,
) -> FsResult<()>
where
    H: FnMut() -> FsResult<()>,
{
    let metadata = op
        .stat(source_path)
        .await
        .map_err(|error| remote_error("remote_stat_failed", from, error))?;
    let reader = op
        .reader(source_path)
        .await
        .map_err(|error| remote_error("remote_read_failed", from, error))?;
    let mut writer = LocalDownloadWriter::new(to, overwrite)?;
    let total_bytes = metadata.content_length();
    let mut offset = 0_u64;

    while offset < total_bytes {
        before_read()?;
        let end = offset
            .saturating_add(TRANSFER_BUFFER_BYTES as u64)
            .min(total_bytes);
        let bytes = reader
            .read(offset..end)
            .await
            .map_err(|error| remote_error("remote_read_failed", from, error))?;
        validate_remote_read_length(
            &from.volume_id,
            source_path,
            end.saturating_sub(offset),
            bytes.len() as u64,
        )?;

        for chunk in bytes.to_io_slice() {
            writer.write_all(chunk.as_ref())?;
        }

        offset = end;
    }

    writer.finish()
}

fn validate_remote_read_length(
    volume_id: &str,
    source_path: &str,
    expected_bytes: u64,
    received_bytes: u64,
) -> FsResult<()> {
    if received_bytes == expected_bytes {
        return Ok(());
    }

    Err(FsError::new(
        "remote_read_failed",
        format!(
            "Remote file ended unexpectedly: expected {expected_bytes} bytes, received {received_bytes}."
        ),
        Some(format_remote_uri(volume_id, source_path)),
    ))
}

fn unknown_remote_entry_type_error(volume_id: &str, source_path: &str) -> FsError {
    FsError::new(
        "remote_entry_type_unknown",
        "The remote item type could not be determined, so the directory copy was stopped before it could be reported as complete.",
        Some(format_remote_uri(volume_id, source_path)),
    )
}

struct LocalDownloadWriter {
    writer: Option<File>,
    stage: Option<LocalReplacementStage>,
    destination: PathBuf,
}

impl LocalDownloadWriter {
    fn new(destination: &Path, overwrite: bool) -> FsResult<Self> {
        let stage = if overwrite {
            Some(LocalReplacementStage::new(destination)?)
        } else {
            None
        };
        let write_path = stage
            .as_ref()
            .map(LocalReplacementStage::path)
            .unwrap_or(destination);
        let writer = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(write_path)
            .map_err(|error| {
                FsError::io("Unable to create destination file", destination, error)
            })?;

        Ok(Self {
            writer: Some(writer),
            stage,
            destination: destination.to_path_buf(),
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> FsResult<()> {
        self.write_all_with(bytes, |writer, bytes| writer.write_all(bytes))
    }

    fn write_all_with<W>(&mut self, bytes: &[u8], write: W) -> FsResult<()>
    where
        W: FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
    {
        let writer = self.writer.as_mut().ok_or_else(|| {
            FsError::new(
                "destination_closed",
                "The local download destination is already closed.",
                Some(self.destination.to_string_lossy().into_owned()),
            )
        })?;

        write(writer, bytes).map_err(|error| {
            FsError::io("Unable to write destination file", &self.destination, error)
        })
    }

    fn finish(self) -> FsResult<()> {
        self.finish_with(
            |writer, staged, destination| {
                writer.flush().map_err(|error| {
                    FsError::io("Unable to flush destination file", destination, error)
                })?;

                if staged {
                    writer.sync_all().map_err(|error| {
                        FsError::io("Unable to sync destination file", destination, error)
                    })?;
                }

                Ok(())
            },
            |stage, destination| stage.commit(destination),
        )
    }

    fn finish_with<F, C>(mut self, flush: F, commit: C) -> FsResult<()>
    where
        F: FnOnce(&mut File, bool, &Path) -> FsResult<()>,
        C: FnOnce(LocalReplacementStage, &Path) -> FsResult<()>,
    {
        let staged = self.stage.is_some();
        let mut writer = self.writer.take().ok_or_else(|| {
            FsError::new(
                "destination_closed",
                "The local download destination is already closed.",
                Some(self.destination.to_string_lossy().into_owned()),
            )
        })?;

        flush(&mut writer, staged, &self.destination)?;
        drop(writer);

        if let Some(stage) = self.stage.take() {
            commit(stage, &self.destination)?;
        }

        Ok(())
    }
}

impl Drop for LocalDownloadWriter {
    fn drop(&mut self) {
        // Close the destination before the stage guard removes it. This order
        // is required on Windows when an async remote read/write returns early.
        drop(self.writer.take());
        drop(self.stage.take());
    }
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

fn remote_destination_exists_error(path: &RemotePath) -> FsError {
    FsError::new(
        "destination_exists",
        "An item already exists at the destination.",
        Some(format_remote_uri(&path.volume_id, &path.path)),
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

async fn ensure_remote_directory_target_not_descendant(
    op: &Operator,
    from: &RemotePath,
    to: &RemotePath,
) -> FsResult<()> {
    let source = op
        .stat(&normalize_remote_object_path(&from.path))
        .await
        .map_err(|error| remote_error("remote_stat_failed", from, error))?;

    if source.is_dir() {
        ensure_remote_target_not_descendant(from, to)?;
    }

    Ok(())
}

fn ensure_remote_target_not_descendant(from: &RemotePath, to: &RemotePath) -> FsResult<()> {
    if from.volume_id != to.volume_id {
        return Ok(());
    }

    let source = normalized_remote_components(&from.path);
    let target = normalized_remote_components(&to.path);
    let is_descendant = target.len() > source.len() && target.starts_with(&source);

    if !is_descendant {
        return Ok(());
    }

    Err(FsError::new(
        "destination_inside_source",
        "A remote directory cannot be copied or moved into one of its own descendants.",
        Some(format_remote_uri(&to.volume_id, &to.path)),
    ))
}

fn normalized_remote_components(path: &str) -> Vec<String> {
    let mut components = Vec::new();

    for component in normalize_remote_object_path(path).split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component.to_string()),
        }
    }

    components
}

fn local_metadata_for_remote_object(root: &Path, object_path: &str) -> Option<fs::Metadata> {
    let path = local_path_for_remote_object(root, object_path)?;
    fs::metadata(path).ok()
}

fn remote_metadata_details_from_local(metadata: fs::Metadata) -> RemoteMetadataDetails {
    RemoteMetadataDetails {
        created_at: metadata.created().ok().and_then(system_time_seconds),
        accessed_at: metadata.accessed().ok().and_then(system_time_seconds),
        is_readonly: Some(metadata.permissions().readonly()),
        permissions: permissions_for_metadata(&metadata),
    }
}

async fn direct_sftp_metadata_details(
    config: &RemoteVolumeConfig,
    object_path: &str,
) -> Option<RemoteMetadataDetails> {
    if !config.scheme.eq_ignore_ascii_case("sftp") || is_password_sftp_config(config) {
        return None;
    }

    direct_sftp_metadata(config, object_path)
        .await
        .ok()
        .map(remote_metadata_details_from_sftp)
}

// True when permission changes can be applied over the direct openssh-sftp
// client (key/agent-authenticated SFTP). Password SFTP and object stores go
// through opendal, which has no chmod equivalent.
fn is_direct_sftp_config(config: &RemoteVolumeConfig) -> bool {
    config.scheme.eq_ignore_ascii_case("sftp") && !is_password_sftp_config(config)
}

async fn connect_direct_sftp(config: &RemoteVolumeConfig) -> Result<Sftp, String> {
    let endpoint = config
        .options
        .get("endpoint")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "SFTP endpoint is not configured.".to_string())?;
    let mut session = SessionBuilder::default();

    if let Some(user) = config
        .options
        .get("user")
        .or_else(|| config.options.get("username"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        session.user(user.to_string());
    }

    if let Some(key) = config
        .options
        .get("key")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        session.keyfile(key);
    }

    session.known_hosts_check(known_hosts_strategy_for_config(config)?);

    let session = session
        .connect(endpoint)
        .await
        .map_err(|error| error.to_string())?;
    Sftp::from_session(session, SftpOptions::default())
        .await
        .map_err(|error| error.to_string())
}

async fn direct_sftp_metadata(
    config: &RemoteVolumeConfig,
    object_path: &str,
) -> Result<SftpMetadata, String> {
    let sftp = connect_direct_sftp(config).await?;
    let mut fs = sftp.fs();
    fs.metadata(sftp_metadata_path(config, object_path)?)
        .await
        .map_err(|error| error.to_string())
}

/// For a mount-backed remote (fs / SMB / password-SFTP, all accessed through a
/// local mount), resolve the object's real local path so permission changes can
/// reuse the local chmod (which supports recursion and sudo). Returns None for
/// remotes that have no local mount (e.g. key-authenticated SFTP).
pub fn remote_local_object_path(
    state: &RemoteVolumeState,
    path: &RemotePath,
) -> FsResult<Option<PathBuf>> {
    let config = state.config(&path.volume_id)?;
    let scheme = config.scheme.to_ascii_lowercase();

    let Some(options) = config.fs_operator_options(&scheme)? else {
        return Ok(None);
    };
    let Some(root) = fs_root_from_options(&options) else {
        return Ok(None);
    };

    Ok(local_path_for_remote_object(
        &root,
        &normalize_remote_object_path(&path.path),
    ))
}

/// Change POSIX permissions on a key-authenticated SFTP item via SSH_FXP_SETSTAT.
/// Non-recursive (recursive remote chmod would be one round-trip per item).
pub async fn set_remote_sftp_permissions(
    state: &RemoteVolumeState,
    path: RemotePath,
    mode: u32,
) -> FsResult<()> {
    let config = state.config(&path.volume_id)?;

    if !is_direct_sftp_config(&config) {
        return Err(FsError::new(
            "unsupported_target",
            "Editing permissions over this storage isn't supported (only local files and key-authenticated SFTP).",
            Some(format_remote_uri(&path.volume_id, &path.path)),
        ));
    }

    // Resolve the same way the read path does so the item we chmod is exactly
    // the one whose mode the dialog displayed.
    let object_path = normalize_remote_object_path(&path.path);
    let target = sftp_metadata_path(&config, &object_path).map_err(|message| {
        FsError::new(
            "remote_set_permissions_failed",
            message,
            Some(format_remote_uri(&path.volume_id, &path.path)),
        )
    })?;

    let sftp = connect_direct_sftp(&config).await.map_err(|message| {
        FsError::new(
            "remote_set_permissions_failed",
            message,
            Some(format_remote_uri(&path.volume_id, &path.path)),
        )
    })?;
    let permissions = SftpPermissions::from((mode & 0o7777) as u16);
    let result = sftp
        .fs()
        .set_permissions(target, permissions)
        .await
        .map_err(|error| {
            FsError::new(
                "remote_set_permissions_failed",
                error.to_string(),
                Some(format_remote_uri(&path.volume_id, &path.path)),
            )
        });

    if result.is_ok() {
        state.invalidate_cache_for_path(&path)?;
    }

    result
}

fn known_hosts_strategy_for_config(config: &RemoteVolumeConfig) -> Result<KnownHosts, String> {
    match config
        .options
        .get("known_hosts_strategy")
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("strict") => Ok(KnownHosts::Strict),
        Some("accept") => Ok(KnownHosts::Accept),
        Some("add") => Ok(KnownHosts::Add),
        Some(value) => Err(format!("Unknown SFTP known hosts strategy: {value}")),
    }
}

fn sftp_metadata_path(config: &RemoteVolumeConfig, object_path: &str) -> Result<PathBuf, String> {
    let root = config.root.as_deref().unwrap_or("").trim();
    let mut path = if root.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(root)
    };

    for component in Path::new(object_path).components() {
        match component {
            Component::Normal(segment) => path.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Remote path escapes its root.".to_string());
            }
        }
    }

    Ok(path)
}

fn remote_metadata_details_from_sftp(metadata: SftpMetadata) -> RemoteMetadataDetails {
    let permissions = metadata.permissions();

    RemoteMetadataDetails {
        created_at: None,
        accessed_at: metadata
            .accessed()
            .and_then(|time| system_time_seconds(time.as_system_time())),
        is_readonly: permissions.map(|permissions| permissions.readonly()),
        permissions: permissions.map(|permissions| {
            file_permissions_from_unix_mode(
                mode_from_sftp_permissions(permissions),
                metadata.uid(),
                metadata.gid(),
                false,
            )
        }),
    }
}

fn mode_from_sftp_permissions(permissions: SftpPermissions) -> u32 {
    let mut mode = 0;

    if permissions.suid() {
        mode |= 0o4000;
    }
    if permissions.sgid() {
        mode |= 0o2000;
    }
    if permissions.svtx() {
        mode |= 0o1000;
    }
    if permissions.read_by_owner() {
        mode |= 0o400;
    }
    if permissions.write_by_owner() {
        mode |= 0o200;
    }
    if permissions.execute_by_owner() {
        mode |= 0o100;
    }
    if permissions.read_by_group() {
        mode |= 0o040;
    }
    if permissions.write_by_group() {
        mode |= 0o020;
    }
    if permissions.execute_by_group() {
        mode |= 0o010;
    }
    if permissions.read_by_other() {
        mode |= 0o004;
    }
    if permissions.write_by_other() {
        mode |= 0o002;
    }
    if permissions.execute_by_other() {
        mode |= 0o001;
    }

    mode
}

fn local_path_for_remote_object(root: &Path, object_path: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();

    for component in Path::new(object_path).components() {
        match component {
            Component::Normal(segment) => path.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(path)
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

fn metadata_to_file_metadata(
    path: &str,
    object_path: &str,
    metadata: &Metadata,
    details: Option<&RemoteMetadataDetails>,
) -> FileMetadata {
    FileMetadata {
        path: path.to_string(),
        kind: entry_kind(metadata),
        size: metadata.is_file().then_some(metadata.content_length()),
        modified_at: modified_seconds(metadata),
        created_at: details.and_then(|details| details.created_at),
        accessed_at: details.and_then(|details| details.accessed_at),
        is_hidden: remote_entry_name(object_path).starts_with('.'),
        is_symlink: false,
        is_readonly: details
            .and_then(|details| details.is_readonly)
            .unwrap_or(false),
        permissions: details.and_then(|details| details.permissions.clone()),
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
        system_time_seconds(modified)
    })
}

fn system_time_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
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

    fn carelo_temporary_items(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .expect("local test directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".carelo-"))
            .map(|entry| entry.path())
            .collect()
    }

    fn assert_original_destination_and_no_stage(root: &Path, destination: &Path) {
        assert_eq!(
            fs::read(destination).expect("original destination should remain readable"),
            b"original"
        );
        assert!(
            carelo_temporary_items(root).is_empty(),
            "remote download left a Carelo temporary item"
        );
    }

    #[test]
    fn remote_overwrite_hook_failures_preserve_destination_and_clean_stage() {
        for hook_point in [
            RemoteWriteHookPoint::Write,
            RemoteWriteHookPoint::Close,
            RemoteWriteHookPoint::Commit,
        ] {
            let remote_root = TestDir::new();
            let local_root = TestDir::new();
            let source = local_root.path().join("source.txt");
            let destination = remote_root.path().join("destination.txt");
            fs::write(&source, b"replacement").expect("write local source");
            fs::write(&destination, b"original").expect("write remote destination");
            let op = fs_config("test", "Test Remote", remote_root.path())
                .operator()
                .expect("create filesystem-backed remote operator");
            let remote_destination = remote("remote://test/destination.txt");

            let error = tauri::async_runtime::block_on(copy_local_file_to_remote_with_hook(
                &op,
                &source,
                &remote_destination,
                true,
                |observed| {
                    if observed == hook_point {
                        Err(FsError::new(
                            "injected_remote_write_failure",
                            "Injected remote write failure.",
                            None,
                        ))
                    } else {
                        Ok(())
                    }
                },
            ))
            .expect_err("injected failure should abort staged overwrite");

            assert_eq!(error.code, "injected_remote_write_failure");
            assert_original_destination_and_no_stage(remote_root.path(), &destination);
        }
    }

    #[test]
    fn conditional_remote_writer_preserves_a_late_destination() {
        let remote_root = TestDir::new();
        let destination = remote_root.path().join("destination.txt");
        fs::write(&destination, b"contender").expect("write racing destination");
        let op = fs_config("test", "Test Remote", remote_root.path())
            .operator()
            .expect("create filesystem-backed remote operator");
        let remote_destination = remote("remote://test/destination.txt");

        let error = match tauri::async_runtime::block_on(open_remote_file_writer(
            &op,
            &remote_destination,
            false,
        )) {
            Ok(_) => panic!("conditional writer must not overwrite a late destination"),
            Err(error) => error,
        };

        assert_eq!(error.code, "destination_exists");
        assert_eq!(
            fs::read(&destination).expect("read racing destination"),
            b"contender"
        );
        assert!(carelo_temporary_items(remote_root.path()).is_empty());
    }

    #[test]
    fn remote_directory_copy_move_and_rename_reject_descendant_targets() {
        let state = RemoteVolumeState::default();
        let remote_root = TestDir::new();
        let source = remote_root.path().join("Source");
        fs::create_dir(&source).expect("create remote source directory");
        fs::write(source.join("file.txt"), b"data").expect("write remote source file");
        add_fs_remote(&state, remote_root.path());

        for result in [
            tauri::async_runtime::block_on(copy_remote_item(
                &state,
                remote("remote://test/Source"),
                remote("remote://test/Source/Copy"),
                false,
            )),
            tauri::async_runtime::block_on(move_remote_item(
                &state,
                remote("remote://test/Source"),
                remote("remote://test/Source/Moved"),
                false,
            )),
            tauri::async_runtime::block_on(rename_remote_item(
                &state,
                remote("remote://test/Source"),
                remote("remote://test/Source/Renamed"),
            )),
        ] {
            assert_eq!(
                result.expect_err("descendant target should fail").code,
                "destination_inside_source"
            );
        }

        assert_eq!(
            fs::read(source.join("file.txt")).expect("source remains intact"),
            b"data"
        );
        assert!(!source.join("Copy").exists());
        assert!(!source.join("Moved").exists());
        assert!(!source.join("Renamed").exists());
    }

    #[test]
    fn validates_exact_remote_ranges_and_rejects_short_reads() {
        validate_remote_read_length("test", "source.txt", 8, 8)
            .expect("an exact range should be accepted");

        let error = validate_remote_read_length("test", "source.txt", 8, 3)
            .expect_err("a short successful read must not complete a copy");

        assert_eq!(error.code, "remote_read_failed");
        assert_eq!(error.path.as_deref(), Some("remote://test/source.txt"));
        assert!(error.message.contains("expected 8 bytes, received 3"));
    }

    #[test]
    fn unknown_remote_entries_are_reported_as_copy_failures() {
        let error = unknown_remote_entry_type_error("test", "Folder/mystery");

        assert_eq!(error.code, "remote_entry_type_unknown");
        assert_eq!(error.path.as_deref(), Some("remote://test/Folder/mystery"));
    }

    #[test]
    fn remote_to_local_overwrite_commits_complete_file_and_cleans_stage() {
        let state = RemoteVolumeState::default();
        let remote_root = TestDir::new();
        let local_root = TestDir::new();
        fs::write(remote_root.path().join("source.txt"), b"replacement")
            .expect("remote source should be created");
        let destination = local_root.path().join("destination.txt");
        fs::write(&destination, b"original").expect("destination should be created");
        add_fs_remote(&state, remote_root.path());

        tauri::async_runtime::block_on(copy_remote_to_local_item(
            &state,
            remote("remote://test/source.txt"),
            &destination,
            true,
        ))
        .expect("remote overwrite should commit");

        assert_eq!(
            fs::read(&destination).expect("replacement should be readable"),
            b"replacement"
        );
        assert!(carelo_temporary_items(local_root.path()).is_empty());
    }

    #[test]
    fn remote_read_failure_after_staging_preserves_destination_and_cleans_stage() {
        let remote_root = TestDir::new();
        let local_root = TestDir::new();
        fs::write(remote_root.path().join("source.txt"), b"replacement")
            .expect("remote source should be created");
        let destination = local_root.path().join("destination.txt");
        fs::write(&destination, b"original").expect("destination should be created");
        let config = fs_config("test", "Test Remote", remote_root.path());
        let op = config.operator().expect("remote operator should build");
        let from = remote("remote://test/source.txt");

        let error = tauri::async_runtime::block_on(copy_remote_file_path_to_local_with_read_hook(
            &op,
            &from,
            "source.txt",
            &destination,
            true,
            || {
                Err(FsError::new(
                    "remote_read_failed",
                    "Injected remote read failure.",
                    Some("remote://test/source.txt".to_string()),
                ))
            },
        ))
        .expect_err("injected remote read failure should abort overwrite");

        assert_eq!(error.code, "remote_read_failed");
        assert_original_destination_and_no_stage(local_root.path(), &destination);
    }

    #[test]
    fn remote_download_write_failure_preserves_destination_and_cleans_stage() {
        let local_root = TestDir::new();
        let destination = local_root.path().join("destination.txt");
        fs::write(&destination, b"original").expect("destination should be created");
        let mut writer =
            LocalDownloadWriter::new(&destination, true).expect("download stage should open");

        let error = writer
            .write_all_with(b"replacement", |file, bytes| {
                file.write_all(&bytes[..3])?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "injected local write failure",
                ))
            })
            .expect_err("injected write failure should abort overwrite");
        drop(writer);

        assert_eq!(error.code, "io_error");
        assert_original_destination_and_no_stage(local_root.path(), &destination);
    }

    #[test]
    fn remote_download_flush_failure_preserves_destination_and_cleans_stage() {
        let local_root = TestDir::new();
        let destination = local_root.path().join("destination.txt");
        fs::write(&destination, b"original").expect("destination should be created");
        let mut writer =
            LocalDownloadWriter::new(&destination, true).expect("download stage should open");
        writer
            .write_all(b"replacement")
            .expect("replacement should be staged");

        let error = writer
            .finish_with(
                |_file, _staged, _destination| {
                    Err(FsError::new(
                        "injected_flush_failure",
                        "Injected local flush failure.",
                        None,
                    ))
                },
                |_stage, _destination| panic!("commit must not run after flush failure"),
            )
            .expect_err("injected flush failure should abort overwrite");

        assert_eq!(error.code, "injected_flush_failure");
        assert_original_destination_and_no_stage(local_root.path(), &destination);
    }

    #[test]
    fn remote_download_commit_failure_preserves_destination_and_cleans_stage() {
        let local_root = TestDir::new();
        let destination = local_root.path().join("destination.txt");
        fs::write(&destination, b"original").expect("destination should be created");
        let mut writer =
            LocalDownloadWriter::new(&destination, true).expect("download stage should open");
        writer
            .write_all(b"replacement")
            .expect("replacement should be staged");

        let error = writer
            .finish_with(
                |file, _staged, _destination| {
                    file.flush().expect("staged file should flush");
                    file.sync_all().expect("staged file should sync");
                    Ok(())
                },
                |_stage, _destination| {
                    Err(FsError::new(
                        "injected_commit_failure",
                        "Injected local commit failure.",
                        None,
                    ))
                },
            )
            .expect_err("injected commit failure should abort overwrite");

        assert_eq!(error.code, "injected_commit_failure");
        assert_original_destination_and_no_stage(local_root.path(), &destination);
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
        assert_eq!(volumes[0].health.status, "unknown");
        assert!(volumes[0].capabilities.can_read);
        assert!(volumes[0].capabilities.has_posix_permissions);
        assert!(volumes[0].capabilities.is_mount_backed);
        let volume_entries = state
            .volume_entries()
            .expect("remote volumes should convert to sidebar entries");
        assert_eq!(volume_entries[0].name, "Alpha");
        assert_eq!(
            volume_entries[0].detail.as_deref(),
            Some("FS • Not checked")
        );
        assert!(volume_entries[0].is_mounted);
        assert_eq!(
            volume_entries[0]
                .health
                .as_ref()
                .map(|health| health.status.as_str()),
            Some("unknown")
        );
        assert!(volume_entries[0]
            .capabilities
            .as_ref()
            .map(|capabilities| capabilities.can_search_content)
            .unwrap_or(false));

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
    fn tracks_remote_volumes_released_from_active_set() {
        let state = RemoteVolumeState::default();
        let root = TestDir::new();

        state
            .add(fs_config("alpha", "Alpha", root.path()))
            .expect("alpha remote should be added");
        state
            .add(fs_config("beta", "Beta", root.path()))
            .expect("beta remote should be added");

        let released = state
            .set_active_ids(HashSet::from(["alpha".to_string(), "beta".to_string()]))
            .expect("initial active set should be accepted");
        assert!(released.is_empty());

        let released = state
            .set_active_ids(HashSet::from(["beta".to_string(), "missing".to_string()]))
            .expect("updated active set should be accepted");
        assert_eq!(
            released
                .iter()
                .map(|config| config.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha"]
        );

        let released = state
            .set_active_ids(HashSet::new())
            .expect("empty active set should be accepted");
        assert_eq!(
            released
                .iter()
                .map(|config| config.id.as_str())
                .collect::<Vec<_>>(),
            vec!["beta"]
        );
    }

    #[test]
    fn lists_and_stats_fs_backed_remote_entries() {
        let state = RemoteVolumeState::default();
        let root = TestDir::new();
        fs::create_dir(root.path().join("Folder")).expect("folder should be created");
        fs::write(root.path().join("Folder").join("nested.md"), b"nested")
            .expect("nested file should be created");
        fs::write(root.path().join("alpha.txt"), b"hello").expect("file should be created");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(
                root.path().join("Folder"),
                fs::Permissions::from_mode(0o750),
            )
            .expect("folder permissions should be set");
            fs::set_permissions(
                root.path().join("alpha.txt"),
                fs::Permissions::from_mode(0o640),
            )
            .expect("file permissions should be set");
        }
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
        #[cfg(unix)]
        {
            let permissions = metadata
                .permissions
                .expect("remote fs permissions should be exposed");
            assert_eq!(permissions.octal, "640");
            assert!(permissions.owner.read);
            assert!(permissions.owner.write);
            assert!(!permissions.others.read);

            let folder_metadata = tauri::async_runtime::block_on(stat_remote_item(
                &state,
                remote("remote://test/Folder"),
            ))
            .expect("remote folder should stat");
            let folder_permissions = folder_metadata
                .permissions
                .expect("remote folder permissions should be exposed");
            assert_eq!(folder_permissions.octal, "750");
        }
        #[cfg(not(unix))]
        assert!(metadata.permissions.is_none());
    }

    #[test]
    fn invalidates_remote_directory_and_metadata_cache_after_writes() {
        let state = RemoteVolumeState::default();
        let root = TestDir::new();
        let local_root = TestDir::new();
        fs::write(root.path().join("cached.txt"), b"old").expect("remote file should be created");
        add_fs_remote(&state, root.path());

        let initial_entries =
            tauri::async_runtime::block_on(list_remote_directory(&state, remote("remote://test/")))
                .expect("remote root should list");
        assert_eq!(initial_entries.len(), 1);

        tauri::async_runtime::block_on(create_remote_folder(
            &state,
            remote("remote://test/New Folder"),
        ))
        .expect("remote folder should be created");
        let refreshed_entries =
            tauri::async_runtime::block_on(list_remote_directory(&state, remote("remote://test/")))
                .expect("remote root should refresh after create");
        assert_eq!(
            refreshed_entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["New Folder", "cached.txt"]
        );

        let initial_metadata = tauri::async_runtime::block_on(stat_remote_item(
            &state,
            remote("remote://test/cached.txt"),
        ))
        .expect("remote file should stat");
        assert_eq!(initial_metadata.size, Some(3));

        let local_file = local_root.path().join("cached.txt");
        fs::write(&local_file, b"new content").expect("local replacement should be created");
        tauri::async_runtime::block_on(copy_local_to_remote_item(
            &state,
            &local_file,
            remote("remote://test/cached.txt"),
            true,
            SymlinkMode::Preserve,
        ))
        .expect("remote file should overwrite");

        let refreshed_metadata = tauri::async_runtime::block_on(stat_remote_item(
            &state,
            remote("remote://test/cached.txt"),
        ))
        .expect("remote file metadata should refresh after overwrite");
        assert_eq!(refreshed_metadata.size, Some(11));
    }

    #[test]
    fn invalidates_remote_cache_when_config_is_replaced() {
        let state = RemoteVolumeState::default();
        let first_root = TestDir::new();
        let second_root = TestDir::new();
        fs::write(first_root.path().join("first.txt"), b"first")
            .expect("first remote file should be created");
        fs::write(second_root.path().join("second.txt"), b"second")
            .expect("second remote file should be created");

        add_fs_remote(&state, first_root.path());
        let initial_entries =
            tauri::async_runtime::block_on(list_remote_directory(&state, remote("remote://test/")))
                .expect("first remote root should list");
        assert_eq!(
            initial_entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["first.txt"]
        );

        add_fs_remote(&state, second_root.path());
        let replaced_entries =
            tauri::async_runtime::block_on(list_remote_directory(&state, remote("remote://test/")))
                .expect("replaced remote root should list");
        assert_eq!(
            replaced_entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["second.txt"]
        );
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
    fn search_stream_skips_oversized_files_and_honors_cancellation() {
        let state = RemoteVolumeState::default();
        let remote_root = TestDir::new();
        fs::write(remote_root.path().join("search.txt"), b"0123456789")
            .expect("remote search fixture should be created");
        add_fs_remote(&state, remote_root.path());

        let mut full_read_checkpoints = 0_u8;
        let full = tauri::async_runtime::block_on(read_remote_search_file_streamed(
            &state,
            remote("remote://test/search.txt"),
            10,
            || {
                full_read_checkpoints = full_read_checkpoints.saturating_add(1);
                Ok(())
            },
        ))
        .expect("remote search stream should read the complete file");
        assert_eq!(full.bytes, b"0123456789");
        assert!(!full.truncated);
        assert_eq!(full.total_bytes, 10);
        assert!(full_read_checkpoints >= 4);

        let mut checkpoints = 0_u8;
        let preview = tauri::async_runtime::block_on(read_remote_search_file_streamed(
            &state,
            remote("remote://test/search.txt"),
            9,
            || {
                checkpoints = checkpoints.saturating_add(1);
                Ok(())
            },
        ))
        .expect("oversized remote search file should be skipped");
        assert!(preview.bytes.is_empty());
        assert!(preview.truncated);
        assert_eq!(preview.total_bytes, 10);
        assert_eq!(checkpoints, 2);

        let mut cancellation_checkpoints = 0_u8;
        let error = tauri::async_runtime::block_on(read_remote_search_file_streamed(
            &state,
            remote("remote://test/search.txt"),
            10,
            || {
                cancellation_checkpoints = cancellation_checkpoints.saturating_add(1);
                if cancellation_checkpoints == 4 {
                    Err(FsError::new(
                        "operation_cancelled",
                        "Search cancelled.",
                        Some("remote://test/search.txt".to_string()),
                    ))
                } else {
                    Ok(())
                }
            },
        ))
        .expect_err("remote stream should observe cancellation after receiving data");
        assert_eq!(error.code, "operation_cancelled");
        assert_eq!(cancellation_checkpoints, 4);
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
