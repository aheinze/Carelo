use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use tar::Archive as TarArchive;
use tar::Builder as TarBuilder;
use zip::read::ZipFile;
use zip::result::ZipError;
use zip::unstable::write::FileOptionsExt;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::fs::models::{FileEntry, FileEntryKind, FileMetadata, FsError, FsResult};

const PROGRESS_BYTE_STEP: u64 = 512 * 1024;
const DEFAULT_ARCHIVE_FORMAT: ArchiveFormat = ArchiveFormat::Zip;
const DEFAULT_COMPRESSION_LEVEL: ArchiveCompressionLevel = ArchiveCompressionLevel::Balanced;
const ARCHIVE_URI_PREFIX: &str = "archive://";
const ARCHIVE_URI_SEPARATOR: &str = "!/";

#[derive(Debug, Clone, Default)]
pub struct ArchiveProgress {
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub processed_entries: u64,
    pub total_entries: u64,
    pub current_path: Option<String>,
    pub current_bytes: u64,
    pub current_total_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct ArchiveMeasure {
    bytes: u64,
    entries: u64,
}

#[derive(Debug, Clone, Default)]
struct ProgressState {
    processed_bytes: u64,
    total_bytes: u64,
    processed_entries: u64,
    total_entries: u64,
    last_emitted_bytes: u64,
    last_emitted_current_bytes: u64,
    last_emitted_entries: u64,
    current_bytes: u64,
    current_total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePath {
    pub archive_path: PathBuf,
    pub inner_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowsableArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarZst,
}

#[derive(Debug, Clone)]
struct ArchiveEntryInfo {
    path: String,
    kind: FileEntryKind,
    size: Option<u64>,
    modified_at: Option<u64>,
    is_symlink: bool,
}

#[derive(Debug, Clone)]
struct ArchiveChildInfo {
    name: String,
    inner_path: String,
    kind: FileEntryKind,
    size: Option<u64>,
    modified_at: Option<u64>,
    is_symlink: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarZst,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveCompressionLevel {
    Fast,
    Balanced,
    Best,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOptions {
    #[serde(default = "default_archive_format")]
    pub format: ArchiveFormat,
    #[serde(default = "default_compression_level")]
    pub compression_level: ArchiveCompressionLevel,
    #[serde(default = "default_include_top_level_directory")]
    pub include_top_level_directory: bool,
    #[serde(default)]
    pub password: Option<String>,
}

impl Default for ArchiveOptions {
    fn default() -> Self {
        Self {
            format: DEFAULT_ARCHIVE_FORMAT,
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            include_top_level_directory: default_include_top_level_directory(),
            password: None,
        }
    }
}

pub fn parse_archive_uri(path: &str) -> Option<ArchivePath> {
    let raw = path.strip_prefix(ARCHIVE_URI_PREFIX)?;
    let (archive_path, inner_path) = raw.split_once(ARCHIVE_URI_SEPARATOR)?;
    let archive_path = percent_decode(archive_path)?;
    let inner_path = normalize_archive_inner_path(inner_path)?;

    if archive_path.trim().is_empty() {
        return None;
    }

    Some(ArchivePath {
        archive_path: PathBuf::from(archive_path),
        inner_path,
    })
}

pub fn archive_root_uri(path: &Path) -> String {
    format!(
        "{ARCHIVE_URI_PREFIX}{}{ARCHIVE_URI_SEPARATOR}",
        percent_encode(&path.to_string_lossy())
    )
}

pub fn format_archive_uri(path: &Path, inner_path: &str) -> String {
    let inner_path = normalize_archive_inner_path(inner_path).unwrap_or_default();
    let separator = if inner_path.is_empty() {
        ""
    } else {
        &inner_path
    };

    format!(
        "{ARCHIVE_URI_PREFIX}{}{ARCHIVE_URI_SEPARATOR}{separator}",
        percent_encode(&path.to_string_lossy())
    )
}

pub fn is_archive_uri(path: &str) -> bool {
    parse_archive_uri(path).is_some()
}

pub fn is_supported_archive_path(path: &Path) -> bool {
    archive_format_for_path(path).is_some()
}

pub fn list_archive_directory(path: &ArchivePath) -> FsResult<Vec<FileEntry>> {
    ensure_archive_file(&path.archive_path)?;

    let children = match archive_format_for_path(&path.archive_path) {
        Some(BrowsableArchiveFormat::Zip) => list_zip_directory(path)?,
        Some(BrowsableArchiveFormat::Tar) => {
            let file = open_archive_file(&path.archive_path)?;
            list_tar_directory(path, file)?
        }
        Some(BrowsableArchiveFormat::TarGz) => {
            let file = open_archive_file(&path.archive_path)?;
            let decoder = GzDecoder::new(file);
            list_tar_directory(path, decoder)?
        }
        Some(BrowsableArchiveFormat::TarZst) => {
            let file = open_archive_file(&path.archive_path)?;
            let decoder = zstd::stream::read::Decoder::new(file).map_err(|error| {
                archive_io_error("Unable to read tar.zst archive", &path.archive_path, error)
            })?;
            list_tar_directory(path, decoder)?
        }
        None => {
            return Err(FsError::new(
                "unsupported_archive_format",
                "This archive format is not supported for browsing.",
                Some(path.archive_path.to_string_lossy().into_owned()),
            ));
        }
    };

    Ok(children
        .into_values()
        .map(|child| child_to_file_entry(&path.archive_path, child))
        .collect())
}

pub fn stat_archive_entry(path: &ArchivePath) -> FsResult<FileMetadata> {
    ensure_archive_file(&path.archive_path)?;

    if path.inner_path.is_empty() {
        return Ok(FileMetadata {
            path: format_archive_uri(&path.archive_path, ""),
            kind: FileEntryKind::Directory,
            size: None,
            modified_at: archive_modified_at(&path.archive_path),
            created_at: None,
            accessed_at: None,
            is_hidden: false,
            is_symlink: false,
            is_readonly: true,
            permissions: None,
        });
    }

    let entry = archive_entry_info(path)?.ok_or_else(|| {
        FsError::new(
            "archive_entry_not_found",
            "The archive entry could not be found.",
            Some(format_archive_uri(&path.archive_path, &path.inner_path)),
        )
    })?;

    let name = archive_name_for_inner_path(&entry.path);

    Ok(FileMetadata {
        path: format_archive_uri(&path.archive_path, &entry.path),
        kind: entry.kind,
        size: entry.size,
        modified_at: entry.modified_at,
        created_at: None,
        accessed_at: None,
        is_hidden: name.starts_with('.'),
        is_symlink: entry.is_symlink,
        is_readonly: true,
        permissions: None,
    })
}

pub fn materialize_archive_file(path: &ArchivePath) -> FsResult<PathBuf> {
    let entry = archive_entry_info(path)?.ok_or_else(|| {
        FsError::new(
            "archive_entry_not_found",
            "The archive entry could not be found.",
            Some(format_archive_uri(&path.archive_path, &path.inner_path)),
        )
    })?;

    if entry.kind == FileEntryKind::Directory {
        return Err(FsError::new(
            "archive_entry_is_directory",
            "Choose a file inside the archive.",
            Some(format_archive_uri(&path.archive_path, &path.inner_path)),
        ));
    }

    let file_name = archive_name_for_inner_path(&entry.path);
    let target_directory = std::env::temp_dir()
        .join("carelo-archive-open")
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
            "Unable to create temporary archive preview directory",
            &target_directory,
            error,
        )
    })?;

    let target = target_directory.join(file_name);
    extract_archive_entry_to(path, &target, true)?;
    Ok(target)
}

pub fn extract_archive_entry_to(
    path: &ArchivePath,
    destination: &Path,
    overwrite: bool,
) -> FsResult<()> {
    ensure_archive_file(&path.archive_path)?;

    if path.inner_path.is_empty() {
        return Err(FsError::new(
            "archive_root_extract_unsupported",
            "Choose files or folders inside the archive to copy out.",
            Some(format_archive_uri(&path.archive_path, "")),
        ));
    }

    let entry = archive_entry_info(path)?.ok_or_else(|| {
        FsError::new(
            "archive_entry_not_found",
            "The archive entry could not be found.",
            Some(format_archive_uri(&path.archive_path, &path.inner_path)),
        )
    })?;

    ensure_archive_destination_available(destination, entry.kind, overwrite)?;

    match archive_format_for_path(&path.archive_path) {
        Some(BrowsableArchiveFormat::Zip) => extract_zip_entry_to(path, destination, overwrite),
        Some(BrowsableArchiveFormat::Tar) => {
            let file = open_archive_file(&path.archive_path)?;
            extract_tar_entry_to(path, file, destination, overwrite)
        }
        Some(BrowsableArchiveFormat::TarGz) => {
            let file = open_archive_file(&path.archive_path)?;
            let decoder = GzDecoder::new(file);
            extract_tar_entry_to(path, decoder, destination, overwrite)
        }
        Some(BrowsableArchiveFormat::TarZst) => {
            let file = open_archive_file(&path.archive_path)?;
            let decoder = zstd::stream::read::Decoder::new(file).map_err(|error| {
                archive_io_error("Unable to read tar.zst archive", &path.archive_path, error)
            })?;
            extract_tar_entry_to(path, decoder, destination, overwrite)
        }
        None => Err(FsError::new(
            "unsupported_archive_format",
            "This archive format is not supported for browsing.",
            Some(path.archive_path.to_string_lossy().into_owned()),
        )),
    }
}

fn default_archive_format() -> ArchiveFormat {
    DEFAULT_ARCHIVE_FORMAT
}

fn default_compression_level() -> ArchiveCompressionLevel {
    DEFAULT_COMPRESSION_LEVEL
}

fn default_include_top_level_directory() -> bool {
    true
}

fn archive_format_for_path(path: &Path) -> Option<BrowsableArchiveFormat> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();

    if name.ends_with(".zip") {
        Some(BrowsableArchiveFormat::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(BrowsableArchiveFormat::TarGz)
    } else if name.ends_with(".tar.zst") || name.ends_with(".tzst") {
        Some(BrowsableArchiveFormat::TarZst)
    } else if name.ends_with(".tar") {
        Some(BrowsableArchiveFormat::Tar)
    } else {
        None
    }
}

fn ensure_archive_file(path: &Path) -> FsResult<()> {
    let metadata = fs::metadata(path)
        .map_err(|error| FsError::io("Unable to read archive metadata", path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "archive_source_not_file",
            "Archive browsing requires a regular archive file.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    if archive_format_for_path(path).is_none() {
        return Err(FsError::new(
            "unsupported_archive_format",
            "This archive format is not supported for browsing.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

fn open_archive_file(path: &Path) -> FsResult<File> {
    File::open(path).map_err(|error| FsError::io("Unable to open archive", path, error))
}

fn archive_modified_at(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn normalize_archive_inner_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    let normalized = path.replace('\\', "/");

    for part in normalized.split('/') {
        let part = part.trim();

        if part.is_empty() || part == "." {
            continue;
        }

        if part == ".." {
            return None;
        }

        parts.push(part);
    }

    Some(parts.join("/"))
}

fn archive_name_for_inner_path(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();

    for byte in value.as_bytes() {
        let character = *byte as char;

        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~' | '/') {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }

    encoded
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push((hex_value(high)? << 4) | hex_value(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn archive_entry_info(path: &ArchivePath) -> FsResult<Option<ArchiveEntryInfo>> {
    match archive_format_for_path(&path.archive_path) {
        Some(BrowsableArchiveFormat::Zip) => zip_entry_info(path),
        Some(BrowsableArchiveFormat::Tar) => {
            let file = open_archive_file(&path.archive_path)?;
            tar_entry_info(path, file)
        }
        Some(BrowsableArchiveFormat::TarGz) => {
            let file = open_archive_file(&path.archive_path)?;
            tar_entry_info(path, GzDecoder::new(file))
        }
        Some(BrowsableArchiveFormat::TarZst) => {
            let file = open_archive_file(&path.archive_path)?;
            let decoder = zstd::stream::read::Decoder::new(file).map_err(|error| {
                archive_io_error("Unable to read tar.zst archive", &path.archive_path, error)
            })?;
            tar_entry_info(path, decoder)
        }
        None => Ok(None),
    }
}

fn list_zip_directory(path: &ArchivePath) -> FsResult<BTreeMap<String, ArchiveChildInfo>> {
    let file = open_archive_file(&path.archive_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| zip_error("Unable to read zip archive", &path.archive_path, error))?;
    let mut children = BTreeMap::new();

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            zip_error(
                "Unable to read zip archive entry",
                &path.archive_path,
                error,
            )
        })?;
        let Some(info) = zip_entry_to_info(entry)? else {
            continue;
        };

        add_direct_child(&path.archive_path, &path.inner_path, info, &mut children);
    }

    Ok(children)
}

fn list_tar_directory<R: Read>(
    path: &ArchivePath,
    reader: R,
) -> FsResult<BTreeMap<String, ArchiveChildInfo>> {
    let mut archive = TarArchive::new(reader);
    let mut children = BTreeMap::new();

    for entry in archive.entries().map_err(|error| {
        archive_io_error("Unable to read tar archive", &path.archive_path, error)
    })? {
        let entry = entry.map_err(|error| {
            archive_io_error(
                "Unable to read tar archive entry",
                &path.archive_path,
                error,
            )
        })?;
        let Some(info) = tar_entry_to_info(entry)? else {
            continue;
        };

        add_direct_child(&path.archive_path, &path.inner_path, info, &mut children);
    }

    Ok(children)
}

fn zip_entry_info(path: &ArchivePath) -> FsResult<Option<ArchiveEntryInfo>> {
    let file = open_archive_file(&path.archive_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| zip_error("Unable to read zip archive", &path.archive_path, error))?;
    let mut found_directory = None;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            zip_error(
                "Unable to read zip archive entry",
                &path.archive_path,
                error,
            )
        })?;
        let Some(info) = zip_entry_to_info(entry)? else {
            continue;
        };

        if info.path == path.inner_path {
            return Ok(Some(info));
        }

        if is_archive_child_path(&info.path, &path.inner_path) {
            found_directory = Some(ArchiveEntryInfo {
                path: path.inner_path.clone(),
                kind: FileEntryKind::Directory,
                size: None,
                modified_at: found_directory
                    .as_ref()
                    .and_then(|entry: &ArchiveEntryInfo| entry.modified_at)
                    .or(info.modified_at),
                is_symlink: false,
            });
        }
    }

    Ok(found_directory)
}

fn tar_entry_info<R: Read>(path: &ArchivePath, reader: R) -> FsResult<Option<ArchiveEntryInfo>> {
    let mut archive = TarArchive::new(reader);
    let mut found_directory = None;

    for entry in archive.entries().map_err(|error| {
        archive_io_error("Unable to read tar archive", &path.archive_path, error)
    })? {
        let entry = entry.map_err(|error| {
            archive_io_error(
                "Unable to read tar archive entry",
                &path.archive_path,
                error,
            )
        })?;
        let Some(info) = tar_entry_to_info(entry)? else {
            continue;
        };

        if info.path == path.inner_path {
            return Ok(Some(info));
        }

        if is_archive_child_path(&info.path, &path.inner_path) {
            found_directory = Some(ArchiveEntryInfo {
                path: path.inner_path.clone(),
                kind: FileEntryKind::Directory,
                size: None,
                modified_at: found_directory
                    .as_ref()
                    .and_then(|entry: &ArchiveEntryInfo| entry.modified_at)
                    .or(info.modified_at),
                is_symlink: false,
            });
        }
    }

    Ok(found_directory)
}

fn zip_entry_to_info(entry: ZipFile<'_>) -> FsResult<Option<ArchiveEntryInfo>> {
    if is_symlink_entry(&entry) {
        let Some(path) = safe_zip_entry_path(&entry)? else {
            return Ok(None);
        };

        return Ok(Some(ArchiveEntryInfo {
            path,
            kind: FileEntryKind::Symlink,
            size: None,
            modified_at: zip_modified_at(entry.last_modified()),
            is_symlink: true,
        }));
    }

    let Some(path) = safe_zip_entry_path(&entry)? else {
        return Ok(None);
    };
    let is_directory = entry.name().ends_with('/');

    if is_directory {
        return Ok(Some(ArchiveEntryInfo {
            path,
            kind: FileEntryKind::Directory,
            size: None,
            modified_at: zip_modified_at(entry.last_modified()),
            is_symlink: false,
        }));
    }

    Ok(Some(ArchiveEntryInfo {
        path,
        kind: FileEntryKind::File,
        size: Some(entry.size()),
        modified_at: zip_modified_at(entry.last_modified()),
        is_symlink: false,
    }))
}

fn tar_entry_to_info<R: Read>(entry: tar::Entry<'_, R>) -> FsResult<Option<ArchiveEntryInfo>> {
    let path = entry
        .path()
        .map_err(|error| archive_io_error("Unable to read tar entry path", Path::new(""), error))?;
    let Some(path) = normalize_archive_inner_path(&path.to_string_lossy()) else {
        return Err(FsError::new(
            "unsafe_archive_entry",
            "The archive contains an unsafe path.",
            Some(path.to_string_lossy().into_owned()),
        ));
    };

    if path.is_empty() {
        return Ok(None);
    }

    let entry_type = entry.header().entry_type();
    let kind = if entry_type.is_dir() {
        FileEntryKind::Directory
    } else if entry_type.is_symlink() {
        FileEntryKind::Symlink
    } else if entry_type.is_file() {
        FileEntryKind::File
    } else {
        FileEntryKind::Other
    };
    let size = if kind == FileEntryKind::File {
        Some(entry.size())
    } else {
        None
    };
    let modified_at = entry.header().mtime().ok();

    Ok(Some(ArchiveEntryInfo {
        path,
        kind,
        size,
        modified_at,
        is_symlink: kind == FileEntryKind::Symlink,
    }))
}

fn safe_zip_entry_path(entry: &ZipFile<'_>) -> FsResult<Option<String>> {
    let Some(path) = entry.enclosed_name() else {
        return Err(FsError::new(
            "unsafe_archive_entry",
            "The zip archive contains an unsafe path.",
            Some(entry.name().to_string()),
        ));
    };
    let Some(path) = normalize_archive_inner_path(&path.to_string_lossy()) else {
        return Err(FsError::new(
            "unsafe_archive_entry",
            "The zip archive contains an unsafe path.",
            Some(entry.name().to_string()),
        ));
    };

    if path.is_empty() {
        return Ok(None);
    }

    Ok(Some(path))
}

fn add_direct_child(
    archive_path: &Path,
    current_inner_path: &str,
    info: ArchiveEntryInfo,
    children: &mut BTreeMap<String, ArchiveChildInfo>,
) {
    let Some((name, child_inner_path, is_implicit_directory)) =
        direct_child_for(current_inner_path, &info.path)
    else {
        return;
    };
    let kind = if is_implicit_directory {
        FileEntryKind::Directory
    } else {
        info.kind
    };
    let child = ArchiveChildInfo {
        name: name.clone(),
        inner_path: child_inner_path,
        kind,
        size: if kind == FileEntryKind::File {
            info.size
        } else {
            None
        },
        modified_at: info
            .modified_at
            .or_else(|| archive_modified_at(archive_path)),
        is_symlink: !is_implicit_directory && info.is_symlink,
    };

    merge_archive_child(children, child);
}

fn direct_child_for(current_inner_path: &str, entry_path: &str) -> Option<(String, String, bool)> {
    let relative = if current_inner_path.is_empty() {
        entry_path
    } else if entry_path == current_inner_path {
        return None;
    } else {
        entry_path.strip_prefix(&format!("{current_inner_path}/"))?
    };
    let (name, rest) = relative.split_once('/').unwrap_or((relative, ""));

    if name.is_empty() {
        return None;
    }

    let child_inner_path = if current_inner_path.is_empty() {
        name.to_string()
    } else {
        format!("{current_inner_path}/{name}")
    };

    Some((name.to_string(), child_inner_path, !rest.is_empty()))
}

fn merge_archive_child(children: &mut BTreeMap<String, ArchiveChildInfo>, child: ArchiveChildInfo) {
    let key = child.name.clone();

    match children.get_mut(&key) {
        Some(existing) if existing.kind == FileEntryKind::Directory => {
            if existing.modified_at.is_none() {
                existing.modified_at = child.modified_at;
            }
        }
        Some(existing) if child.kind == FileEntryKind::Directory => {
            *existing = child;
        }
        Some(_) => {}
        None => {
            children.insert(key, child);
        }
    }
}

fn child_to_file_entry(archive_path: &Path, child: ArchiveChildInfo) -> FileEntry {
    FileEntry {
        name: child.name,
        path: format_archive_uri(archive_path, &child.inner_path),
        kind: child.kind,
        size: child.size,
        modified_at: child.modified_at,
        is_hidden: archive_name_for_inner_path(&child.inner_path).starts_with('.'),
        is_symlink: child.is_symlink,
        is_readonly: true,
        tag_color: None,
    }
}

fn is_archive_child_path(path: &str, parent: &str) -> bool {
    !parent.is_empty() && path.starts_with(&format!("{parent}/"))
}

fn ensure_archive_destination_available(
    destination: &Path,
    source_kind: FileEntryKind,
    overwrite: bool,
) -> FsResult<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if !overwrite {
                return Err(FsError::new(
                    "destination_exists",
                    "An item already exists at the destination.",
                    Some(destination.to_string_lossy().into_owned()),
                ));
            }

            if source_kind == FileEntryKind::Directory || metadata.is_dir() {
                return Err(FsError::new(
                    "destination_type_conflict",
                    "The existing destination has an incompatible type.",
                    Some(destination.to_string_lossy().into_owned()),
                ));
            }

            fs::remove_file(destination).map_err(|error| {
                FsError::io("Unable to replace existing file", destination, error)
            })?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(FsError::io(
                "Unable to read destination metadata",
                destination,
                error,
            ));
        }
    }

    Ok(())
}

fn extract_zip_entry_to(path: &ArchivePath, destination: &Path, overwrite: bool) -> FsResult<()> {
    let file = open_archive_file(&path.archive_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| zip_error("Unable to read zip archive", &path.archive_path, error))?;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            zip_error(
                "Unable to read zip archive entry",
                &path.archive_path,
                error,
            )
        })?;
        let Some(info) = zip_entry_to_info_for_extract(&entry)? else {
            continue;
        };
        let Some(relative_path) = relative_extract_path(&path.inner_path, &info.path) else {
            continue;
        };

        write_zip_extract_entry(entry, info, destination, &relative_path, overwrite)?;
    }

    Ok(())
}

fn extract_tar_entry_to<R: Read>(
    path: &ArchivePath,
    reader: R,
    destination: &Path,
    overwrite: bool,
) -> FsResult<()> {
    let mut archive = TarArchive::new(reader);

    for entry in archive.entries().map_err(|error| {
        archive_io_error("Unable to read tar archive", &path.archive_path, error)
    })? {
        let mut entry = entry.map_err(|error| {
            archive_io_error(
                "Unable to read tar archive entry",
                &path.archive_path,
                error,
            )
        })?;
        let Some(info) = tar_entry_to_info_for_extract(&entry)? else {
            continue;
        };
        let Some(relative_path) = relative_extract_path(&path.inner_path, &info.path) else {
            continue;
        };

        write_tar_extract_entry(&mut entry, info, destination, &relative_path, overwrite)?;
    }

    Ok(())
}

fn zip_entry_to_info_for_extract(entry: &ZipFile<'_>) -> FsResult<Option<ArchiveEntryInfo>> {
    if is_symlink_entry(entry) {
        return Err(FsError::new(
            "unsupported_archive_entry",
            "Archive entries that create symbolic links are not supported.",
            Some(entry.name().to_string()),
        ));
    }

    let Some(path) = safe_zip_entry_path(entry)? else {
        return Ok(None);
    };
    let kind = if entry.name().ends_with('/') {
        FileEntryKind::Directory
    } else {
        FileEntryKind::File
    };

    Ok(Some(ArchiveEntryInfo {
        path,
        kind,
        size: if kind == FileEntryKind::File {
            Some(entry.size())
        } else {
            None
        },
        modified_at: zip_modified_at(entry.last_modified()),
        is_symlink: false,
    }))
}

fn tar_entry_to_info_for_extract<R: Read>(
    entry: &tar::Entry<'_, R>,
) -> FsResult<Option<ArchiveEntryInfo>> {
    let path = entry
        .path()
        .map_err(|error| archive_io_error("Unable to read tar entry path", Path::new(""), error))?;
    let Some(path) = normalize_archive_inner_path(&path.to_string_lossy()) else {
        return Err(FsError::new(
            "unsafe_archive_entry",
            "The archive contains an unsafe path.",
            Some(path.to_string_lossy().into_owned()),
        ));
    };

    if path.is_empty() {
        return Ok(None);
    }

    let entry_type = entry.header().entry_type();

    if entry_type.is_symlink() {
        return Err(FsError::new(
            "unsupported_archive_entry",
            "Archive entries that create symbolic links are not supported.",
            Some(path),
        ));
    }

    let kind = if entry_type.is_dir() {
        FileEntryKind::Directory
    } else if entry_type.is_file() {
        FileEntryKind::File
    } else {
        FileEntryKind::Other
    };

    Ok(Some(ArchiveEntryInfo {
        path,
        kind,
        size: if kind == FileEntryKind::File {
            Some(entry.size())
        } else {
            None
        },
        modified_at: entry.header().mtime().ok(),
        is_symlink: false,
    }))
}

fn relative_extract_path(selected_path: &str, entry_path: &str) -> Option<PathBuf> {
    if entry_path == selected_path {
        return Some(PathBuf::new());
    }

    entry_path
        .strip_prefix(&format!("{selected_path}/"))
        .map(PathBuf::from)
}

fn write_zip_extract_entry(
    mut entry: ZipFile<'_>,
    info: ArchiveEntryInfo,
    destination: &Path,
    relative_path: &Path,
    overwrite: bool,
) -> FsResult<()> {
    write_archive_entry(
        &mut entry,
        info.kind,
        destination,
        relative_path,
        overwrite,
        info.size.unwrap_or(0),
    )
}

fn write_tar_extract_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    info: ArchiveEntryInfo,
    destination: &Path,
    relative_path: &Path,
    overwrite: bool,
) -> FsResult<()> {
    write_archive_entry(
        entry,
        info.kind,
        destination,
        relative_path,
        overwrite,
        info.size.unwrap_or(0),
    )
}

fn write_archive_entry<R: Read>(
    reader: &mut R,
    kind: FileEntryKind,
    destination: &Path,
    relative_path: &Path,
    overwrite: bool,
    _size: u64,
) -> FsResult<()> {
    let output_path = if relative_path.as_os_str().is_empty() {
        destination.to_path_buf()
    } else {
        destination.join(relative_path)
    };

    if kind == FileEntryKind::Directory {
        fs::create_dir_all(&output_path).map_err(|error| {
            FsError::io("Unable to create extracted directory", &output_path, error)
        })?;
        return Ok(());
    }

    if kind != FileEntryKind::File {
        return Ok(());
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            FsError::io("Unable to create extracted parent directory", parent, error)
        })?;
    }

    if overwrite && output_path.exists() {
        fs::remove_file(&output_path).map_err(|error| {
            FsError::io(
                "Unable to replace existing extracted file",
                &output_path,
                error,
            )
        })?;
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|error| FsError::io("Unable to create extracted file", &output_path, error))?;

    io::copy(reader, &mut output)
        .map(|_| ())
        .map_err(|error| FsError::io("Unable to write extracted file", &output_path, error))
}

fn zip_modified_at(datetime: zip::DateTime) -> Option<u64> {
    unix_seconds_from_ymd_hms(
        i32::from(datetime.year()),
        u32::from(datetime.month()),
        u32::from(datetime.day()),
        u32::from(datetime.hour()),
        u32::from(datetime.minute()),
        u32::from(datetime.second()),
    )
}

fn unix_seconds_from_ymd_hms(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600)?
        .checked_add(i64::from(minute) * 60)?
        .checked_add(i64::from(second))?;

    u64::try_from(seconds).ok()
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    let year = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    Some(days)
}

pub fn archive_items(paths: &[String], destination: &str, overwrite: bool) -> FsResult<()> {
    archive_items_with_options(paths, destination, overwrite, &ArchiveOptions::default())
}

pub fn archive_items_with_options(
    paths: &[String],
    destination: &str,
    overwrite: bool,
    options: &ArchiveOptions,
) -> FsResult<()> {
    archive_items_with_progress(paths, destination, overwrite, options, |_| {}, || false)
}

pub fn archive_items_with_progress<F, C>(
    paths: &[String],
    destination: &str,
    overwrite: bool,
    options: &ArchiveOptions,
    mut on_progress: F,
    mut should_cancel: C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    if paths.is_empty() {
        return Err(FsError::new(
            "archive_empty_selection",
            "Select at least one item to archive.",
            None,
        ));
    }

    let source_paths = paths
        .iter()
        .map(|path| expand_path(path))
        .collect::<FsResult<Vec<_>>>()?;
    let destination = expand_path(destination)?;
    let options = normalize_archive_options(options);

    validate_archive_options(&options, &destination)?;
    validate_archive_destination(&source_paths, &destination, overwrite, options.format)?;
    check_cancelled(&mut should_cancel, Some(&destination))?;

    let mut measure_visited_directories = HashSet::new();
    emit_progress(
        &mut ProgressState::default(),
        Some(&destination),
        true,
        &mut on_progress,
    );
    let measure = measure_paths(
        &source_paths,
        &options,
        &mut measure_visited_directories,
        &mut should_cancel,
    )?;
    if measure.entries == 0 {
        return Err(FsError::new(
            "archive_no_supported_sources",
            "No archiveable files or folders were found.",
            None,
        ));
    }

    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };
    emit_progress(&mut progress, None, true, &mut on_progress);

    let mut visited_directories = HashSet::new();

    let result = (|| -> FsResult<()> {
        match options.format {
            ArchiveFormat::Zip => {
                let archive_file = File::create(&destination).map_err(|error| {
                    FsError::io("Unable to create zip archive", &destination, error)
                })?;
                let mut zip = ZipWriter::new(archive_file);
                add_sources_to_zip_archive(
                    &mut zip,
                    &source_paths,
                    &options,
                    &mut visited_directories,
                    &mut progress,
                    &mut on_progress,
                    &mut should_cancel,
                )?;
                zip.finish().map(|_| ()).map_err(|error| {
                    zip_error("Unable to finish zip archive", &destination, error)
                })?;
            }
            ArchiveFormat::Tar => {
                let archive_file = File::create(&destination).map_err(|error| {
                    FsError::io("Unable to create tar archive", &destination, error)
                })?;
                let mut tar = TarBuilder::new(archive_file);
                add_sources_to_tar_archive(
                    &mut tar,
                    &source_paths,
                    &options,
                    &mut visited_directories,
                    &mut progress,
                    &mut on_progress,
                    &mut should_cancel,
                )?;
                finish_tar_archive(tar, &destination)?;
            }
            ArchiveFormat::TarGz => {
                let archive_file = File::create(&destination).map_err(|error| {
                    FsError::io("Unable to create tar.gz archive", &destination, error)
                })?;
                let encoder =
                    GzEncoder::new(archive_file, gzip_compression(options.compression_level));
                let mut tar = TarBuilder::new(encoder);
                add_sources_to_tar_archive(
                    &mut tar,
                    &source_paths,
                    &options,
                    &mut visited_directories,
                    &mut progress,
                    &mut on_progress,
                    &mut should_cancel,
                )?;
                finish_tar_gz_archive(tar, &destination)?;
            }
            ArchiveFormat::TarZst => {
                let archive_file = File::create(&destination).map_err(|error| {
                    FsError::io("Unable to create tar.zst archive", &destination, error)
                })?;
                let encoder = zstd::stream::write::Encoder::new(
                    archive_file,
                    zstd_compression_level(options.compression_level),
                )
                .map_err(|error| {
                    archive_io_error("Unable to start zstd encoder", &destination, error)
                })?;
                let mut tar = TarBuilder::new(encoder);
                add_sources_to_tar_archive(
                    &mut tar,
                    &source_paths,
                    &options,
                    &mut visited_directories,
                    &mut progress,
                    &mut on_progress,
                    &mut should_cancel,
                )?;
                finish_tar_zst_archive(tar, &destination)?;
            }
        }

        progress.processed_bytes = progress.total_bytes;
        progress.processed_entries = progress.total_entries;
        emit_progress(&mut progress, None, true, &mut on_progress);
        Ok(())
    })();

    if matches!(&result, Err(error) if error.code == "operation_cancelled") {
        let _ = fs::remove_file(&destination);
    }

    result
}

pub fn unarchive_items(paths: &[String], destination_directory: &str) -> FsResult<Vec<String>> {
    unarchive_items_with_progress(paths, destination_directory, |_| {}, || false)
}

pub fn unarchive_items_with_progress<F, C>(
    paths: &[String],
    destination_directory: &str,
    mut on_progress: F,
    mut should_cancel: C,
) -> FsResult<Vec<String>>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    if paths.is_empty() {
        return Err(FsError::new(
            "unarchive_empty_selection",
            "Select at least one zip archive to extract.",
            None,
        ));
    }

    let destination_directory = expand_path(destination_directory)?;

    fs::create_dir_all(&destination_directory).map_err(|error| {
        FsError::io(
            "Unable to create extraction directory",
            &destination_directory,
            error,
        )
    })?;

    let archive_paths = paths
        .iter()
        .map(|path| expand_path(path))
        .collect::<FsResult<Vec<_>>>()?;
    emit_progress(
        &mut ProgressState::default(),
        Some(&destination_directory),
        true,
        &mut on_progress,
    );
    let measure = measure_archives(&archive_paths, &mut should_cancel)?;
    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };
    emit_progress(&mut progress, None, true, &mut on_progress);

    let mut extracted_paths = Vec::new();

    for archive_path in archive_paths {
        validate_zip_source(&archive_path)?;

        let target_directory = unique_extraction_directory(&destination_directory, &archive_path)?;
        fs::create_dir(&target_directory).map_err(|error| {
            FsError::io(
                "Unable to create extraction folder",
                &target_directory,
                error,
            )
        })?;

        if let Err(error) = extract_archive_to_directory(
            &archive_path,
            &target_directory,
            &mut progress,
            &mut on_progress,
            &mut should_cancel,
        ) {
            let _ = fs::remove_dir_all(&target_directory);
            return Err(error);
        }

        extracted_paths.push(target_directory.to_string_lossy().into_owned());
    }

    progress.processed_bytes = progress.total_bytes;
    progress.processed_entries = progress.total_entries;
    emit_progress(&mut progress, None, true, &mut on_progress);
    Ok(extracted_paths)
}

fn add_sources_to_zip_archive<F, C>(
    zip: &mut ZipWriter<File>,
    source_paths: &[PathBuf],
    options: &ArchiveOptions,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    for source_path in source_paths {
        let Some(metadata) = archive_metadata(source_path)? else {
            continue;
        };

        if should_flatten_single_source_directory(source_paths, options, &metadata) {
            add_zip_directory_children(
                zip,
                source_path,
                options,
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        } else {
            let archive_name = archive_root_name(source_path)?;
            add_path_to_zip_archive(
                zip,
                source_path,
                Path::new(&archive_name),
                options,
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        }
    }

    Ok(())
}

fn add_zip_directory_children<F, C>(
    zip: &mut ZipWriter<File>,
    directory: &Path,
    options: &ArchiveOptions,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(directory))?;

    let canonical = fs::canonicalize(directory)
        .map_err(|error| FsError::io("Unable to resolve archive directory", directory, error))?;

    if !visited_directories.insert(canonical) {
        return Err(FsError::new(
            "archive_cycle",
            "Refusing to archive a directory cycle.",
            Some(directory.to_string_lossy().into_owned()),
        ));
    }

    for child in fs::read_dir(directory)
        .map_err(|error| FsError::io("Unable to read archive directory", directory, error))?
    {
        let child = child.map_err(|error| {
            FsError::io("Unable to read archive directory entry", directory, error)
        })?;
        add_path_to_zip_archive(
            zip,
            &child.path(),
            Path::new(&child.file_name()),
            options,
            visited_directories,
            progress,
            on_progress,
            should_cancel,
        )?;
    }

    Ok(())
}

fn add_path_to_zip_archive<F, C>(
    zip: &mut ZipWriter<File>,
    path: &Path,
    archive_path: &Path,
    options: &ArchiveOptions,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(path))?;

    let Some(metadata) = archive_metadata(path)? else {
        return Ok(());
    };

    if metadata.is_dir() {
        let canonical = fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive directory", path, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "archive_cycle",
                "Refusing to archive a directory cycle.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let directory_name = zip_directory_name(archive_path)?;
        zip.add_directory(
            directory_name,
            zip_options(&metadata, options.compression_level, None),
        )
        .map_err(|error| zip_error("Unable to add directory to zip archive", path, error))?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(path), true, on_progress);

        for child in fs::read_dir(path)
            .map_err(|error| FsError::io("Unable to read archive directory", path, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read archive directory entry", path, error)
            })?;
            add_path_to_zip_archive(
                zip,
                &child.path(),
                &archive_path.join(child.file_name()),
                options,
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        }

        return Ok(());
    }

    if metadata.is_file() {
        let archive_name = zip_entry_name(archive_path)?;
        let source = File::open(path)
            .map_err(|error| FsError::io("Unable to open archive source file", path, error))?;

        zip.start_file(
            archive_name,
            zip_options(
                &metadata,
                options.compression_level,
                options.password.as_deref(),
            ),
        )
        .map_err(|error| zip_error("Unable to add file to zip archive", path, error))?;
        copy_with_progress(
            source,
            zip,
            path,
            metadata.len(),
            progress,
            on_progress,
            should_cancel,
        )?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(path), true, on_progress);

        return Ok(());
    }

    Err(FsError::new(
        "unsupported_archive_source",
        "Only regular files and folders can be archived.",
        Some(path.to_string_lossy().into_owned()),
    ))
}

fn extract_archive_to_directory<F, C>(
    archive_path: &Path,
    target_directory: &Path,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    let file = File::open(archive_path)
        .map_err(|error| FsError::io("Unable to open zip archive", archive_path, error))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| zip_error("Unable to read zip archive", archive_path, error))?;

    for index in 0..archive.len() {
        check_cancelled(should_cancel, Some(archive_path))?;

        let entry = archive
            .by_index(index)
            .map_err(|error| zip_error("Unable to read zip archive entry", archive_path, error))?;

        if is_symlink_entry(&entry) {
            return Err(FsError::new(
                "unsupported_archive_entry",
                "Zip entries that create symbolic links are not supported.",
                Some(archive_path.to_string_lossy().into_owned()),
            ));
        }

        let enclosed_name = entry.enclosed_name().ok_or_else(|| {
            FsError::new(
                "unsafe_archive_entry",
                "The zip archive contains an unsafe path.",
                Some(entry.name().to_string()),
            )
        })?;
        let output_path = target_directory.join(enclosed_name);
        let mode = entry.unix_mode();

        if entry.name().ends_with('/') {
            fs::create_dir_all(&output_path).map_err(|error| {
                FsError::io("Unable to create extracted directory", &output_path, error)
            })?;
            progress.processed_entries += 1;
            emit_progress(progress, Some(&output_path), true, on_progress);
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                FsError::io("Unable to create extracted parent directory", parent, error)
            })?;
        }

        let current_total_bytes = entry.size();
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| FsError::io("Unable to create extracted file", &output_path, error))?;
        copy_with_progress(
            entry,
            &mut output,
            &output_path,
            current_total_bytes,
            progress,
            on_progress,
            should_cancel,
        )?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(&output_path), true, on_progress);

        #[cfg(unix)]
        if let Some(mode) = mode {
            let permissions = fs::Permissions::from_mode(mode & 0o777);
            fs::set_permissions(&output_path, permissions).map_err(|error| {
                FsError::io(
                    "Unable to apply extracted file permissions",
                    &output_path,
                    error,
                )
            })?;
        }
    }

    Ok(())
}

fn measure_paths<C>(
    paths: &[PathBuf],
    options: &ArchiveOptions,
    visited_directories: &mut HashSet<PathBuf>,
    should_cancel: &mut C,
) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    let mut measure = ArchiveMeasure::default();

    for path in paths {
        let Some(metadata) = archive_metadata(path)? else {
            continue;
        };

        if should_flatten_single_source_directory(paths, options, &metadata) {
            measure.add(measure_directory_children(
                path,
                visited_directories,
                should_cancel,
            )?);
        } else {
            measure.add(measure_path_with_metadata(
                path,
                metadata,
                visited_directories,
                should_cancel,
            )?);
        }
    }

    Ok(measure)
}

fn measure_directory_children<C>(
    directory: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    should_cancel: &mut C,
) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(directory))?;

    let canonical = fs::canonicalize(directory)
        .map_err(|error| FsError::io("Unable to resolve archive directory", directory, error))?;

    if !visited_directories.insert(canonical) {
        return Err(FsError::new(
            "archive_cycle",
            "Refusing to archive a directory cycle.",
            Some(directory.to_string_lossy().into_owned()),
        ));
    }

    let mut measure = ArchiveMeasure::default();

    for child in fs::read_dir(directory)
        .map_err(|error| FsError::io("Unable to read archive directory", directory, error))?
    {
        let child = child.map_err(|error| {
            FsError::io("Unable to read archive directory entry", directory, error)
        })?;
        measure.add(measure_path(
            &child.path(),
            visited_directories,
            should_cancel,
        )?);
    }

    Ok(measure)
}

fn measure_path<C>(
    path: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    should_cancel: &mut C,
) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(path))?;

    let Some(metadata) = archive_metadata(path)? else {
        return Ok(ArchiveMeasure::default());
    };

    measure_path_with_metadata(path, metadata, visited_directories, should_cancel)
}

fn measure_path_with_metadata<C>(
    path: &Path,
    metadata: fs::Metadata,
    visited_directories: &mut HashSet<PathBuf>,
    should_cancel: &mut C,
) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    if metadata.is_dir() {
        let canonical = fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive directory", path, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "archive_cycle",
                "Refusing to archive a directory cycle.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let mut measure = ArchiveMeasure {
            bytes: 0,
            entries: 1,
        };

        for child in fs::read_dir(path)
            .map_err(|error| FsError::io("Unable to read archive directory", path, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read archive directory entry", path, error)
            })?;
            measure.add(measure_path(
                &child.path(),
                visited_directories,
                should_cancel,
            )?);
        }

        return Ok(measure);
    }

    if metadata.is_file() {
        return Ok(ArchiveMeasure {
            bytes: metadata.len(),
            entries: 1,
        });
    }

    Err(FsError::new(
        "unsupported_archive_source",
        "Only regular files and folders can be archived.",
        Some(path.to_string_lossy().into_owned()),
    ))
}

fn measure_archives<C>(paths: &[PathBuf], should_cancel: &mut C) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    let mut measure = ArchiveMeasure::default();

    for path in paths {
        check_cancelled(should_cancel, Some(path))?;
        validate_zip_source(path)?;
        let file = File::open(path)
            .map_err(|error| FsError::io("Unable to open zip archive", path, error))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| zip_error("Unable to read zip archive", path, error))?;

        for index in 0..archive.len() {
            check_cancelled(should_cancel, Some(path))?;
            let entry = archive
                .by_index(index)
                .map_err(|error| zip_error("Unable to read zip archive entry", path, error))?;

            if is_symlink_entry(&entry) {
                return Err(FsError::new(
                    "unsupported_archive_entry",
                    "Zip entries that create symbolic links are not supported.",
                    Some(path.to_string_lossy().into_owned()),
                ));
            }

            if entry.enclosed_name().is_none() {
                return Err(FsError::new(
                    "unsafe_archive_entry",
                    "The zip archive contains an unsafe path.",
                    Some(entry.name().to_string()),
                ));
            }

            measure.entries += 1;

            if !entry.name().ends_with('/') {
                measure.bytes = measure.bytes.saturating_add(entry.size());
            }
        }
    }

    Ok(measure)
}

impl ArchiveMeasure {
    fn add(&mut self, other: ArchiveMeasure) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.entries = self.entries.saturating_add(other.entries);
    }
}

fn add_sources_to_tar_archive<W, F, C>(
    tar: &mut TarBuilder<W>,
    source_paths: &[PathBuf],
    options: &ArchiveOptions,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    W: Write,
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    for source_path in source_paths {
        let Some(metadata) = archive_metadata(source_path)? else {
            continue;
        };

        if should_flatten_single_source_directory(source_paths, options, &metadata) {
            add_tar_directory_children(
                tar,
                source_path,
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        } else {
            let archive_name = archive_root_name(source_path)?;
            add_path_to_tar_archive(
                tar,
                source_path,
                Path::new(&archive_name),
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        }
    }

    Ok(())
}

fn add_tar_directory_children<W, F, C>(
    tar: &mut TarBuilder<W>,
    directory: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    W: Write,
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(directory))?;

    let canonical = fs::canonicalize(directory)
        .map_err(|error| FsError::io("Unable to resolve archive directory", directory, error))?;

    if !visited_directories.insert(canonical) {
        return Err(FsError::new(
            "archive_cycle",
            "Refusing to archive a directory cycle.",
            Some(directory.to_string_lossy().into_owned()),
        ));
    }

    for child in fs::read_dir(directory)
        .map_err(|error| FsError::io("Unable to read archive directory", directory, error))?
    {
        let child = child.map_err(|error| {
            FsError::io("Unable to read archive directory entry", directory, error)
        })?;
        add_path_to_tar_archive(
            tar,
            &child.path(),
            Path::new(&child.file_name()),
            visited_directories,
            progress,
            on_progress,
            should_cancel,
        )?;
    }

    Ok(())
}

fn add_path_to_tar_archive<W, F, C>(
    tar: &mut TarBuilder<W>,
    path: &Path,
    archive_path: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    W: Write,
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(path))?;

    let Some(metadata) = archive_metadata(path)? else {
        return Ok(());
    };

    if metadata.is_dir() {
        let canonical = fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive directory", path, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "archive_cycle",
                "Refusing to archive a directory cycle.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let entry_path = tar_entry_path(archive_path)?;
        tar.append_dir(&entry_path, path).map_err(|error| {
            archive_io_error("Unable to add directory to tar archive", path, error)
        })?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(path), true, on_progress);

        for child in fs::read_dir(path)
            .map_err(|error| FsError::io("Unable to read archive directory", path, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read archive directory entry", path, error)
            })?;
            add_path_to_tar_archive(
                tar,
                &child.path(),
                &archive_path.join(child.file_name()),
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        }

        return Ok(());
    }

    if metadata.is_file() {
        let entry_path = tar_entry_path(archive_path)?;
        let source = File::open(path)
            .map_err(|error| FsError::io("Unable to open archive source file", path, error))?;
        let reader = ProgressReader {
            inner: source,
            current_path: path,
            current_total_bytes: metadata.len(),
            progress,
            on_progress,
            should_cancel,
        };
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&metadata);
        header.set_size(metadata.len());
        header.set_cksum();
        tar.append_data(&mut header, entry_path, reader)
            .map_err(|error| archive_io_error("Unable to add file to tar archive", path, error))?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(path), true, on_progress);

        return Ok(());
    }

    Err(FsError::new(
        "unsupported_archive_source",
        "Only regular files and folders can be archived.",
        Some(path.to_string_lossy().into_owned()),
    ))
}

fn archive_metadata(path: &Path) -> FsResult<Option<fs::Metadata>> {
    let symlink_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(FsError::io(
                "Unable to read archive source metadata",
                path,
                error,
            ));
        }
    };

    if !symlink_metadata.file_type().is_symlink() {
        return Ok(Some(symlink_metadata));
    }

    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(FsError::io(
            "Unable to read archive source metadata",
            path,
            error,
        )),
    }
}

fn copy_with_progress<R, W, F, C>(
    mut reader: R,
    writer: &mut W,
    current_path: &Path,
    current_total_bytes: u64,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    R: Read,
    W: Write,
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    let mut buffer = [0_u8; 64 * 1024];
    progress.current_bytes = 0;
    progress.current_total_bytes = current_total_bytes;
    emit_progress(progress, Some(current_path), true, on_progress);

    loop {
        check_cancelled(should_cancel, Some(current_path))?;
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|error| FsError::io("Unable to read archive data", current_path, error))?;

        if bytes_read == 0 {
            break;
        }

        writer
            .write_all(&buffer[..bytes_read])
            .map_err(|error| FsError::io("Unable to write archive data", current_path, error))?;
        progress.processed_bytes = progress.processed_bytes.saturating_add(bytes_read as u64);
        progress.current_bytes = progress.current_bytes.saturating_add(bytes_read as u64);
        emit_progress(progress, Some(current_path), false, on_progress);
    }

    progress.current_bytes = progress.current_total_bytes;
    emit_progress(progress, Some(current_path), true, on_progress);
    progress.current_bytes = 0;
    progress.current_total_bytes = 0;
    emit_progress(progress, Some(current_path), true, on_progress);
    Ok(())
}

fn check_cancelled<C>(should_cancel: &mut C, path: Option<&Path>) -> FsResult<()>
where
    C: FnMut() -> bool,
{
    if should_cancel() {
        return Err(FsError::new(
            "operation_cancelled",
            "The file operation was cancelled.",
            path.map(|path| path.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

struct ProgressReader<'a, R, F, C>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    inner: R,
    current_path: &'a Path,
    current_total_bytes: u64,
    progress: &'a mut ProgressState,
    on_progress: &'a mut F,
    should_cancel: &'a mut C,
}

impl<R, F, C> Read for ProgressReader<'_, R, F, C>
where
    R: Read,
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if (self.should_cancel)() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation_cancelled",
            ));
        }

        if self.progress.current_total_bytes != self.current_total_bytes {
            self.progress.current_bytes = 0;
            self.progress.current_total_bytes = self.current_total_bytes;
            emit_progress(
                self.progress,
                Some(self.current_path),
                true,
                self.on_progress,
            );
        }

        let bytes_read = self.inner.read(buffer)?;

        if bytes_read == 0 {
            self.progress.current_bytes = self.current_total_bytes;
            emit_progress(
                self.progress,
                Some(self.current_path),
                true,
                self.on_progress,
            );
            self.progress.current_bytes = 0;
            self.progress.current_total_bytes = 0;
            emit_progress(
                self.progress,
                Some(self.current_path),
                true,
                self.on_progress,
            );
            return Ok(0);
        }

        self.progress.processed_bytes = self
            .progress
            .processed_bytes
            .saturating_add(bytes_read as u64);
        self.progress.current_bytes = self
            .progress
            .current_bytes
            .saturating_add(bytes_read as u64);
        emit_progress(
            self.progress,
            Some(self.current_path),
            false,
            self.on_progress,
        );

        Ok(bytes_read)
    }
}

fn emit_progress<F>(
    state: &mut ProgressState,
    current_path: Option<&Path>,
    force: bool,
    on_progress: &mut F,
) where
    F: FnMut(ArchiveProgress),
{
    let bytes_delta = state
        .processed_bytes
        .saturating_sub(state.last_emitted_bytes);
    let current_delta = state
        .current_bytes
        .saturating_sub(state.last_emitted_current_bytes);
    let entries_changed = state.processed_entries != state.last_emitted_entries;

    if !force
        && !entries_changed
        && bytes_delta < PROGRESS_BYTE_STEP
        && current_delta < PROGRESS_BYTE_STEP
    {
        return;
    }

    state.last_emitted_bytes = state.processed_bytes;
    state.last_emitted_current_bytes = state.current_bytes;
    state.last_emitted_entries = state.processed_entries;

    on_progress(ArchiveProgress {
        processed_bytes: state.processed_bytes.min(state.total_bytes),
        total_bytes: state.total_bytes,
        processed_entries: state.processed_entries.min(state.total_entries),
        total_entries: state.total_entries,
        current_path: current_path.map(|path| path.to_string_lossy().into_owned()),
        current_bytes: state.current_bytes.min(state.current_total_bytes),
        current_total_bytes: state.current_total_bytes,
    });
}

fn normalize_archive_options(options: &ArchiveOptions) -> ArchiveOptions {
    let password = options
        .password
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    ArchiveOptions {
        format: options.format,
        compression_level: options.compression_level,
        include_top_level_directory: options.include_top_level_directory,
        password,
    }
}

fn validate_archive_options(options: &ArchiveOptions, destination: &Path) -> FsResult<()> {
    if options.password.is_some() && options.format != ArchiveFormat::Zip {
        return Err(FsError::new(
            "archive_password_unsupported",
            "Password protection is currently supported for zip archives only.",
            Some(destination.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

fn should_flatten_single_source_directory(
    source_paths: &[PathBuf],
    options: &ArchiveOptions,
    metadata: &fs::Metadata,
) -> bool {
    source_paths.len() == 1 && metadata.is_dir() && !options.include_top_level_directory
}

fn archive_root_name(source_path: &Path) -> FsResult<PathBuf> {
    let archive_name = source_path.file_name().ok_or_else(|| {
        FsError::new(
            "invalid_archive_source",
            "Unable to archive a path without a file name.",
            Some(source_path.to_string_lossy().into_owned()),
        )
    })?;

    Ok(PathBuf::from(archive_name))
}

fn archive_format_extension(format: ArchiveFormat) -> &'static str {
    match format {
        ArchiveFormat::Zip => ".zip",
        ArchiveFormat::Tar => ".tar",
        ArchiveFormat::TarGz => ".tar.gz",
        ArchiveFormat::TarZst => ".tar.zst",
    }
}

fn archive_format_label(format: ArchiveFormat) -> &'static str {
    match format {
        ArchiveFormat::Zip => "ZIP",
        ArchiveFormat::Tar => "TAR",
        ArchiveFormat::TarGz => "TAR.GZ",
        ArchiveFormat::TarZst => "TAR.ZST",
    }
}

fn zip_compression_level(level: ArchiveCompressionLevel) -> i32 {
    match level {
        ArchiveCompressionLevel::Fast => 1,
        ArchiveCompressionLevel::Balanced => 6,
        ArchiveCompressionLevel::Best => 9,
    }
}

fn gzip_compression(level: ArchiveCompressionLevel) -> Compression {
    match level {
        ArchiveCompressionLevel::Fast => Compression::fast(),
        ArchiveCompressionLevel::Balanced => Compression::default(),
        ArchiveCompressionLevel::Best => Compression::best(),
    }
}

fn zstd_compression_level(level: ArchiveCompressionLevel) -> i32 {
    match level {
        ArchiveCompressionLevel::Fast => 1,
        ArchiveCompressionLevel::Balanced => 6,
        ArchiveCompressionLevel::Best => 19,
    }
}

fn finish_tar_archive<W: Write>(mut tar: TarBuilder<W>, destination: &Path) -> FsResult<W> {
    tar.finish()
        .map_err(|error| archive_io_error("Unable to finish tar archive", destination, error))?;
    tar.into_inner()
        .map_err(|error| archive_io_error("Unable to finish tar archive", destination, error))
}

fn finish_tar_gz_archive(tar: TarBuilder<GzEncoder<File>>, destination: &Path) -> FsResult<File> {
    let encoder = finish_tar_archive(tar, destination)?;
    encoder
        .finish()
        .map_err(|error| archive_io_error("Unable to finish gzip archive", destination, error))
}

fn finish_tar_zst_archive(
    tar: TarBuilder<zstd::stream::write::Encoder<'_, File>>,
    destination: &Path,
) -> FsResult<File> {
    let encoder = finish_tar_archive(tar, destination)?;
    encoder
        .finish()
        .map_err(|error| archive_io_error("Unable to finish zstd archive", destination, error))
}

fn validate_archive_destination(
    source_paths: &[PathBuf],
    destination: &Path,
    overwrite: bool,
    format: ArchiveFormat,
) -> FsResult<()> {
    let destination_name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let expected_extension = archive_format_extension(format);

    if !destination_name.ends_with(expected_extension) {
        return Err(FsError::new(
            "invalid_archive_destination",
            format!(
                "{} archive names must end in {expected_extension}.",
                archive_format_label(format)
            ),
            Some(destination.to_string_lossy().into_owned()),
        ));
    }

    let parent = destination.parent().ok_or_else(|| {
        FsError::new(
            "invalid_archive_destination",
            "Unable to resolve the archive destination folder.",
            Some(destination.to_string_lossy().into_owned()),
        )
    })?;

    fs::create_dir_all(parent).map_err(|error| {
        FsError::io("Unable to create archive destination folder", parent, error)
    })?;

    if destination.exists() {
        if destination.is_dir() {
            return Err(FsError::new(
                "archive_destination_is_directory",
                "A folder already exists with that archive name.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }

        if !overwrite {
            return Err(FsError::new(
                "archive_destination_exists",
                "A file already exists with that archive name.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }

        fs::remove_file(destination).map_err(|error| {
            FsError::io("Unable to replace existing archive", destination, error)
        })?;
    }

    let destination_parent = fs::canonicalize(parent).map_err(|error| {
        FsError::io(
            "Unable to resolve archive destination folder",
            parent,
            error,
        )
    })?;
    let destination_name = destination.file_name().ok_or_else(|| {
        FsError::new(
            "invalid_archive_destination",
            "Unable to resolve the archive destination name.",
            Some(destination.to_string_lossy().into_owned()),
        )
    })?;
    let destination_absolute = destination_parent.join(destination_name);

    for source_path in source_paths {
        let metadata = fs::symlink_metadata(source_path).map_err(|error| {
            FsError::io("Unable to read archive source metadata", source_path, error)
        })?;
        let source_absolute = archive_source_absolute(source_path, &metadata)?;

        if source_absolute == destination_absolute {
            return Err(FsError::new(
                "archive_destination_matches_source",
                "The archive destination cannot be one of the selected files.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }

        if metadata.is_dir() && destination_absolute.starts_with(&source_absolute) {
            return Err(FsError::new(
                "archive_destination_inside_source",
                "The archive destination cannot be inside one of the selected folders.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }
    }

    Ok(())
}

fn archive_source_absolute(path: &Path, metadata: &fs::Metadata) -> FsResult<PathBuf> {
    if !metadata.file_type().is_symlink() {
        return fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive source", path, error));
    }

    let parent = path.parent().ok_or_else(|| {
        FsError::new(
            "invalid_archive_source",
            "Unable to resolve the selected item folder.",
            Some(path.to_string_lossy().into_owned()),
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        FsError::new(
            "invalid_archive_source",
            "Unable to archive a path without a file name.",
            Some(path.to_string_lossy().into_owned()),
        )
    })?;
    let parent_absolute = if parent.as_os_str().is_empty() {
        std::env::current_dir().map_err(|error| {
            FsError::new(
                "invalid_archive_source",
                format!("Unable to resolve the selected item folder: {error}"),
                Some(path.to_string_lossy().into_owned()),
            )
        })?
    } else {
        fs::canonicalize(parent).map_err(|error| {
            FsError::io("Unable to resolve archive source folder", parent, error)
        })?
    };

    Ok(parent_absolute.join(file_name))
}

fn validate_zip_source(path: &Path) -> FsResult<()> {
    let metadata = fs::metadata(path)
        .map_err(|error| FsError::io("Unable to read zip archive metadata", path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "invalid_archive_source",
            "Only zip files can be extracted.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    if extension != "zip" {
        return Err(FsError::new(
            "invalid_archive_source",
            "Only .zip archives can be extracted.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

fn unique_extraction_directory(parent: &Path, archive_path: &Path) -> FsResult<PathBuf> {
    let base_name = archive_path
        .file_stem()
        .and_then(OsStr::to_str)
        .map(safe_file_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Archive".to_string());

    for index in 1..1000 {
        let candidate_name = if index == 1 {
            base_name.clone()
        } else {
            format!("{base_name} {index}")
        };
        let candidate = parent.join(candidate_name);

        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(FsError::new(
        "extract_destination_unavailable",
        "Unable to choose a unique extraction folder.",
        Some(parent.to_string_lossy().into_owned()),
    ))
}

fn zip_options(
    metadata: &fs::Metadata,
    level: ArchiveCompressionLevel,
    password: Option<&str>,
) -> FileOptions {
    let mut options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(zip_compression_level(level)));

    if let Some(password) = password {
        options = options.with_deprecated_encryption(password.as_bytes());
    }

    #[cfg(unix)]
    {
        options.unix_permissions(metadata.permissions().mode())
    }

    #[cfg(not(unix))]
    {
        options
    }
}

fn is_symlink_entry(entry: &ZipFile<'_>) -> bool {
    entry
        .unix_mode()
        .map(|mode| mode & 0o170000 == 0o120000)
        .unwrap_or(false)
}

fn zip_entry_name(path: &Path) -> FsResult<String> {
    let mut parts = Vec::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {
                return Err(FsError::new(
                    "invalid_archive_entry_name",
                    "Unable to create a safe zip entry path.",
                    Some(path.to_string_lossy().into_owned()),
                ));
            }
        }
    }

    if parts.is_empty() {
        return Err(FsError::new(
            "invalid_archive_entry_name",
            "Unable to create a zip entry without a file name.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(parts.join("/"))
}

fn tar_entry_path(path: &Path) -> FsResult<PathBuf> {
    let mut parts = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            _ => {
                return Err(FsError::new(
                    "invalid_archive_entry_name",
                    "Unable to create a safe tar entry path.",
                    Some(path.to_string_lossy().into_owned()),
                ));
            }
        }
    }

    if parts.as_os_str().is_empty() {
        return Err(FsError::new(
            "invalid_archive_entry_name",
            "Unable to create a tar entry without a file name.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(parts)
}

fn zip_directory_name(path: &Path) -> FsResult<String> {
    let mut name = zip_entry_name(path)?;

    if !name.ends_with('/') {
        name.push('/');
    }

    Ok(name)
}

fn expand_path(path: &str) -> FsResult<PathBuf> {
    let trimmed = path.trim();

    if trimmed.is_empty() || trimmed == "~" {
        return home_dir();
    }

    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }

    Ok(PathBuf::from(trimmed))
}

fn home_dir() -> FsResult<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }

    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }

    std::env::current_dir().map_err(|error| {
        FsError::new(
            "home_not_found",
            format!("Unable to resolve a home directory: {error}"),
            None,
        )
    })
}

fn safe_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' | '\\' | '\0' => '_',
            _ => character,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

fn zip_error(action: &str, path: &Path, error: ZipError) -> FsError {
    match error {
        ZipError::Io(error) => FsError::io(action, path, error),
        _ => FsError::new(
            "archive_error",
            format!("{action}: {error}"),
            Some(path.to_string_lossy().into_owned()),
        ),
    }
}

fn archive_io_error(action: &str, path: &Path, error: io::Error) -> FsError {
    if error.kind() == io::ErrorKind::Interrupted && error.to_string() == "operation_cancelled" {
        return FsError::new(
            "operation_cancelled",
            "The file operation was cancelled.",
            Some(path.to_string_lossy().into_owned()),
        );
    }

    FsError::io(action, path, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("carelo-archive-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn archives_and_extracts_selected_items() {
        let root = test_root("round-trip");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directories");
        fs::write(root.join("note.txt"), "hello").expect("write source file");
        fs::write(root.join("folder").join("nested.txt"), "nested").expect("write nested file");

        let archive_path = root.join("bundle.zip");
        archive_items(
            &[
                root.join("note.txt").to_string_lossy().into_owned(),
                root.join("folder").to_string_lossy().into_owned(),
            ],
            &archive_path.to_string_lossy(),
            false,
        )
        .expect("create archive");

        let output_root = root.join("out");
        let extracted = unarchive_items(
            &[archive_path.to_string_lossy().into_owned()],
            &output_root.to_string_lossy(),
        )
        .expect("extract archive");

        let extracted_root = PathBuf::from(&extracted[0]);
        assert_eq!(
            fs::read_to_string(extracted_root.join("note.txt")).expect("read extracted file"),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(extracted_root.join("folder").join("nested.txt"))
                .expect("read nested extracted file"),
            "nested"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn skips_broken_symlinks_inside_archived_folders() {
        let root = test_root("broken-symlink");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directories");
        fs::write(root.join("folder").join("note.txt"), "hello").expect("write regular file");
        std::os::unix::fs::symlink("missing.txt", root.join("folder").join("broken-link"))
            .expect("create broken symlink");

        let archive_path = root.join("links.zip");
        archive_items(
            &[root.join("folder").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
        )
        .expect("create archive with broken symlink");

        let archive_file = File::open(&archive_path).expect("open archive");
        let mut archive = ZipArchive::new(archive_file).expect("read archive");
        assert!(archive.by_name("folder/").is_ok());
        assert!(archive.by_name("folder/note.txt").is_ok());
        assert!(archive.by_name("folder/broken-link").is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unsafe_extraction_paths() {
        let root = test_root("unsafe-path");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");

        let archive_path = root.join("unsafe.zip");
        let archive_file = File::create(&archive_path).expect("create unsafe archive");
        let mut archive = ZipWriter::new(archive_file);
        archive
            .start_file("../escape.txt", FileOptions::default())
            .expect("add unsafe entry");
        archive.write_all(b"bad").expect("write unsafe entry");
        archive.finish().expect("finish unsafe archive");

        let error = unarchive_items(
            &[archive_path.to_string_lossy().into_owned()],
            &root.join("out").to_string_lossy(),
        )
        .expect_err("unsafe archive should be rejected");

        assert_eq!(error.code, "unsafe_archive_entry");
        assert!(!root.join("escape.txt").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn creates_tar_gz_without_top_level_directory() {
        let root = test_root("tar-gz-flat");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directory");
        fs::write(root.join("folder").join("nested.txt"), "nested").expect("write nested file");

        let archive_path = root.join("bundle.tar.gz");
        archive_items_with_options(
            &[root.join("folder").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
            &ArchiveOptions {
                format: ArchiveFormat::TarGz,
                include_top_level_directory: false,
                ..ArchiveOptions::default()
            },
        )
        .expect("create tar.gz archive");

        let archive_file = File::open(&archive_path).expect("open tar.gz archive");
        let decoder = flate2::read::GzDecoder::new(archive_file);
        let mut archive = tar::Archive::new(decoder);
        let names = archive
            .entries()
            .expect("read tar entries")
            .map(|entry| {
                entry
                    .expect("read tar entry")
                    .path()
                    .expect("read tar entry path")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();

        assert!(names.iter().any(|name| name == "nested.txt"));
        assert!(!names.iter().any(|name| name == "folder/nested.txt"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn browses_zip_archive_and_copies_entries_out() {
        let root = test_root("zip-browse");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directory");
        fs::write(root.join("folder").join("nested.txt"), "nested").expect("write nested file");
        fs::write(root.join("note.txt"), "hello").expect("write source file");

        let archive_path = root.join("browse.zip");
        archive_items(
            &[
                root.join("folder").to_string_lossy().into_owned(),
                root.join("note.txt").to_string_lossy().into_owned(),
            ],
            &archive_path.to_string_lossy(),
            false,
        )
        .expect("create archive");

        let archive_root =
            parse_archive_uri(&archive_root_uri(&archive_path)).expect("parse archive root uri");
        let root_entries = list_archive_directory(&archive_root).expect("list archive root");
        let root_names = root_entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.kind))
            .collect::<Vec<_>>();
        assert!(root_names.contains(&("folder", FileEntryKind::Directory)));
        assert!(root_names.contains(&("note.txt", FileEntryKind::File)));

        let folder_path = parse_archive_uri(
            &root_entries
                .iter()
                .find(|entry| entry.name == "folder")
                .expect("find folder")
                .path,
        )
        .expect("parse folder archive uri");
        let folder_entries = list_archive_directory(&folder_path).expect("list folder in archive");
        assert_eq!(folder_entries.len(), 1);
        assert_eq!(folder_entries[0].name, "nested.txt");
        assert_eq!(folder_entries[0].kind, FileEntryKind::File);

        let nested_path = parse_archive_uri(&folder_entries[0].path).expect("parse nested uri");
        let metadata = stat_archive_entry(&nested_path).expect("stat nested file");
        assert_eq!(metadata.kind, FileEntryKind::File);
        assert_eq!(metadata.size, Some(6));
        assert!(metadata.is_readonly);

        let copied_path = root.join("copied.txt");
        extract_archive_entry_to(&nested_path, &copied_path, false).expect("copy entry out");
        assert_eq!(
            fs::read_to_string(copied_path).expect("read copied file"),
            "nested"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn browses_tar_gz_archive_directories() {
        let root = test_root("tar-gz-browse");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directory");
        fs::write(root.join("folder").join("nested.txt"), "nested").expect("write nested file");

        let archive_path = root.join("browse.tar.gz");
        archive_items_with_options(
            &[root.join("folder").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
            &ArchiveOptions {
                format: ArchiveFormat::TarGz,
                ..ArchiveOptions::default()
            },
        )
        .expect("create tar.gz archive");

        let archive_root =
            parse_archive_uri(&archive_root_uri(&archive_path)).expect("parse archive root uri");
        let root_entries = list_archive_directory(&archive_root).expect("list tar.gz root");
        let folder = root_entries
            .iter()
            .find(|entry| entry.name == "folder")
            .expect("find folder");
        assert_eq!(folder.kind, FileEntryKind::Directory);

        let folder_path = parse_archive_uri(&folder.path).expect("parse folder uri");
        let folder_entries = list_archive_directory(&folder_path).expect("list tar.gz folder");
        assert!(folder_entries
            .iter()
            .any(|entry| entry.name == "nested.txt" && entry.kind == FileEntryKind::File));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn creates_password_protected_zip() {
        let root = test_root("zip-password");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        fs::write(root.join("secret.txt"), "classified").expect("write source file");

        let archive_path = root.join("protected.zip");
        archive_items_with_options(
            &[root.join("secret.txt").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
            &ArchiveOptions {
                password: Some("open sesame".to_string()),
                ..ArchiveOptions::default()
            },
        )
        .expect("create password zip archive");

        let archive_file = File::open(&archive_path).expect("open protected archive");
        let mut archive = ZipArchive::new(archive_file).expect("read protected archive");
        let mut entry = archive
            .by_name_decrypt("secret.txt", b"open sesame")
            .expect("find encrypted entry")
            .expect("decrypt encrypted entry");
        let mut contents = String::new();
        entry
            .read_to_string(&mut contents)
            .expect("read decrypted entry");

        assert_eq!(contents, "classified");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancels_archive_before_creating_output() {
        let root = test_root("cancel");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        fs::write(root.join("note.txt"), "hello").expect("write source file");

        let archive_path = root.join("cancelled.zip");
        let error = archive_items_with_progress(
            &[root.join("note.txt").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
            &ArchiveOptions::default(),
            |_| {},
            || true,
        )
        .expect_err("cancelled archive should fail");

        assert_eq!(error.code, "operation_cancelled");
        assert!(!archive_path.exists());

        let _ = fs::remove_dir_all(&root);
    }
}
