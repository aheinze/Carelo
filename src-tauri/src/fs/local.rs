use std::cmp::Ordering;
#[cfg(unix)]
use std::ffi::CStr;
use std::ffi::OsStr;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fs::models::{
    FileEntry, FileEntryKind, FileMetadata, FilePermissions, FsError, FsResult, PermissionSet,
};
use crate::fs::provider::FileProvider;

#[derive(Debug, Default)]
pub struct LocalFileProvider;

impl LocalFileProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn home_dir() -> FsResult<PathBuf> {
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

    fn expand_path(&self, path: &str) -> FsResult<PathBuf> {
        let trimmed = path.trim();

        if trimmed.is_empty() {
            return Self::home_dir();
        }

        if trimmed == "~" {
            return Self::home_dir();
        }

        if let Some(rest) = trimmed.strip_prefix("~/") {
            return Ok(Self::home_dir()?.join(rest));
        }

        Ok(PathBuf::from(trimmed))
    }

    fn entry_from_path(path: &Path) -> FsResult<FileEntry> {
        let symlink_metadata = fs::symlink_metadata(path)
            .map_err(|error| FsError::io("Unable to read file metadata", path, error))?;
        let is_symlink = symlink_metadata.file_type().is_symlink();
        let metadata = if is_symlink {
            fs::metadata(path).unwrap_or(symlink_metadata)
        } else {
            symlink_metadata
        };

        let kind = if metadata.is_dir() {
            FileEntryKind::Directory
        } else if metadata.is_file() {
            FileEntryKind::File
        } else if is_symlink {
            FileEntryKind::Symlink
        } else {
            FileEntryKind::Other
        };

        let name = path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy()
            .into_owned();
        let is_hidden = name.starts_with('.');
        let size = if metadata.is_file() {
            Some(metadata.len())
        } else {
            None
        };

        Ok(FileEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            kind,
            size,
            modified_at: modified_seconds(&metadata),
            is_hidden,
            is_symlink,
            is_readonly: metadata.permissions().readonly(),
            tag_color: None,
        })
    }

    fn copy_recursive(from: &Path, to: &Path) -> FsResult<()> {
        let metadata = fs::metadata(from)
            .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

        if metadata.is_dir() {
            fs::create_dir_all(to).map_err(|error| {
                FsError::io("Unable to create destination directory", to, error)
            })?;

            for child in fs::read_dir(from)
                .map_err(|error| FsError::io("Unable to read source directory", from, error))?
            {
                let child = child.map_err(|error| {
                    FsError::io("Unable to read source directory entry", from, error)
                })?;
                Self::copy_recursive(&child.path(), &to.join(child.file_name()))?;
            }

            return Ok(());
        }

        fs::copy(from, to)
            .map(|_| ())
            .map_err(|error| FsError::io("Unable to copy file", from, error))
    }

    fn delete_path(path: &Path) -> FsResult<()> {
        let metadata = fs::metadata(path)
            .map_err(|error| FsError::io("Unable to read item before delete", path, error))?;

        if metadata.is_dir() {
            fs::remove_dir_all(path)
                .map_err(|error| FsError::io("Unable to delete directory", path, error))
        } else {
            fs::remove_file(path).map_err(|error| FsError::io("Unable to delete file", path, error))
        }
    }

    fn cleanup_partial_copy(path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };

        let _ = if metadata.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
    }

    fn temporary_move_path(to: &Path) -> FsResult<PathBuf> {
        let parent = to.parent().unwrap_or_else(|| Path::new("."));

        for attempt in 0..100 {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let candidate = parent.join(format!(
                ".carelo-move-{}-{nonce}-{attempt}.tmp",
                std::process::id()
            ));

            if !candidate.exists() {
                return Ok(candidate);
            }
        }

        Err(FsError::new(
            "temporary_path_unavailable",
            "Unable to reserve a temporary destination for the move.",
            Some(to.to_string_lossy().into_owned()),
        ))
    }
}

impl FileProvider for LocalFileProvider {
    fn list(&self, path: &str) -> FsResult<Vec<FileEntry>> {
        let directory = self.expand_path(path)?;
        let metadata = fs::metadata(&directory)
            .map_err(|error| FsError::io("Unable to read directory metadata", &directory, error))?;

        if !metadata.is_dir() {
            return Err(FsError::new(
                "not_directory",
                "Path is not a directory.",
                Some(directory.to_string_lossy().into_owned()),
            ));
        }

        let mut entries = Vec::new();
        let read_dir = fs::read_dir(&directory)
            .map_err(|error| FsError::io("Unable to list directory", &directory, error))?;

        for entry in read_dir {
            let entry = entry.map_err(|error| {
                FsError::io("Unable to read directory entry", &directory, error)
            })?;
            entries.push(Self::entry_from_path(&entry.path())?);
        }

        entries.sort_by(compare_entries);
        Ok(entries)
    }

    fn stat(&self, path: &str) -> FsResult<FileMetadata> {
        let path = self.expand_path(path)?;
        let entry = Self::entry_from_path(&path)?;
        let symlink_metadata = fs::symlink_metadata(&path)
            .map_err(|error| FsError::io("Unable to read file metadata", &path, error))?;
        let metadata = if entry.is_symlink {
            fs::metadata(&path).unwrap_or(symlink_metadata)
        } else {
            symlink_metadata
        };

        Ok(FileMetadata {
            path: entry.path,
            kind: entry.kind,
            size: entry.size,
            modified_at: entry.modified_at,
            created_at: time_seconds(metadata.created().ok()),
            accessed_at: time_seconds(metadata.accessed().ok()),
            is_hidden: entry.is_hidden,
            is_symlink: entry.is_symlink,
            is_readonly: entry.is_readonly,
            permissions: permissions_for_metadata(&metadata),
        })
    }

    fn create_dir(&self, path: &str) -> FsResult<()> {
        let path = self.expand_path(path)?;
        fs::create_dir(&path)
            .map_err(|error| FsError::io("Unable to create directory", &path, error))
    }

    fn rename(&self, from: &str, to: &str) -> FsResult<()> {
        let from = self.expand_path(from)?;
        let to = self.expand_path(to)?;
        fs::rename(&from, &to).map_err(|error| FsError::io("Unable to rename item", &from, error))
    }

    fn delete(&self, path: &str) -> FsResult<()> {
        let path = self.expand_path(path)?;
        Self::delete_path(&path)
    }

    fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let from = self.expand_path(from)?;
        let to = self.expand_path(to)?;
        Self::copy_recursive(&from, &to)
    }

    fn move_item(&self, from: &str, to: &str) -> FsResult<()> {
        let from = self.expand_path(from)?;
        let to = self.expand_path(to)?;

        match fs::rename(&from, &to) {
            Ok(()) => Ok(()),
            Err(error) if is_cross_device_error(&error) => {
                let temporary_to = Self::temporary_move_path(&to)?;

                if let Err(copy_error) = Self::copy_recursive(&from, &temporary_to) {
                    Self::cleanup_partial_copy(&temporary_to);
                    return Err(copy_error);
                }

                if let Err(replace_error) = fs::rename(&temporary_to, &to) {
                    Self::cleanup_partial_copy(&temporary_to);
                    return Err(FsError::io(
                        "Unable to place moved item",
                        &to,
                        replace_error,
                    ));
                }

                Self::delete_path(&from)
            }
            Err(error) => Err(FsError::io("Unable to move item", &from, error)),
        }
    }
}

#[cfg(unix)]
fn is_cross_device_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(18)
}

#[cfg(windows)]
fn is_cross_device_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(17)
}

#[cfg(not(any(unix, windows)))]
fn is_cross_device_error(_error: &std::io::Error) -> bool {
    false
}

fn modified_seconds(metadata: &fs::Metadata) -> Option<u64> {
    time_seconds(metadata.modified().ok())
}

fn time_seconds(time: Option<SystemTime>) -> Option<u64> {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

#[cfg(unix)]
fn permissions_for_metadata(metadata: &fs::Metadata) -> Option<FilePermissions> {
    let mode = metadata.mode() & 0o7777;
    let uid = metadata.uid();
    let gid = metadata.gid();

    Some(FilePermissions {
        owner: permission_set(mode, 0o400, 0o200, 0o100),
        group: permission_set(mode, 0o040, 0o020, 0o010),
        others: permission_set(mode, 0o004, 0o002, 0o001),
        symbolic: symbolic_mode(mode),
        octal: format!("{:03o}", mode & 0o777),
        mode,
        owner_name: user_name(uid),
        group_name: group_name(gid),
        uid: Some(uid),
        gid: Some(gid),
    })
}

#[cfg(not(unix))]
fn permissions_for_metadata(_metadata: &fs::Metadata) -> Option<FilePermissions> {
    None
}

#[cfg(unix)]
fn permission_set(mode: u32, read: u32, write: u32, execute: u32) -> PermissionSet {
    PermissionSet {
        read: mode & read != 0,
        write: mode & write != 0,
        execute: mode & execute != 0,
    }
}

#[cfg(unix)]
fn symbolic_mode(mode: u32) -> String {
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];

    bits.into_iter()
        .map(|(bit, character)| if mode & bit != 0 { character } else { '-' })
        .collect()
}

#[cfg(unix)]
fn user_name(uid: u32) -> Option<String> {
    let mut passwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0; 4096];

    let status = unsafe {
        libc::getpwuid_r(
            uid,
            passwd.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };

    if status != 0 || result.is_null() {
        return None;
    }

    let passwd = unsafe { passwd.assume_init() };
    c_string(passwd.pw_name)
}

#[cfg(unix)]
fn group_name(gid: u32) -> Option<String> {
    let mut group = std::mem::MaybeUninit::<libc::group>::uninit();
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0; 4096];

    let status = unsafe {
        libc::getgrgid_r(
            gid,
            group.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };

    if status != 0 || result.is_null() {
        return None;
    }

    let group = unsafe { group.assume_init() };
    c_string(group.gr_name)
}

#[cfg(unix)]
fn c_string(value: *const libc::c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }

    Some(
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn compare_entries(a: &FileEntry, b: &FileEntry) -> Ordering {
    a.kind
        .sort_rank()
        .cmp(&b.kind.sort_rank())
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}
