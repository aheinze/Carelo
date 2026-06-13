use crate::fs::local::LocalFileProvider;
use crate::fs::models::{FileEntry, FileEntryKind, FileMetadata, FsError, FsResult, VolumeEntry};
use crate::fs::provider::FileProvider;
use crate::fs::remote::{
    check_registered_remote, check_remote, copy_local_to_remote_item, copy_remote_item,
    copy_remote_to_local_item, create_remote_folder, delete_remote_item, format_remote_uri,
    list_remote_directory, materialize_remote_file,
    measure_remote_items_size as measure_remote_paths_size, move_local_to_remote_item,
    move_remote_item, move_remote_to_local_item, parse_remote_path, read_remote_file_prefix,
    release_remote_volume_resources, rename_remote_item, stat_remote_item, RemotePath,
    RemoteReleaseResult, RemoteSizeMeasure, RemoteVolumeConfig, RemoteVolumeInfo,
    RemoteVolumeState,
};
use crate::fs::sudo;
use crate::fs::{archive, operations};
use crate::open_with::{self, OpenWithContext};
use crate::store::AppStoreState;
use ignore::WalkBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use quick_xml::events::Event;
use quick_xml::Reader;
use rand::distr::{Alphanumeric, SampleString};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const MEDIA_PREVIEW_MAX_BYTES: u64 = 128 * 1024 * 1024;
const MEDIA_STREAM_MAX_ENTRIES: usize = 256;
const OFFICE_XML_PART_MAX_BYTES: u64 = 8 * 1024 * 1024;
const EXTRACTED_TEXT_MAX_BYTES: usize = 8 * 1024 * 1024;

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

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMode {
    #[default]
    Permanent,
    Trash,
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
    pub match_indices: Vec<usize>,
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
    pub match_count: usize,
    pub score: i64,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChecksum {
    pub algorithm: String,
    pub path: String,
    pub hash: String,
    pub bytes: u64,
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
    24 * 1024 * 1024
}

pub(crate) mod archives;
pub(crate) mod git;
pub(crate) mod image_tools;
pub(crate) mod pdf_tools;
pub(crate) mod preview;
pub(crate) mod remotes;
pub(crate) mod search;
pub(crate) mod size;
pub(crate) mod state;
pub(crate) mod tools;
pub(crate) mod transfer;
pub(crate) mod volumes;
pub(crate) mod watcher;

pub use archives::{archive_items, unarchive_items};
pub use git::get_git_file_info;
pub use image_tools::convert_images;
pub use pdf_tools::{compress_pdfs, run_pdf_tool};
pub use preview::{
    compare_file_checksums, compute_file_checksum, create_media_stream_url, read_media_preview,
    read_text_preview, MediaStreamState,
};
pub use remotes::{add_remote_volume, list_remote_volumes, remove_remote_volume};
pub use search::{search_content, search_files, FileSearchIndexState};
pub use size::measure_items_size;
pub use state::{
    cancel_file_operation, pause_file_operation, resume_file_operation, FileOperationState,
    SizeMeasureResult,
};
pub use tools::{
    edit_file, list_open_with_apps, open_with_app, open_with_default_app, reveal_in_file_manager,
    run_custom_tool, RemoteEditSyncState,
};
pub use transfer::{
    copy_items, create_folder, delete_items, get_file_metadata, get_home_directory, list_directory,
    move_items, rename_item, same_volume,
};
pub use volumes::{eject_volume, list_volumes, mount_volume, unlock_volume};
pub use watcher::{watch_active_directories, DirectoryWatchState};

use search::{expand_local_search_root, is_probably_binary};
use state::{
    emit_file_operation_progress, emit_file_operation_status, emit_transfer_operation_progress,
    OperationStateCleanup, ProgressSnapshot,
};

fn archive_read_only_error(path: &str) -> FsError {
    FsError::new(
        "archive_read_only",
        "Archive browsing is read-only. Copy items out of the archive instead.",
        Some(path.to_string()),
    )
}

struct TemporaryWorkspace {
    path: PathBuf,
}

impl TemporaryWorkspace {
    fn new(label: &str) -> FsResult<Self> {
        let path = std::env::temp_dir()
            .join(format!("carelo-{label}"))
            .join(format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
        fs::create_dir_all(&path)
            .map_err(|error| FsError::io("Unable to create temporary workspace", &path, error))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn unique_child_path(&self, name: &str) -> PathBuf {
        unique_child_path_in(&self.path, name)
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

async fn materialize_remote_sources(
    remotes: &RemoteVolumeState,
    paths: &[String],
    workspace: &TemporaryWorkspace,
) -> FsResult<Vec<String>> {
    let mut staged_paths = Vec::with_capacity(paths.len());

    for path in paths {
        let Some(remote_path) = parse_remote_path(path) else {
            staged_paths.push(path.clone());
            continue;
        };
        let target = workspace.unique_child_path(&remote_leaf_name(&remote_path, "remote-item"));
        copy_remote_to_local_item(remotes, remote_path, &target, true).await?;
        staged_paths.push(target.to_string_lossy().into_owned());
    }

    Ok(staged_paths)
}

fn create_persistent_temp_workspace(label: &str) -> FsResult<PathBuf> {
    let path = std::env::temp_dir()
        .join(format!("carelo-{label}"))
        .join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
    fs::create_dir_all(&path)
        .map_err(|error| FsError::io("Unable to create temporary workspace", &path, error))?;
    Ok(path)
}

fn unique_child_path_in(parent: &Path, name: &str) -> PathBuf {
    let clean_name = if name.trim().is_empty() { "item" } else { name };
    let mut candidate = parent.join(clean_name);

    if !candidate.exists() {
        return candidate;
    }

    for index in 1..1000 {
        candidate = parent.join(format!("{clean_name}-{index}"));

        if !candidate.exists() {
            return candidate;
        }
    }

    parent.join(format!("{clean_name}-{}", random_token(8)))
}

fn remote_leaf_name(path: &RemotePath, fallback: &str) -> String {
    path.path
        .trim_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn join_remote_object_path(parent: &str, child: &str) -> String {
    let parent = parent.trim().trim_matches('/');
    let child = child.trim().trim_matches('/');

    if parent.is_empty() {
        child.to_string()
    } else if child.is_empty() {
        parent.to_string()
    } else {
        format!("{parent}/{child}")
    }
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

fn random_token(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), len)
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
