use crate::fs::models::{FileEntry, FileMetadata, FsResult};

pub trait FileProvider: Send + Sync {
    fn list(&self, path: &str) -> FsResult<Vec<FileEntry>>;
    fn stat(&self, path: &str) -> FsResult<FileMetadata>;
    fn create_dir(&self, path: &str) -> FsResult<()>;
    fn rename(&self, from: &str, to: &str) -> FsResult<()>;
    fn delete(&self, path: &str) -> FsResult<()>;
    fn copy(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()>;
    fn move_item(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()>;
}
