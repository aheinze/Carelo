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

    fn destination_exists_error(path: &Path) -> FsError {
        FsError::new(
            "destination_exists",
            "An item already exists at the destination.",
            Some(path.to_string_lossy().into_owned()),
        )
    }

    fn destination_type_error(path: &Path) -> FsError {
        FsError::new(
            "destination_type_conflict",
            "The existing destination has an incompatible type.",
            Some(path.to_string_lossy().into_owned()),
        )
    }

    fn path_exists(path: &Path) -> FsResult<bool> {
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

    fn copy_recursive(from: &Path, to: &Path, overwrite: bool) -> FsResult<()> {
        let symlink_metadata = fs::symlink_metadata(from)
            .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

        if !overwrite && Self::path_exists(to)? {
            return Err(Self::destination_exists_error(to));
        }

        if symlink_metadata.file_type().is_symlink() {
            return Self::copy_symlink(from, to, overwrite);
        }

        let metadata = symlink_metadata;

        if metadata.is_dir() {
            if overwrite && Self::path_exists(to)? {
                return Err(Self::destination_type_error(to));
            }

            fs::create_dir_all(to).map_err(|error| {
                FsError::io("Unable to create destination directory", to, error)
            })?;

            for child in fs::read_dir(from)
                .map_err(|error| FsError::io("Unable to read source directory", from, error))?
            {
                let child = child.map_err(|error| {
                    FsError::io("Unable to read source directory entry", from, error)
                })?;
                Self::copy_recursive(&child.path(), &to.join(child.file_name()), overwrite)?;
            }

            return Ok(());
        }

        if overwrite && Self::path_exists(to)? {
            let target_metadata = fs::symlink_metadata(to)
                .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;

            if target_metadata.is_dir() {
                return Err(Self::destination_type_error(to));
            }

            fs::remove_file(to)
                .map_err(|error| FsError::io("Unable to replace existing file", to, error))?;
        }

        fs::copy(from, to)
            .map(|_| ())
            .map_err(|error| FsError::io("Unable to copy file", from, error))
    }

    fn copy_symlink(from: &Path, to: &Path, overwrite: bool) -> FsResult<()> {
        if overwrite && Self::path_exists(to)? {
            let target_metadata = fs::symlink_metadata(to)
                .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;

            if target_metadata.is_dir() && !target_metadata.file_type().is_symlink() {
                return Err(Self::destination_type_error(to));
            }

            fs::remove_file(to).map_err(|error| {
                FsError::io("Unable to replace existing symbolic link", to, error)
            })?;
        }

        let target = fs::read_link(from)
            .map_err(|error| FsError::io("Unable to read symbolic link", from, error))?;

        create_symlink(&target, to, from)
    }

    fn delete_path(path: &Path) -> FsResult<()> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| FsError::io("Unable to read item before delete", path, error))?;

        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(path)
                .map_err(|error| FsError::io("Unable to delete directory", path, error))
        } else {
            fs::remove_file(path).map_err(|error| FsError::io("Unable to delete file", path, error))
        }
    }

    fn cleanup_partial_copy(path: &Path) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };

        let _ = if metadata.is_dir() && !metadata.file_type().is_symlink() {
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

        sort_entries(&mut entries);
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

    fn create_file(&self, path: &str) -> FsResult<()> {
        let path = self.expand_path(path)?;
        // create_new fails if the path already exists, so we never clobber a file.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map(|_| ())
            .map_err(|error| FsError::io("Unable to create file", &path, error))
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

    fn copy(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
        let from = self.expand_path(from)?;
        let to = self.expand_path(to)?;
        let existed_before = Self::path_exists(&to)?;

        if !overwrite && existed_before {
            return Err(Self::destination_exists_error(&to));
        }

        match Self::copy_recursive(&from, &to, overwrite) {
            Ok(()) => Ok(()),
            Err(error) => {
                if !overwrite && !existed_before {
                    Self::cleanup_partial_copy(&to);
                }

                Err(error)
            }
        }
    }

    fn move_item(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
        let from = self.expand_path(from)?;
        let to = self.expand_path(to)?;

        if !overwrite && Self::path_exists(&to)? {
            return Err(Self::destination_exists_error(&to));
        }

        if overwrite && Self::path_exists(&to)? {
            let from_metadata = fs::symlink_metadata(&from)
                .map_err(|error| FsError::io("Unable to read source metadata", &from, error))?;
            let target_metadata = fs::symlink_metadata(&to)
                .map_err(|error| FsError::io("Unable to read destination metadata", &to, error))?;

            if from_metadata.is_dir() || target_metadata.is_dir() {
                return Err(Self::destination_type_error(&to));
            }
        }

        match fs::rename(&from, &to) {
            Ok(()) => Ok(()),
            Err(error) if is_cross_device_error(&error) => {
                let temporary_to = Self::temporary_move_path(&to)?;

                if let Err(copy_error) = Self::copy_recursive(&from, &temporary_to, false) {
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

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path, _source_link: &Path) -> FsResult<()> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|error| FsError::io("Unable to create symbolic link", link, error))
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path, source_link: &Path) -> FsResult<()> {
    let target_is_dir = fs::metadata(source_link)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    let result = if target_is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };

    result.map_err(|error| FsError::io("Unable to create symbolic link", link, error))
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &Path, link: &Path, _source_link: &Path) -> FsResult<()> {
    Err(FsError::new(
        "symlink_unsupported",
        "Preserving symbolic links is not supported on this platform.",
        Some(link.to_string_lossy().into_owned()),
    ))
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
pub(crate) fn permissions_for_metadata(metadata: &fs::Metadata) -> Option<FilePermissions> {
    let mode = metadata.mode() & 0o7777;
    Some(file_permissions_from_unix_mode(
        mode,
        Some(metadata.uid()),
        Some(metadata.gid()),
        true,
    ))
}

pub(crate) fn file_permissions_from_unix_mode(
    mode: u32,
    uid: Option<u32>,
    gid: Option<u32>,
    resolve_names: bool,
) -> FilePermissions {
    FilePermissions {
        owner: permission_set(mode, 0o400, 0o200, 0o100),
        group: permission_set(mode, 0o040, 0o020, 0o010),
        others: permission_set(mode, 0o004, 0o002, 0o001),
        symbolic: symbolic_mode(mode),
        octal: format!("{:03o}", mode & 0o777),
        mode,
        owner_name: uid.and_then(|uid| resolve_names.then(|| user_name_for_id(uid)).flatten()),
        group_name: gid.and_then(|gid| resolve_names.then(|| group_name_for_id(gid)).flatten()),
        uid,
        gid,
    }
}

#[cfg(not(unix))]
pub(crate) fn permissions_for_metadata(_metadata: &fs::Metadata) -> Option<FilePermissions> {
    None
}

fn permission_set(mode: u32, read: u32, write: u32, execute: u32) -> PermissionSet {
    PermissionSet {
        read: mode & read != 0,
        write: mode & write != 0,
        execute: mode & execute != 0,
    }
}

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
fn user_name_for_id(uid: u32) -> Option<String> {
    user_name(uid)
}

#[cfg(not(unix))]
fn user_name_for_id(_uid: u32) -> Option<String> {
    None
}

#[cfg(unix)]
fn group_name_for_id(gid: u32) -> Option<String> {
    group_name(gid)
}

#[cfg(not(unix))]
fn group_name_for_id(_gid: u32) -> Option<String> {
    None
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

fn sort_entries(entries: &mut [FileEntry]) {
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

    fn test_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "carelo-local-fs-{name}-{}-{nonce}",
            std::process::id()
        ));

        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn create_file_makes_empty_file_and_refuses_to_clobber() {
        let root = test_root("create-file");
        let target = root.join("notes.txt");
        let provider = LocalFileProvider::new();

        provider
            .create_file(&target.to_string_lossy())
            .expect("create empty file");
        assert!(target.is_file());
        assert_eq!(
            fs::read_to_string(&target).expect("read created file"),
            ""
        );

        // A second create must not truncate the now-populated file.
        fs::write(&target, "keep me").expect("write content");
        let error = provider
            .create_file(&target.to_string_lossy())
            .expect_err("create should refuse an existing path");
        assert_eq!(error.code, "io_error");
        assert_eq!(
            fs::read_to_string(&target).expect("read preserved file"),
            "keep me"
        );

        cleanup(&root);
    }

    #[test]
    fn copy_file_requires_explicit_overwrite_for_existing_destination() {
        let root = test_root("copy-file-conflict");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "incoming").expect("write source");
        fs::write(&destination, "existing").expect("write destination");
        let provider = LocalFileProvider::new();

        let error = provider
            .copy(
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                false,
            )
            .expect_err("copy should refuse implicit overwrite");

        assert_eq!(error.code, "destination_exists");
        assert_eq!(
            fs::read_to_string(&destination).expect("read destination"),
            "existing"
        );

        provider
            .copy(
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                true,
            )
            .expect("copy with overwrite");
        assert_eq!(
            fs::read_to_string(&destination).expect("read overwritten destination"),
            "incoming"
        );

        cleanup(&root);
    }

    #[test]
    fn move_file_requires_explicit_overwrite_for_existing_destination() {
        let root = test_root("move-file-conflict");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "incoming").expect("write source");
        fs::write(&destination, "existing").expect("write destination");
        let provider = LocalFileProvider::new();

        let error = provider
            .move_item(
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                false,
            )
            .expect_err("move should refuse implicit overwrite");

        assert_eq!(error.code, "destination_exists");
        assert!(source.exists());
        assert_eq!(
            fs::read_to_string(&destination).expect("read destination"),
            "existing"
        );

        provider
            .move_item(
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                true,
            )
            .expect("move with overwrite");
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(&destination).expect("read overwritten destination"),
            "incoming"
        );

        cleanup(&root);
    }

    #[test]
    fn directory_replacement_is_blocked_for_moves() {
        let root = test_root("move-directory-conflict");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source directory");
        fs::create_dir_all(&destination).expect("create destination directory");
        fs::write(source.join("incoming.txt"), "incoming").expect("write source child");
        fs::write(destination.join("existing.txt"), "existing").expect("write destination child");
        let provider = LocalFileProvider::new();

        let error = provider
            .move_item(
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                true,
            )
            .expect_err("directory replacement should be blocked");

        assert_eq!(error.code, "destination_type_conflict");
        assert!(source.exists());
        assert!(destination.join("existing.txt").exists());

        cleanup(&root);
    }

    #[test]
    fn directory_replacement_is_blocked_for_copies() {
        let root = test_root("copy-directory-conflict");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source directory");
        fs::create_dir_all(&destination).expect("create destination directory");
        fs::write(source.join("incoming.txt"), "incoming").expect("write source child");
        fs::write(destination.join("existing.txt"), "existing").expect("write destination child");
        let provider = LocalFileProvider::new();

        let error = provider
            .copy(
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                true,
            )
            .expect_err("directory replacement should be blocked");

        assert_eq!(error.code, "destination_type_conflict");
        assert!(source.exists());
        assert!(destination.join("existing.txt").exists());
        assert!(!destination.join("incoming.txt").exists());

        cleanup(&root);
    }
}
