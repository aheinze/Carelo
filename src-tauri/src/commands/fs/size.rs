use super::*;

#[tauri::command]
pub async fn measure_items_size(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    job_id: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<SizeMeasureResult, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if paths.is_empty() {
        return Ok(SizeMeasureResult::default());
    }

    let mut remote_paths = Vec::new();
    let mut local_path_strings = Vec::new();

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(FsError::new(
                "unsupported_size_measure",
                "Folder size measurement is available for local and remote files and folders only.",
                Some(path.clone()),
            ));
        }

        if let Some(remote_path) = parse_remote_path(path) {
            remote_paths.push(remote_path);
        } else {
            local_path_strings.push(path.clone());
        }
    }

    let mut result = SizeMeasureResult::default();

    if !remote_paths.is_empty() {
        let remote_result = measure_remote_paths_size(&remotes, remote_paths).await?;
        result = merge_size_measure_results(result, remote_result.into());
    }

    if local_path_strings.is_empty() {
        emit_file_operation_progress(
            &app,
            &job_id,
            "measure",
            "running",
            ProgressSnapshot {
                processed_bytes: result.logical_bytes,
                processed_entries: result
                    .files
                    .saturating_add(result.directories)
                    .saturating_add(result.symlinks)
                    .saturating_add(result.skipped),
                ..ProgressSnapshot::default()
            },
        );
        return Ok(result);
    }

    let local_paths = local_path_strings
        .iter()
        .map(|path| expand_local_path(path))
        .collect::<FsResult<Vec<_>>>()?;

    let measure_app = app.clone();
    let measure_job_id = job_id.clone();
    let measure_operation_state = operation_state.inner().clone();

    let local_result = run_local(move |_| {
        measure_local_items_size(
            &measure_app,
            &measure_operation_state,
            &measure_job_id,
            local_paths,
        )
    })
    .await?;

    Ok(merge_size_measure_results(result, local_result))
}

#[derive(Debug, Default)]
struct SizeMeasureAccumulator {
    logical_bytes: u64,
    disk_bytes: u64,
    files: u64,
    directories: u64,
    symlinks: u64,
    skipped: u64,
    processed_entries: u64,
}

impl From<SizeMeasureAccumulator> for SizeMeasureResult {
    fn from(value: SizeMeasureAccumulator) -> Self {
        Self {
            logical_bytes: value.logical_bytes,
            disk_bytes: value.disk_bytes,
            files: value.files,
            directories: value.directories,
            symlinks: value.symlinks,
            skipped: value.skipped,
        }
    }
}

impl From<RemoteSizeMeasure> for SizeMeasureResult {
    fn from(value: RemoteSizeMeasure) -> Self {
        Self {
            logical_bytes: value.logical_bytes,
            disk_bytes: value.disk_bytes,
            files: value.files,
            directories: value.directories,
            symlinks: value.symlinks,
            skipped: value.skipped,
        }
    }
}

fn merge_size_measure_results(
    mut left: SizeMeasureResult,
    right: SizeMeasureResult,
) -> SizeMeasureResult {
    left.logical_bytes = left.logical_bytes.saturating_add(right.logical_bytes);
    left.disk_bytes = left.disk_bytes.saturating_add(right.disk_bytes);
    left.files = left.files.saturating_add(right.files);
    left.directories = left.directories.saturating_add(right.directories);
    left.symlinks = left.symlinks.saturating_add(right.symlinks);
    left.skipped = left.skipped.saturating_add(right.skipped);
    left
}

fn measure_local_items_size(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    roots: Vec<PathBuf>,
) -> FsResult<SizeMeasureResult> {
    let mut accumulator = SizeMeasureAccumulator::default();
    let mut stack = roots.into_iter().rev().collect::<Vec<_>>();
    #[cfg(unix)]
    let mut seen_directories = HashSet::new();
    #[cfg(not(unix))]
    let mut seen_directories = ();
    #[cfg(unix)]
    let mut seen_regular_files = HashSet::new();
    #[cfg(not(unix))]
    let mut seen_regular_files = ();

    emit_size_measure_progress(app, job_id, &accumulator, None);

    while let Some(path) = stack.pop() {
        operation_state.checkpoint(job_id, Some(&path))?;

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                accumulator.skipped = accumulator.skipped.saturating_add(1);
                accumulator.processed_entries = accumulator.processed_entries.saturating_add(1);
                maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
                continue;
            }
        };

        accumulator.processed_entries = accumulator.processed_entries.saturating_add(1);
        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            accumulator.symlinks = accumulator.symlinks.saturating_add(1);
            accumulator.logical_bytes = accumulator.logical_bytes.saturating_add(metadata.len());
            accumulator.disk_bytes = accumulator
                .disk_bytes
                .saturating_add(metadata_disk_usage_bytes(&metadata));
            maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
            continue;
        }

        if metadata.is_file() {
            accumulator.files = accumulator.files.saturating_add(1);

            if should_count_regular_file(&metadata, &mut seen_regular_files) {
                accumulator.logical_bytes =
                    accumulator.logical_bytes.saturating_add(metadata.len());
                accumulator.disk_bytes = accumulator
                    .disk_bytes
                    .saturating_add(metadata_disk_usage_bytes(&metadata));
            }

            maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
            continue;
        }

        if metadata.is_dir() {
            if !should_count_directory(&metadata, &mut seen_directories) {
                maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
                continue;
            }

            accumulator.directories = accumulator.directories.saturating_add(1);
            accumulator.disk_bytes = accumulator
                .disk_bytes
                .saturating_add(metadata_disk_usage_bytes(&metadata));

            match fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        operation_state.checkpoint(job_id, Some(&path))?;

                        match entry {
                            Ok(entry) => stack.push(entry.path()),
                            Err(_) => {
                                accumulator.skipped = accumulator.skipped.saturating_add(1);
                            }
                        }
                    }
                }
                Err(_) => {
                    accumulator.skipped = accumulator.skipped.saturating_add(1);
                }
            }

            maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
            continue;
        }

        accumulator.files = accumulator.files.saturating_add(1);
        accumulator.logical_bytes = accumulator.logical_bytes.saturating_add(metadata.len());
        accumulator.disk_bytes = accumulator
            .disk_bytes
            .saturating_add(metadata_disk_usage_bytes(&metadata));
        maybe_emit_size_measure_progress(app, job_id, &accumulator, &path);
    }

    emit_size_measure_progress(app, job_id, &accumulator, None);
    Ok(accumulator.into())
}

fn maybe_emit_size_measure_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    accumulator: &SizeMeasureAccumulator,
    current_path: &Path,
) {
    if accumulator.processed_entries % 128 != 0 {
        return;
    }

    emit_size_measure_progress(app, job_id, accumulator, Some(current_path));
}

fn emit_size_measure_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    accumulator: &SizeMeasureAccumulator,
    current_path: Option<&Path>,
) {
    emit_file_operation_progress(
        app,
        job_id,
        "measure",
        "running",
        ProgressSnapshot {
            processed_bytes: accumulator.logical_bytes,
            processed_entries: accumulator.processed_entries,
            current_path: current_path.map(|path| path.to_string_lossy().into_owned()),
            ..ProgressSnapshot::default()
        },
    );
}

#[cfg(unix)]
fn should_count_directory(
    metadata: &fs::Metadata,
    seen_directories: &mut HashSet<(u64, u64)>,
) -> bool {
    seen_directories.insert((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn should_count_directory(metadata: &fs::Metadata, seen_directories: &mut ()) -> bool {
    let _ = (metadata, seen_directories);
    true
}

#[cfg(unix)]
fn should_count_regular_file(
    metadata: &fs::Metadata,
    seen_regular_files: &mut HashSet<(u64, u64)>,
) -> bool {
    metadata.nlink() <= 1 || seen_regular_files.insert((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn should_count_regular_file(metadata: &fs::Metadata, seen_regular_files: &mut ()) -> bool {
    let _ = (metadata, seen_regular_files);
    true
}

#[cfg(unix)]
fn metadata_disk_usage_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn metadata_disk_usage_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}
