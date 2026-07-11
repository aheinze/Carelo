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
    pub is_encrypted: bool,
    pub needs_unlock: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<FileOperationBatchResult>,
}

impl FsError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, path: Option<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path,
            batch: None,
        }
    }

    pub fn with_batch(mut self, batch: FileOperationBatchResult) -> Self {
        self.batch = Some(batch);
        self
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationBatchResult {
    pub items: Vec<FileOperationItemResult>,
    pub cancelled: bool,
}

impl FileOperationBatchResult {
    pub fn new(mut items: Vec<FileOperationItemResult>, cancelled: bool) -> Self {
        items.sort_by_key(|item| item.index);
        Self { items, cancelled }
    }

    pub fn is_complete(&self) -> bool {
        !self.cancelled
            && self
                .items
                .iter()
                .all(|item| item.status == FileOperationItemStatus::Completed)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationItemResult {
    pub index: usize,
    pub from: String,
    pub to: Option<String>,
    pub status: FileOperationItemStatus,
    pub affected: bool,
    pub errors: Vec<FileOperationError>,
}

impl FileOperationItemResult {
    pub fn not_started(index: usize, from: String, to: Option<String>) -> Self {
        Self {
            index,
            from,
            to,
            status: FileOperationItemStatus::NotStarted,
            affected: false,
            errors: Vec::new(),
        }
    }

    pub fn completed(index: usize, from: String, to: Option<String>) -> Self {
        Self {
            index,
            from,
            to,
            status: FileOperationItemStatus::Completed,
            affected: true,
            errors: Vec::new(),
        }
    }

    pub fn failed(
        index: usize,
        from: String,
        to: Option<String>,
        error: FsError,
        affected: bool,
    ) -> Self {
        Self {
            index,
            from,
            to,
            status: if affected {
                FileOperationItemStatus::Partial
            } else {
                FileOperationItemStatus::Failed
            },
            affected,
            errors: vec![FileOperationError::from(error)],
        }
    }

    pub fn cancelled(
        index: usize,
        from: String,
        to: Option<String>,
        error: FsError,
        affected: bool,
    ) -> Self {
        Self {
            index,
            from,
            to,
            status: FileOperationItemStatus::Cancelled,
            affected,
            errors: vec![FileOperationError::from(error)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileOperationItemStatus {
    Completed,
    Failed,
    Partial,
    Cancelled,
    NotStarted,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationError {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
}

impl From<FsError> for FileOperationError {
    fn from(error: FsError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            path: error.path,
        }
    }
}
