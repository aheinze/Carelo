use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fs::models::{FsError, FsResult};

const PROGRESS_BYTE_STEP: u64 = 512 * 1024;

#[derive(Debug, Clone)]
pub struct LocalTransferItem {
    pub from: String,
    pub to: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Default)]
pub struct OperationProgress {
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub processed_entries: u64,
    pub total_entries: u64,
    pub current_path: Option<String>,
    pub current_bytes: u64,
    pub current_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Measure {
    bytes: u64,
    entries: u64,
}

#[derive(Debug, Clone, Default)]
struct ProgressState {
    processed_bytes: u64,
    total_bytes: u64,
    processed_entries: u64,
    total_entries: u64,
    current_bytes: u64,
    current_total_bytes: u64,
    last_emitted_bytes: u64,
    last_emitted_current_bytes: u64,
    last_emitted_entries: u64,
}

pub fn copy_items_with_progress<F, C>(
    items: &[LocalTransferItem],
    mut on_progress: F,
    mut checkpoint: C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    let resolved_items = resolve_items(items)?;
    let measure = measure_items(&resolved_items, &mut checkpoint)?;
    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };

    emit_progress(&mut progress, None, true, &mut on_progress);

    for item in resolved_items {
        checkpoint(Some(&item.from))?;
        copy_path(
            &item.from,
            &item.to,
            item.overwrite,
            &mut progress,
            &mut on_progress,
            &mut checkpoint,
        )?;
    }

    progress.processed_bytes = progress.total_bytes;
    progress.processed_entries = progress.total_entries;
    progress.current_bytes = 0;
    progress.current_total_bytes = 0;
    emit_progress(&mut progress, None, true, &mut on_progress);
    Ok(())
}

pub fn move_items_with_progress<F, C>(
    items: &[LocalTransferItem],
    mut on_progress: F,
    mut checkpoint: C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    let resolved_items = resolve_items(items)?;
    let measure = measure_items(&resolved_items, &mut checkpoint)?;
    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };

    emit_progress(&mut progress, None, true, &mut on_progress);

    for item in resolved_items {
        checkpoint(Some(&item.from))?;
        move_path(
            &item.from,
            &item.to,
            item.overwrite,
            &mut progress,
            &mut on_progress,
            &mut checkpoint,
        )?;
    }

    progress.processed_bytes = progress.total_bytes;
    progress.processed_entries = progress.total_entries;
    progress.current_bytes = 0;
    progress.current_total_bytes = 0;
    emit_progress(&mut progress, None, true, &mut on_progress);
    Ok(())
}

fn resolve_items(items: &[LocalTransferItem]) -> FsResult<Vec<ResolvedTransferItem>> {
    items
        .iter()
        .map(|item| {
            Ok(ResolvedTransferItem {
                from: expand_path(&item.from)?,
                to: expand_path(&item.to)?,
                overwrite: item.overwrite,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ResolvedTransferItem {
    from: PathBuf,
    to: PathBuf,
    overwrite: bool,
}

fn measure_items<C>(items: &[ResolvedTransferItem], checkpoint: &mut C) -> FsResult<Measure>
where
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    let mut total = Measure::default();
    let mut visited_directories = HashSet::new();

    for item in items {
        checkpoint(Some(&item.from))?;
        let measure = measure_path(&item.from, &mut visited_directories, checkpoint)?;
        total.bytes = total.bytes.saturating_add(measure.bytes);
        total.entries = total.entries.saturating_add(measure.entries);
    }

    Ok(total)
}

fn measure_path<C>(
    path: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    checkpoint: &mut C,
) -> FsResult<Measure>
where
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    checkpoint(Some(path))?;
    let metadata = fs::metadata(path)
        .map_err(|error| FsError::io("Unable to read source metadata", path, error))?;

    if metadata.is_dir() {
        let canonical = fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve source directory", path, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "directory_cycle",
                "Refusing to transfer a directory cycle.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let mut total = Measure {
            bytes: 0,
            entries: 1,
        };

        for child in fs::read_dir(path)
            .map_err(|error| FsError::io("Unable to read source directory", path, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read source directory entry", path, error)
            })?;
            let measure = measure_path(&child.path(), visited_directories, checkpoint)?;
            total.bytes = total.bytes.saturating_add(measure.bytes);
            total.entries = total.entries.saturating_add(measure.entries);
        }

        return Ok(total);
    }

    Ok(Measure {
        bytes: metadata.len(),
        entries: 1,
    })
}

fn copy_path<F, C>(
    from: &Path,
    to: &Path,
    overwrite: bool,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    checkpoint(Some(from))?;
    let metadata = fs::metadata(from)
        .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

    if !overwrite && path_exists(to)? {
        return Err(destination_exists_error(to));
    }

    if metadata.is_dir() {
        if overwrite && path_exists(to)? {
            return Err(destination_type_error(to));
        }

        fs::create_dir_all(to)
            .map_err(|error| FsError::io("Unable to create destination directory", to, error))?;
        progress.current_bytes = 0;
        progress.current_total_bytes = 0;
        progress.processed_entries = progress.processed_entries.saturating_add(1);
        emit_progress(progress, Some(from), true, on_progress);

        for child in fs::read_dir(from)
            .map_err(|error| FsError::io("Unable to read source directory", from, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read source directory entry", from, error)
            })?;
            copy_path(
                &child.path(),
                &to.join(child.file_name()),
                overwrite,
                progress,
                on_progress,
                checkpoint,
            )?;
        }

        return Ok(());
    }

    if overwrite && path_exists(to)? {
        let target_metadata = fs::metadata(to)
            .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;

        if target_metadata.is_dir() {
            return Err(destination_type_error(to));
        }
    }

    copy_file(
        from,
        to,
        &metadata,
        overwrite,
        progress,
        on_progress,
        checkpoint,
    )
}

fn move_path<F, C>(
    from: &Path,
    to: &Path,
    overwrite: bool,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    checkpoint(Some(from))?;

    if !overwrite && path_exists(to)? {
        return Err(destination_exists_error(to));
    }

    if overwrite && path_exists(to)? {
        let from_metadata = fs::metadata(from)
            .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;
        let target_metadata = fs::metadata(to)
            .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;

        if from_metadata.is_dir() || target_metadata.is_dir() {
            return Err(destination_type_error(to));
        }
    }

    match fs::rename(from, to) {
        Ok(()) => {
            let mut visited_directories = HashSet::new();
            let measure = measure_path(to, &mut visited_directories, checkpoint)?;
            progress.processed_bytes = progress.processed_bytes.saturating_add(measure.bytes);
            progress.processed_entries = progress.processed_entries.saturating_add(measure.entries);
            progress.current_bytes = measure.bytes;
            progress.current_total_bytes = measure.bytes;
            emit_progress(progress, Some(to), true, on_progress);
            Ok(())
        }
        Err(error) if is_cross_device_error(&error) => {
            let temporary_to = temporary_move_path(to)?;

            if let Err(copy_error) = copy_path(
                from,
                &temporary_to,
                false,
                progress,
                on_progress,
                checkpoint,
            ) {
                cleanup_partial_copy(&temporary_to);
                return Err(copy_error);
            }

            checkpoint(Some(from))?;

            if !overwrite && path_exists(to)? {
                cleanup_partial_copy(&temporary_to);
                return Err(destination_exists_error(to));
            }

            if let Err(replace_error) = fs::rename(&temporary_to, to) {
                cleanup_partial_copy(&temporary_to);
                return Err(FsError::io("Unable to place moved item", to, replace_error));
            }

            delete_path(from)
        }
        Err(error) => Err(FsError::io("Unable to move item", from, error)),
    }
}

fn copy_file<F, C>(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    overwrite: bool,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    let mut reader =
        File::open(from).map_err(|error| FsError::io("Unable to open source file", from, error))?;
    let mut writer = OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(!overwrite)
        .truncate(overwrite)
        .open(to)
        .map_err(|error| FsError::io("Unable to create destination file", to, error))?;
    let mut buffer = [0_u8; 256 * 1024];

    progress.current_bytes = 0;
    progress.current_total_bytes = metadata.len();
    emit_progress(progress, Some(from), true, on_progress);

    loop {
        checkpoint(Some(from))?;
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|error| FsError::io("Unable to read source file", from, error))?;

        if bytes_read == 0 {
            break;
        }

        writer
            .write_all(&buffer[..bytes_read])
            .map_err(|error| FsError::io("Unable to write destination file", to, error))?;
        progress.processed_bytes = progress.processed_bytes.saturating_add(bytes_read as u64);
        progress.current_bytes = progress.current_bytes.saturating_add(bytes_read as u64);
        emit_progress(progress, Some(from), false, on_progress);
    }

    progress.current_bytes = progress.current_total_bytes;
    progress.processed_entries = progress.processed_entries.saturating_add(1);
    emit_progress(progress, Some(from), true, on_progress);
    Ok(())
}

fn emit_progress<F>(
    state: &mut ProgressState,
    current_path: Option<&Path>,
    force: bool,
    on_progress: &mut F,
) where
    F: FnMut(OperationProgress),
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

    on_progress(OperationProgress {
        processed_bytes: state.processed_bytes.min(state.total_bytes),
        total_bytes: state.total_bytes,
        processed_entries: state.processed_entries.min(state.total_entries),
        total_entries: state.total_entries,
        current_path: current_path.map(|path| path.to_string_lossy().into_owned()),
        current_bytes: state.current_bytes.min(state.current_total_bytes),
        current_total_bytes: state.current_total_bytes,
    });
}

fn expand_path(path: &str) -> FsResult<PathBuf> {
    let trimmed = path.trim();

    if trimmed.is_empty() {
        return home_dir();
    }

    if trimmed == "~" {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "carelo-operations-{name}-{}-{nonce}",
            std::process::id()
        ));

        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn copy_reports_aggregate_and_current_file_progress() {
        let root = test_root("copy-progress");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "hello world").expect("write source");
        let mut events = Vec::new();

        copy_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: false,
            }],
            |progress| events.push(progress),
            |_| Ok(()),
        )
        .expect("copy with progress");

        assert_eq!(
            fs::read_to_string(&destination).expect("read destination"),
            "hello world"
        );
        assert!(events.iter().any(|event| event.total_bytes == 11));
        assert!(events.iter().any(|event| event.current_total_bytes == 11));
        let final_event = events.last().expect("final progress event");
        assert_eq!(final_event.processed_bytes, final_event.total_bytes);
        assert_eq!(final_event.processed_entries, final_event.total_entries);

        cleanup(&root);
    }

    #[test]
    fn copy_stops_when_checkpoint_returns_cancelled() {
        let root = test_root("copy-cancel");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "hello world").expect("write source");
        let mut checkpoints = 0;

        let error = copy_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: false,
            }],
            |_| {},
            |_| {
                checkpoints += 1;

                if checkpoints > 1 {
                    Err(FsError::new(
                        "operation_cancelled",
                        "The file operation was cancelled.",
                        Some(source.to_string_lossy().into_owned()),
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("copy should stop at checkpoint");

        assert_eq!(error.code, "operation_cancelled");

        cleanup(&root);
    }
}
