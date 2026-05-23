use super::*;

#[derive(Clone, Default)]
pub struct FileOperationState {
    cancelled_jobs: Arc<Mutex<HashSet<String>>>,
    paused_jobs: Arc<Mutex<HashSet<String>>>,
}

impl FileOperationState {
    pub(super) fn request_cancel(&self, job_id: &str) {
        if let Ok(mut cancelled_jobs) = self.cancelled_jobs.lock() {
            cancelled_jobs.insert(job_id.to_string());
        }
    }

    pub(super) fn clear_cancel(&self, job_id: &str) {
        if let Ok(mut cancelled_jobs) = self.cancelled_jobs.lock() {
            cancelled_jobs.remove(job_id);
        }
    }

    pub(super) fn clear_pause(&self, job_id: &str) {
        if let Ok(mut paused_jobs) = self.paused_jobs.lock() {
            paused_jobs.remove(job_id);
        }
    }

    pub(super) fn request_pause(&self, job_id: &str) {
        if let Ok(mut paused_jobs) = self.paused_jobs.lock() {
            paused_jobs.insert(job_id.to_string());
        }
    }

    pub(super) fn request_resume(&self, job_id: &str) {
        self.clear_pause(job_id);
    }

    fn is_cancelled(&self, job_id: &Option<String>) -> bool {
        let Some(job_id) = job_id else {
            return false;
        };

        self.cancelled_jobs
            .lock()
            .map(|cancelled_jobs| cancelled_jobs.contains(job_id))
            .unwrap_or(false)
    }

    fn is_paused(&self, job_id: &Option<String>) -> bool {
        let Some(job_id) = job_id else {
            return false;
        };

        self.paused_jobs
            .lock()
            .map(|paused_jobs| paused_jobs.contains(job_id))
            .unwrap_or(false)
    }

    pub(super) fn checkpoint(&self, job_id: &Option<String>, path: Option<&Path>) -> FsResult<()> {
        loop {
            if self.is_cancelled(job_id) {
                return Err(FsError::new(
                    "operation_cancelled",
                    "The file operation was cancelled.",
                    path.map(|path| path.to_string_lossy().into_owned()),
                ));
            }

            if !self.is_paused(job_id) {
                return Ok(());
            }

            thread::sleep(Duration::from_millis(120));
        }
    }

    pub(super) fn wait_if_paused_or_cancelled(&self, job_id: &Option<String>) -> bool {
        self.checkpoint(job_id, None).is_err()
    }
}

pub(super) struct OperationStateCleanup {
    operation_state: FileOperationState,
    job_id: Option<String>,
}

impl OperationStateCleanup {
    pub(super) fn new(operation_state: FileOperationState, job_id: Option<String>) -> Self {
        Self {
            operation_state,
            job_id,
        }
    }
}

impl Drop for OperationStateCleanup {
    fn drop(&mut self) {
        if let Some(job_id) = &self.job_id {
            self.operation_state.clear_cancel(job_id);
            self.operation_state.clear_pause(job_id);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOperationProgress {
    job_id: String,
    operation: String,
    status: String,
    pub(super) processed_bytes: u64,
    pub(super) total_bytes: u64,
    pub(super) processed_entries: u64,
    pub(super) total_entries: u64,
    pub(super) current_path: Option<String>,
    pub(super) current_bytes: u64,
    pub(super) current_total_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeMeasureResult {
    pub logical_bytes: u64,
    pub disk_bytes: u64,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub skipped: u64,
}

#[tauri::command]
pub async fn cancel_file_operation(
    job_id: String,
    operation_state: tauri::State<'_, FileOperationState>,
) -> Result<(), FsError> {
    if job_id.trim().is_empty() {
        return Err(FsError::new(
            "invalid_job_id",
            "Unable to cancel an operation without a job id.",
            None,
        ));
    }

    operation_state.request_cancel(&job_id);
    Ok(())
}

#[tauri::command]
pub async fn pause_file_operation(
    job_id: String,
    operation_state: tauri::State<'_, FileOperationState>,
) -> Result<(), FsError> {
    if job_id.trim().is_empty() {
        return Err(FsError::new(
            "invalid_job_id",
            "Unable to pause an operation without a job id.",
            None,
        ));
    }

    operation_state.request_pause(&job_id);
    Ok(())
}

#[tauri::command]
pub async fn resume_file_operation(
    job_id: String,
    operation_state: tauri::State<'_, FileOperationState>,
) -> Result<(), FsError> {
    if job_id.trim().is_empty() {
        return Err(FsError::new(
            "invalid_job_id",
            "Unable to resume an operation without a job id.",
            None,
        ));
    }

    operation_state.request_resume(&job_id);
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub(super) struct ProgressSnapshot {
    pub(super) processed_bytes: u64,
    pub(super) total_bytes: u64,
    pub(super) processed_entries: u64,
    pub(super) total_entries: u64,
    pub(super) current_path: Option<String>,
    pub(super) current_bytes: u64,
    pub(super) current_total_bytes: u64,
}

impl From<archive::ArchiveProgress> for ProgressSnapshot {
    fn from(progress: archive::ArchiveProgress) -> Self {
        Self {
            processed_bytes: progress.processed_bytes,
            total_bytes: progress.total_bytes,
            processed_entries: progress.processed_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path,
            current_bytes: progress.current_bytes,
            current_total_bytes: progress.current_total_bytes,
        }
    }
}

impl From<operations::OperationProgress> for ProgressSnapshot {
    fn from(progress: operations::OperationProgress) -> Self {
        Self {
            processed_bytes: progress.processed_bytes,
            total_bytes: progress.total_bytes,
            processed_entries: progress.processed_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path,
            current_bytes: progress.current_bytes,
            current_total_bytes: progress.current_total_bytes,
        }
    }
}

pub(super) fn emit_file_operation_progress<P>(
    app: &AppHandle,
    job_id: &Option<String>,
    operation: &str,
    status: &str,
    progress: P,
) where
    P: Into<ProgressSnapshot>,
{
    let Some(job_id) = job_id else {
        return;
    };

    let progress = progress.into();
    let _ = app.emit(
        "file-operation-progress",
        FileOperationProgress {
            job_id: job_id.clone(),
            operation: operation.to_string(),
            status: status.to_string(),
            processed_bytes: progress.processed_bytes,
            total_bytes: progress.total_bytes,
            processed_entries: progress.processed_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path,
            current_bytes: progress.current_bytes,
            current_total_bytes: progress.current_total_bytes,
        },
    );
}

pub(super) fn emit_transfer_operation_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    operation: &str,
    status: &str,
    progress: operations::OperationProgress,
) {
    emit_file_operation_progress(app, job_id, operation, status, progress);
}

pub(super) fn emit_file_operation_status(
    app: &AppHandle,
    job_id: &Option<String>,
    operation: &str,
    status: &str,
) {
    emit_file_operation_progress(app, job_id, operation, status, ProgressSnapshot::default());
}
