use serde::Serialize;

pub type FsResult<T> = Result<T, FsError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: FileEntryKind,
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_readonly: bool,
    pub tag_color: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl FileEntryKind {
    pub fn sort_rank(self) -> u8 {
        match self {
            FileEntryKind::Directory => 0,
            FileEntryKind::File => 1,
            FileEntryKind::Symlink => 2,
            FileEntryKind::Other => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub path: String,
    pub kind: FileEntryKind,
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
    pub created_at: Option<u64>,
    pub accessed_at: Option<u64>,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_readonly: bool,
    pub permissions: Option<FilePermissions>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePermissions {
    pub owner: PermissionSet,
    pub group: PermissionSet,
    pub others: PermissionSet,
    pub symbolic: String,
    pub octal: String,
    pub mode: u32,
    pub owner_name: Option<String>,
    pub group_name: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSet {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeEntry {
    pub name: String,
    pub path: String,
    pub device_path: Option<String>,
    pub detail: Option<String>,
    pub is_removable: bool,
    pub is_mounted: bool,
    pub capabilities: Option<RemoteVolumeCapabilities>,
    pub health: Option<RemoteVolumeHealth>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVolumeCapabilities {
    pub can_read: bool,
    pub can_write: bool,
    pub can_create_folders: bool,
    pub can_rename: bool,
    pub can_delete: bool,
    pub can_recursive_delete: bool,
    pub can_server_side_copy: bool,
    pub can_stream_media: bool,
    pub can_search_filenames: bool,
    pub can_search_content: bool,
    pub has_posix_permissions: bool,
    pub has_owner_group: bool,
    pub has_symlinks: bool,
    pub is_mount_backed: bool,
    pub needs_mount: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVolumeHealth {
    pub status: String,
    pub message: Option<String>,
    pub checked_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsError {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
}

impl FsError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, path: Option<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path,
        }
    }

    pub fn io(action: &str, path: &std::path::Path, error: std::io::Error) -> Self {
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "permission_denied"
        } else {
            "io_error"
        };

        Self::new(
            code,
            format!("{action}: {error}"),
            Some(path.to_string_lossy().into_owned()),
        )
    }

    pub fn invalid_path(path: &str) -> Self {
        Self::new(
            "invalid_path",
            "Path is empty or invalid.",
            Some(path.to_string()),
        )
    }
}
