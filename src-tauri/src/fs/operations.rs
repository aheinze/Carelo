use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fs::models::{FsError, FsResult};
use serde::Deserialize;

const PROGRESS_BYTE_STEP: u64 = 512 * 1024;

#[derive(Debug, Clone)]
pub struct LocalTransferItem {
    pub from: String,
    pub to: String,
    pub overwrite: bool,
    pub symlink_mode: SymlinkMode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SymlinkMode {
    #[default]
    Preserve,
    Follow,
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
    let mut failures = Vec::new();
    let (plan, measure) = measure_plan(&resolved_items, &mut checkpoint, &mut failures)?;
    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };

    emit_progress(&mut progress, None, true, &mut on_progress);

    for item in plan {
        checkpoint(Some(&item.from))?;

        if let Err(error) = copy_path(
            &item.from,
            &item.to,
            item.overwrite,
            item.symlink_mode,
            &mut progress,
            &mut on_progress,
            &mut checkpoint,
            &mut failures,
            true,
        ) {
            // A cancellation must stop the whole operation; ordinary failures
            // are collected so the remaining items still get copied. Failures
            // deep inside a tree are collected by copy_path itself.
            if is_cancellation_error(&error) {
                return Err(error);
            }

            failures.push(error);
        }
    }

    progress.processed_bytes = progress.total_bytes;
    progress.processed_entries = progress.total_entries;
    progress.current_bytes = 0;
    progress.current_total_bytes = 0;
    emit_progress(&mut progress, None, true, &mut on_progress);
    finalize_failures(failures)
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
    let mut failures = Vec::new();
    let (plan, measure) = measure_plan(&resolved_items, &mut checkpoint, &mut failures)?;
    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };

    emit_progress(&mut progress, None, true, &mut on_progress);

    for item in plan {
        checkpoint(Some(&item.from))?;

        // Each item moves atomically (rename) or via copy-then-delete that only
        // removes the source after a complete copy, so a failed item leaves its
        // source intact and the remaining items can still proceed.
        if let Err(error) = move_path(
            &item.from,
            &item.to,
            item.overwrite,
            item.symlink_mode,
            &mut progress,
            &mut on_progress,
            &mut checkpoint,
        ) {
            if is_cancellation_error(&error) {
                return Err(error);
            }

            failures.push(error);
        }
    }

    progress.processed_bytes = progress.total_bytes;
    progress.processed_entries = progress.total_entries;
    progress.current_bytes = 0;
    progress.current_total_bytes = 0;
    emit_progress(&mut progress, None, true, &mut on_progress);
    finalize_failures(failures)
}

fn is_cancellation_error(error: &FsError) -> bool {
    error.code == "operation_cancelled"
}

// Turn the per-item failures collected during a batch into a single result:
// success when empty, otherwise an aggregated error that still names the first
// underlying cause. The items that did succeed remain on disk.
fn finalize_failures(mut failures: Vec<FsError>) -> FsResult<()> {
    match failures.len() {
        0 => Ok(()),
        // A lone failure is returned verbatim so its original code (e.g.
        // permission_denied) still drives sudo escalation and exact messaging.
        1 => Err(failures.remove(0)),
        count => {
            let first = failures.remove(0);
            Err(FsError::new(
                "operation_partial_failure",
                format!(
                    "{count} items could not be completed. First error: {}",
                    first.message
                ),
                first.path,
            ))
        }
    }
}

#[derive(Default)]
struct CopyPlan {
    /// (from, to, source metadata) — created depth-first before files.
    dirs: Vec<(PathBuf, PathBuf, fs::Metadata)>,
    files: Vec<FileCopyTask>,
    /// (from, to, overwrite)
    symlinks: Vec<(PathBuf, PathBuf, bool)>,
    total_bytes: u64,
    total_entries: u64,
    /// Items that couldn't even be planned (unreadable, cycles).
    failures: Vec<FsError>,
}

struct FileCopyTask {
    from: PathBuf,
    to: PathBuf,
    metadata: fs::Metadata,
    overwrite: bool,
}

/// Walk the tree (single-threaded) producing a flat plan the worker pool can
/// execute. Mirrors `copy_path`'s structure decisions; cycle detection matches
/// `measure_path` so a symlinked loop in Follow mode can't recurse forever.
fn plan_copy_path(
    from: &Path,
    to: &Path,
    overwrite: bool,
    symlink_mode: SymlinkMode,
    visited_directories: &mut HashSet<PathBuf>,
    plan: &mut CopyPlan,
) -> FsResult<()> {
    let symlink_metadata = fs::symlink_metadata(from)
        .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

    if symlink_metadata.file_type().is_symlink() && matches!(symlink_mode, SymlinkMode::Preserve) {
        if !overwrite && path_exists(to)? {
            return Err(destination_exists_error(to));
        }

        plan.symlinks
            .push((from.to_path_buf(), to.to_path_buf(), overwrite));
        plan.total_entries = plan.total_entries.saturating_add(1);
        return Ok(());
    }

    let metadata = if symlink_metadata.file_type().is_symlink() {
        fs::metadata(from)
            .map_err(|error| FsError::io("Unable to read symlink target metadata", from, error))?
    } else {
        symlink_metadata
    };

    if !overwrite && path_exists(to)? {
        return Err(destination_exists_error(to));
    }

    if metadata.is_dir() {
        if overwrite && path_exists(to)? {
            return Err(destination_type_error(to));
        }

        let canonical = fs::canonicalize(from)
            .map_err(|error| FsError::io("Unable to resolve source directory", from, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "directory_cycle",
                "Refusing to transfer a directory cycle.",
                Some(from.to_string_lossy().into_owned()),
            ));
        }

        plan.dirs
            .push((from.to_path_buf(), to.to_path_buf(), metadata.clone()));
        plan.total_entries = plan.total_entries.saturating_add(1);

        for child in fs::read_dir(from)
            .map_err(|error| FsError::io("Unable to read source directory", from, error))?
        {
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    plan.failures.push(FsError::io(
                        "Unable to read source directory entry",
                        from,
                        error,
                    ));
                    continue;
                }
            };

            // Collect a child's planning failure and keep going so one bad file
            // doesn't drop the whole tree; the worker pool reports copy failures.
            if let Err(error) = plan_copy_path(
                &child.path(),
                &to.join(child.file_name()),
                overwrite,
                symlink_mode,
                visited_directories,
                plan,
            ) {
                plan.failures.push(error);
            }
        }

        return Ok(());
    }

    if overwrite && path_exists(to)? {
        let target_metadata = fs::symlink_metadata(to)
            .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;

        if target_metadata.is_dir() {
            return Err(destination_type_error(to));
        }
    }

    plan.total_bytes = plan.total_bytes.saturating_add(metadata.len());
    plan.total_entries = plan.total_entries.saturating_add(1);
    plan.files.push(FileCopyTask {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        metadata,
        overwrite,
    });

    Ok(())
}

/// Copy a batch using a bounded pool of worker threads. Directories are created
/// first, files are copied in parallel (reusing `copy_file`, so reflink /
/// copy_file_range / metadata preservation all apply per file), then directory
/// timestamps are restored. Continue-on-error and cancellation are honored.
///
/// This is genuinely multi-threaded, so it's only used when the caller has
/// decided the storage is solid-state/remote; spinning disks use the sequential
/// `copy_items_with_progress` instead.
pub fn copy_items_parallel<F, C>(
    items: &[LocalTransferItem],
    concurrency: usize,
    on_progress: F,
    checkpoint: C,
) -> FsResult<()>
where
    F: Fn(OperationProgress) + Sync,
    C: Fn(Option<&Path>) -> FsResult<()> + Sync,
{
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;

    let resolved_items = resolve_items(items)?;
    let mut plan = CopyPlan::default();
    let mut visited_directories = HashSet::new();

    for item in &resolved_items {
        checkpoint(Some(&item.from))?;

        if let Err(error) = plan_copy_path(
            &item.from,
            &item.to,
            item.overwrite,
            item.symlink_mode,
            &mut visited_directories,
            &mut plan,
        ) {
            if is_cancellation_error(&error) {
                return Err(error);
            }

            plan.failures.push(error);
        }
    }

    let total_bytes = plan.total_bytes;
    let total_entries = plan.total_entries;
    let emit = |processed_bytes: u64, processed_entries: u64, current: Option<&Path>| {
        on_progress(OperationProgress {
            processed_bytes: processed_bytes.min(total_bytes),
            total_bytes,
            processed_entries: processed_entries.min(total_entries),
            total_entries,
            current_path: current.map(|path| path.to_string_lossy().into_owned()),
            current_bytes: 0,
            current_total_bytes: 0,
        });
    };

    emit(0, 0, None);

    // 1. Reserve directories depth-first. The final component is created with
    //    create_dir so overwrite=false cannot merge into a directory that
    //    appeared after planning.
    let mut failures = plan.failures;
    let mut processed_entries = 0_u64;
    let mut blocked_destinations = Vec::new();

    for (from, to, _metadata) in &plan.dirs {
        checkpoint(Some(from))?;

        if blocked_destinations
            .iter()
            .any(|blocked: &PathBuf| to.starts_with(blocked))
        {
            continue;
        }

        match create_destination_directory(to) {
            Ok(()) => processed_entries += 1,
            Err(error) => {
                blocked_destinations.push(to.clone());
                failures.push(error);
            }
        }
    }

    // 2. Copy files across a bounded worker pool.
    let files = &plan.files;
    let next = AtomicUsize::new(0);
    let processed_bytes = AtomicU64::new(0);
    let entries_done = AtomicU64::new(processed_entries);
    let last_emitted = AtomicU64::new(0);
    let cancelled = AtomicBool::new(false);
    let file_failures: Mutex<Vec<FsError>> = Mutex::new(Vec::new());
    let workers = concurrency.max(1).min(files.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut scratch = ProgressState::default();
                let mut noop = |_progress: OperationProgress| {};
                let mut worker_checkpoint = |path: Option<&Path>| checkpoint(path);

                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= files.len() {
                        break;
                    }

                    let task = &files[index];

                    if blocked_destinations
                        .iter()
                        .any(|blocked| task.to.starts_with(blocked))
                    {
                        continue;
                    }

                    match copy_file(
                        &task.from,
                        &task.to,
                        &task.metadata,
                        task.overwrite,
                        &mut scratch,
                        &mut noop,
                        &mut worker_checkpoint,
                    ) {
                        Ok(()) => {
                            let bytes = processed_bytes
                                .fetch_add(task.metadata.len(), Ordering::Relaxed)
                                + task.metadata.len();
                            let done = entries_done.fetch_add(1, Ordering::Relaxed) + 1;

                            // Throttle progress events; one worker claims each step.
                            let last = last_emitted.load(Ordering::Relaxed);
                            if (bytes.saturating_sub(last) >= PROGRESS_BYTE_STEP
                                || bytes >= total_bytes)
                                && last_emitted
                                    .compare_exchange(
                                        last,
                                        bytes,
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    )
                                    .is_ok()
                            {
                                emit(bytes, done, Some(&task.from));
                            }
                        }
                        Err(error) => {
                            if is_cancellation_error(&error) {
                                cancelled.store(true, Ordering::Relaxed);
                            }
                            file_failures.lock().unwrap().push(error);
                        }
                    }
                }
            });
        }
    });

    // A cancellation anywhere aborts the whole operation.
    let mut file_failures = file_failures.into_inner().unwrap();
    if file_failures.iter().any(is_cancellation_error) {
        return Err(FsError::new(
            "operation_cancelled",
            "The file operation was cancelled.",
            None,
        ));
    }

    failures.append(&mut file_failures);
    processed_entries = entries_done.load(Ordering::Relaxed);
    let copied_bytes = processed_bytes.load(Ordering::Relaxed);

    // 3. Symlinks (cheap; done sequentially after files).
    for (from, to, overwrite) in &plan.symlinks {
        checkpoint(Some(from))?;

        if blocked_destinations
            .iter()
            .any(|blocked| to.starts_with(blocked))
        {
            continue;
        }

        match copy_symlink(from, to, *overwrite) {
            Ok(()) => {
                processed_entries += 1;
                emit(copied_bytes, processed_entries, Some(from));
            }
            Err(error) => failures.push(error),
        }
    }

    // 4. Restore directory permissions/timestamps now that their contents exist.
    for (_from, to, metadata) in &plan.dirs {
        if blocked_destinations
            .iter()
            .any(|blocked| to.starts_with(blocked))
        {
            continue;
        }

        if let Ok(directory) = File::open(to) {
            preserve_metadata(&directory, metadata);
        }
    }

    emit(total_bytes, total_entries, None);
    finalize_failures(failures)
}

fn resolve_items(items: &[LocalTransferItem]) -> FsResult<Vec<ResolvedTransferItem>> {
    items
        .iter()
        .map(|item| {
            let resolved = ResolvedTransferItem {
                from: expand_path(&item.from)?,
                to: expand_path(&item.to)?,
                overwrite: item.overwrite,
                symlink_mode: item.symlink_mode,
            };
            validate_directory_transfer_target(&resolved)?;
            Ok(resolved)
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ResolvedTransferItem {
    from: PathBuf,
    to: PathBuf,
    overwrite: bool,
    symlink_mode: SymlinkMode,
}

fn validate_directory_transfer_target(item: &ResolvedTransferItem) -> FsResult<()> {
    let Ok(symlink_metadata) = fs::symlink_metadata(&item.from) else {
        // The normal measure/copy path reports missing or unreadable sources with
        // its established error codes. This guard is only about recursive targets.
        return Ok(());
    };

    if symlink_metadata.file_type().is_symlink()
        && matches!(item.symlink_mode, SymlinkMode::Preserve)
    {
        return Ok(());
    }

    let metadata = if symlink_metadata.file_type().is_symlink() {
        match fs::metadata(&item.from) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(()),
        }
    } else {
        symlink_metadata
    };

    if !metadata.is_dir() {
        return Ok(());
    }

    let source = fs::canonicalize(&item.from)
        .map_err(|error| FsError::io("Unable to resolve source directory", &item.from, error))?;
    let destination = canonicalize_with_missing_tail(&item.to)
        .map_err(|error| FsError::io("Unable to resolve destination directory", &item.to, error))?;

    if destination == source || destination.starts_with(&source) {
        return Err(FsError::new(
            "invalid_transfer_target",
            "A folder cannot be copied or moved into itself or one of its descendants.",
            Some(item.to.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

fn canonicalize_with_missing_tail(path: &Path) -> std::io::Result<PathBuf> {
    let mut ancestor = if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    };
    let mut missing = Vec::new();

    loop {
        match fs::canonicalize(&ancestor) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name().map(|name| name.to_os_string()) else {
                    return Err(error);
                };
                missing.push(name);
                if !ancestor.pop() || ancestor.as_os_str().is_empty() {
                    ancestor = PathBuf::from(".");
                }
            }
            Err(error) => return Err(error),
        }
    }
}

// Measure each item up front, collecting per-item failures (e.g. a file that
// was deleted or is unreadable) instead of aborting the whole batch. Returns
// the items that can actually be transferred plus the aggregate size for
// progress. Cancellation still propagates immediately.
fn measure_plan<C>(
    items: &[ResolvedTransferItem],
    checkpoint: &mut C,
    failures: &mut Vec<FsError>,
) -> FsResult<(Vec<ResolvedTransferItem>, Measure)>
where
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    let mut total = Measure::default();
    let mut visited_directories = HashSet::new();
    let mut plan = Vec::new();

    for item in items {
        checkpoint(Some(&item.from))?;

        match measure_path(
            &item.from,
            item.symlink_mode,
            &mut visited_directories,
            checkpoint,
        ) {
            Ok(measure) => {
                total.bytes = total.bytes.saturating_add(measure.bytes);
                total.entries = total.entries.saturating_add(measure.entries);
                plan.push(item.clone());
            }
            Err(error) if is_cancellation_error(&error) => return Err(error),
            Err(error) => failures.push(error),
        }
    }

    Ok((plan, total))
}

fn measure_path<C>(
    path: &Path,
    symlink_mode: SymlinkMode,
    visited_directories: &mut HashSet<PathBuf>,
    checkpoint: &mut C,
) -> FsResult<Measure>
where
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    checkpoint(Some(path))?;
    let symlink_metadata = fs::symlink_metadata(path)
        .map_err(|error| FsError::io("Unable to read source metadata", path, error))?;

    if symlink_metadata.file_type().is_symlink() && matches!(symlink_mode, SymlinkMode::Preserve) {
        return Ok(Measure {
            bytes: 0,
            entries: 1,
        });
    }

    let metadata = if symlink_metadata.file_type().is_symlink() {
        fs::metadata(path)
            .map_err(|error| FsError::io("Unable to read symlink target metadata", path, error))?
    } else {
        symlink_metadata
    };

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
            let Ok(child) = child else {
                continue; // unreadable entry; the copy walk surfaces real failures
            };

            match measure_path(&child.path(), symlink_mode, visited_directories, checkpoint) {
                Ok(measure) => {
                    total.bytes = total.bytes.saturating_add(measure.bytes);
                    total.entries = total.entries.saturating_add(measure.entries);
                }
                // Cancellation aborts the whole measure; other errors are left for
                // the copy/move walk to report, keeping the size estimate best-effort.
                Err(error) if is_cancellation_error(&error) => return Err(error),
                Err(_) => {}
            }
        }

        return Ok(total);
    }

    Ok(Measure {
        bytes: metadata.len(),
        entries: 1,
    })
}

// `continue_on_error` controls within-tree behavior: copies collect per-file
// failures into `failures` and keep going; moves pass `false` so any failure
// aborts before the source is deleted (no data loss). Cancellation always aborts.
#[allow(clippy::too_many_arguments)]
fn copy_path<F, C>(
    from: &Path,
    to: &Path,
    overwrite: bool,
    symlink_mode: SymlinkMode,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
    failures: &mut Vec<FsError>,
    continue_on_error: bool,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    checkpoint(Some(from))?;
    let symlink_metadata = fs::symlink_metadata(from)
        .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

    if symlink_metadata.file_type().is_symlink() && matches!(symlink_mode, SymlinkMode::Preserve) {
        if !overwrite && path_exists(to)? {
            return Err(destination_exists_error(to));
        }

        copy_symlink(from, to, overwrite)?;
        progress.current_bytes = 0;
        progress.current_total_bytes = 0;
        progress.processed_entries = progress.processed_entries.saturating_add(1);
        emit_progress(progress, Some(from), true, on_progress);
        return Ok(());
    }

    let metadata = if symlink_metadata.file_type().is_symlink() {
        fs::metadata(from)
            .map_err(|error| FsError::io("Unable to read symlink target metadata", from, error))?
    } else {
        symlink_metadata
    };

    if !overwrite && path_exists(to)? {
        return Err(destination_exists_error(to));
    }

    if metadata.is_dir() {
        if overwrite && path_exists(to)? {
            return Err(destination_type_error(to));
        }

        create_destination_directory(to)?;
        progress.current_bytes = 0;
        progress.current_total_bytes = 0;
        progress.processed_entries = progress.processed_entries.saturating_add(1);
        emit_progress(progress, Some(from), true, on_progress);

        for child in fs::read_dir(from)
            .map_err(|error| FsError::io("Unable to read source directory", from, error))?
        {
            let child = match child {
                Ok(child) => child,
                Err(error) if continue_on_error => {
                    failures.push(FsError::io(
                        "Unable to read source directory entry",
                        from,
                        error,
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(FsError::io(
                        "Unable to read source directory entry",
                        from,
                        error,
                    ))
                }
            };

            match copy_path(
                &child.path(),
                &to.join(child.file_name()),
                overwrite,
                symlink_mode,
                progress,
                on_progress,
                checkpoint,
                failures,
                continue_on_error,
            ) {
                Ok(()) => {}
                // Cancellation stops everything; otherwise skip just this child.
                Err(error) if is_cancellation_error(&error) => return Err(error),
                Err(error) if continue_on_error => failures.push(error),
                Err(error) => return Err(error),
            }
        }

        // Set directory permissions/timestamps last so copying children doesn't
        // bump the mtime we just restored. Opening a directory handle is Unix-
        // only; on other platforms the directory keeps fresh metadata.
        if let Ok(directory) = File::open(to) {
            preserve_metadata(&directory, &metadata);
        }

        return Ok(());
    }

    if overwrite && path_exists(to)? {
        let target_metadata = fs::symlink_metadata(to)
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

fn create_destination_directory(to: &Path) -> FsResult<()> {
    if let Some(parent) = to.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| {
            FsError::io(
                "Unable to create destination directory parent",
                parent,
                error,
            )
        })?;
    }

    match fs::create_dir(to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(destination_exists_error(to))
        }
        Err(error) => Err(FsError::io(
            "Unable to create destination directory",
            to,
            error,
        )),
    }
}

fn move_path<F, C>(
    from: &Path,
    to: &Path,
    overwrite: bool,
    symlink_mode: SymlinkMode,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    checkpoint(Some(from))?;
    let source_metadata = fs::symlink_metadata(from)
        .map_err(|error| FsError::io("Unable to read source metadata", from, error))?;

    if source_metadata.file_type().is_symlink() && matches!(symlink_mode, SymlinkMode::Follow) {
        let temporary_to = temporary_move_path(to)?;

        if let Err(copy_error) = copy_path(
            from,
            &temporary_to,
            false,
            symlink_mode,
            progress,
            on_progress,
            checkpoint,
            &mut Vec::new(),
            false,
        ) {
            cleanup_partial_copy(&temporary_to);
            return Err(copy_error);
        }

        if let Err(checkpoint_error) = checkpoint(Some(from)) {
            cleanup_partial_copy(&temporary_to);
            return Err(checkpoint_error);
        }

        if let Err(place_error) = place_temporary_move(&temporary_to, to, overwrite) {
            cleanup_partial_copy(&temporary_to);
            return Err(place_error);
        }

        return delete_path(from);
    }

    if !overwrite && path_exists(to)? {
        return Err(destination_exists_error(to));
    }

    if overwrite && path_exists(to)? {
        let target_metadata = fs::symlink_metadata(to)
            .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;

        if source_metadata.is_dir() || target_metadata.is_dir() {
            return Err(destination_type_error(to));
        }
    }

    // Use the same platform commit primitive as staged replacements so
    // overwrite=false cannot clobber an item created after the path_exists
    // check, while overwrite=true still gets the native atomic replacement.
    match commit_staged_path(from, to, overwrite) {
        Ok(()) => {
            let mut visited_directories = HashSet::new();
            let measure = measure_path(to, symlink_mode, &mut visited_directories, checkpoint)?;
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
                symlink_mode,
                progress,
                on_progress,
                checkpoint,
                &mut Vec::new(),
                false,
            ) {
                cleanup_partial_copy(&temporary_to);
                return Err(copy_error);
            }

            if let Err(checkpoint_error) = checkpoint(Some(from)) {
                cleanup_partial_copy(&temporary_to);
                return Err(checkpoint_error);
            }

            if let Err(place_error) = place_temporary_move(&temporary_to, to, overwrite) {
                cleanup_partial_copy(&temporary_to);
                return Err(place_error);
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
    copy_file_with_contents(
        from,
        to,
        metadata,
        overwrite,
        progress,
        on_progress,
        checkpoint,
        |reader, writer, progress, on_progress, checkpoint| {
            copy_file_contents(
                reader,
                writer,
                from,
                to,
                metadata.len(),
                progress,
                on_progress,
                checkpoint,
            )
        },
    )
}

// Overwrites are staged beside the destination. The existing item is not
// touched until the complete replacement has been written and flushed, then a
// same-directory commit asks the platform for its strongest replacement
// guarantee (rename on Unix, MoveFileExW on Windows).
// Keeping the writer inside this function also guarantees that it is closed
// before the rename, which is required by Windows.
#[allow(clippy::too_many_arguments)]
fn copy_file_with_contents<F, C, W>(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    overwrite: bool,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
    copy_contents: W,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
    W: FnOnce(&File, &mut File, &mut ProgressState, &mut F, &mut C) -> FsResult<()>,
{
    let staged_to = if overwrite {
        Some(temporary_copy_path(to)?)
    } else {
        None
    };
    let write_to = staged_to.as_deref().unwrap_or(to);

    let reader =
        File::open(from).map_err(|error| FsError::io("Unable to open source file", from, error))?;
    let result = (|| {
        let mut writer = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(write_to)
            .map_err(|error| FsError::io("Unable to create destination file", to, error))?;

        progress.current_bytes = 0;
        progress.current_total_bytes = metadata.len();
        emit_progress(progress, Some(from), true, on_progress);

        copy_contents(&reader, &mut writer, progress, on_progress, checkpoint)?;

        // Preserve permissions and timestamps. Best-effort: some targets (FAT,
        // certain network shares) can't store them, and that must not fail the copy.
        preserve_metadata(&writer, metadata);

        if staged_to.is_some() {
            writer
                .sync_all()
                .map_err(|error| FsError::io("Unable to flush replacement file", to, error))?;
        }

        // Drop the handle before committing so Windows can rename the staged
        // file over an existing destination.
        drop(writer);

        if let Some(staged_to) = staged_to.as_deref() {
            checkpoint(Some(from))?;
            place_staged_item(staged_to, to, true, "Unable to replace destination")?;
        }

        progress.current_bytes = progress.current_total_bytes;
        progress.processed_entries = progress.processed_entries.saturating_add(1);
        emit_progress(progress, Some(from), true, on_progress);
        Ok(())
    })();

    if result.is_err() {
        if let Some(staged_to) = staged_to.as_deref() {
            cleanup_partial_copy(staged_to);
        }
    }

    result
}

// Copy file contents with the fastest method the platform/filesystem offers,
// falling back to a portable buffered loop. Progress and cancellation are
// always honored.
fn copy_file_contents<F, C>(
    reader: &File,
    writer: &mut File,
    from: &Path,
    to: &Path,
    total: u64,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    #[cfg(target_os = "linux")]
    {
        // Reflink: an instant copy-on-write clone on Btrfs/XFS/bcachefs/etc.
        if reflink_clone(reader, writer) {
            progress.processed_bytes = progress.processed_bytes.saturating_add(total);
            progress.current_bytes = total;
            emit_progress(progress, Some(from), true, on_progress);
            return Ok(());
        }

        // copy_file_range: in-kernel copy that also preserves sparse regions.
        // Returns false only when the syscall is unsupported for this pair, so
        // the buffered fallback finishes from the (still in-sync) file offsets.
        if copy_file_range_loop(reader, writer, from, progress, on_progress, checkpoint)? {
            return Ok(());
        }
    }

    // `total` only feeds the Linux fast paths; the buffered loop tracks its own.
    #[cfg(not(target_os = "linux"))]
    let _ = total;

    copy_buffered(reader, writer, from, to, progress, on_progress, checkpoint)
}

#[cfg(target_os = "linux")]
fn reflink_clone(reader: &File, writer: &File) -> bool {
    use std::os::unix::io::AsRawFd;

    // FICLONE = _IOW(0x94, 9, int); identical across Linux architectures.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    // SAFETY: both descriptors are valid, open files for the duration of the call.
    let result = unsafe { libc::ioctl(writer.as_raw_fd(), FICLONE, reader.as_raw_fd()) };
    result == 0
}

#[cfg(target_os = "linux")]
fn copy_file_range_loop<F, C>(
    reader: &File,
    writer: &File,
    from: &Path,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<bool>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    use std::os::unix::io::AsRawFd;

    // Cap each call so cancellation and progress stay responsive on huge files.
    // Looping to EOF (rather than a precomputed size) is robust if the source
    // changes size between measuring and copying.
    const CHUNK: usize = 16 * 1024 * 1024;
    let src = reader.as_raw_fd();
    let dst = writer.as_raw_fd();
    let mut first = true;

    loop {
        checkpoint(Some(from))?;
        // SAFETY: descriptors are valid; null offsets advance the file positions.
        let copied = unsafe {
            libc::copy_file_range(
                src,
                std::ptr::null_mut(),
                dst,
                std::ptr::null_mut(),
                CHUNK,
                0,
            )
        };

        if copied < 0 {
            let error = std::io::Error::last_os_error();

            // On the first call these errno values mean "not supported for this
            // pair of files" — let the caller finish with the buffered loop.
            if first
                && matches!(
                    error.raw_os_error(),
                    Some(libc::ENOSYS)
                        | Some(libc::EXDEV)
                        | Some(libc::EOPNOTSUPP)
                        | Some(libc::EINVAL)
                        | Some(libc::EBADF)
                )
            {
                return Ok(false);
            }

            return Err(FsError::io("Unable to copy file", from, error));
        }

        if copied == 0 {
            break; // EOF
        }

        let copied = copied as u64;
        progress.processed_bytes = progress.processed_bytes.saturating_add(copied);
        progress.current_bytes = progress.current_bytes.saturating_add(copied);
        emit_progress(progress, Some(from), false, on_progress);
        first = false;
    }

    Ok(true)
}

fn copy_buffered<F, C>(
    reader: &File,
    writer: &mut File,
    from: &Path,
    to: &Path,
    progress: &mut ProgressState,
    on_progress: &mut F,
    checkpoint: &mut C,
) -> FsResult<()>
where
    F: FnMut(OperationProgress),
    C: FnMut(Option<&Path>) -> FsResult<()>,
{
    // `&File` implements Read, so the source position keeps advancing even when
    // a partial copy_file_range run handed control over to us.
    let mut reader = reader;
    let mut buffer = [0_u8; 256 * 1024];

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

    Ok(())
}

// Best-effort copy of permissions and timestamps from the source metadata onto
// an already-open destination handle (a file or a directory on Unix).
fn preserve_metadata(file: &File, source: &fs::Metadata) {
    let _ = file.set_permissions(source.permissions());

    if let Ok(modified) = source.modified() {
        let mut times = fs::FileTimes::new().set_modified(modified);

        if let Ok(accessed) = source.accessed() {
            times = times.set_accessed(accessed);
        }

        let _ = file.set_times(times);
    }
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

fn copy_symlink(from: &Path, to: &Path, overwrite: bool) -> FsResult<()> {
    copy_symlink_with_rename(from, to, overwrite, |from, to| {
        commit_staged_path(from, to, overwrite)
    })
}

fn copy_symlink_with_rename<R>(from: &Path, to: &Path, overwrite: bool, rename: R) -> FsResult<()>
where
    R: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    if !overwrite && path_exists(to)? {
        return Err(destination_exists_error(to));
    }

    let target = fs::read_link(from)
        .map_err(|error| FsError::io("Unable to read symbolic link", from, error))?;

    if !overwrite {
        return create_symlink(&target, to, from);
    }

    let staged_to = temporary_copy_path(to)?;
    if let Err(error) = create_symlink(&target, &staged_to, from) {
        cleanup_partial_copy(&staged_to);
        return Err(error);
    }

    place_staged_item_with_rename(
        &staged_to,
        to,
        true,
        "Unable to replace destination",
        rename,
    )
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

fn temporary_copy_path(to: &Path) -> FsResult<PathBuf> {
    temporary_operation_path(to, "copy")
}

// A narrow crate-internal boundary for code paths (such as remote downloads)
// that produce a complete local replacement themselves. Dropping an
// uncommitted stage always removes the hidden sibling; after a successful
// commit the staged path no longer exists, so the same cleanup is harmless.
pub(crate) struct LocalReplacementStage {
    path: PathBuf,
}

impl LocalReplacementStage {
    pub(crate) fn new(destination: &Path) -> FsResult<Self> {
        Ok(Self {
            path: temporary_copy_path(destination)?,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn commit(self, destination: &Path) -> FsResult<()> {
        place_staged_item(
            &self.path,
            destination,
            true,
            "Unable to replace destination",
        )
    }
}

impl Drop for LocalReplacementStage {
    fn drop(&mut self) {
        cleanup_partial_copy(&self.path);
    }
}

fn temporary_move_path(to: &Path) -> FsResult<PathBuf> {
    temporary_operation_path(to, "move")
}

fn temporary_operation_path(to: &Path, operation: &str) -> FsResult<PathBuf> {
    static TEMPORARY_PATH_SEQUENCE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    // The process-wide sequence makes candidates unique even when many copy
    // workers observe the same clock tick. create_new / symlink creation still
    // provides the final guard against a stale path from another process.
    let sequence = TEMPORARY_PATH_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    for attempt in 0..100 {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = parent.join(format!(
            ".carelo-{operation}-{}-{nonce}-{sequence}-{attempt}.tmp",
            std::process::id(),
        ));

        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => {
                return Err(FsError::io(
                    "Unable to inspect temporary destination",
                    &candidate,
                    error,
                ))
            }
        }
    }

    Err(FsError::new(
        "temporary_path_unavailable",
        format!("Unable to reserve a temporary destination for the {operation}."),
        Some(to.to_string_lossy().into_owned()),
    ))
}

fn place_temporary_move(temporary_to: &Path, to: &Path, overwrite: bool) -> FsResult<()> {
    place_temporary_move_with_rename(temporary_to, to, overwrite, |from, to| {
        commit_staged_path(from, to, overwrite)
    })
}

fn place_temporary_move_with_rename<R>(
    temporary_to: &Path,
    to: &Path,
    overwrite: bool,
    rename: R,
) -> FsResult<()>
where
    R: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    if let Err(error) = sync_staged_item(temporary_to, to) {
        cleanup_partial_copy(temporary_to);
        return Err(error);
    }

    place_staged_item_with_rename(
        temporary_to,
        to,
        overwrite,
        "Unable to place moved item",
        rename,
    )
}

fn sync_staged_item(staged_to: &Path, reported_path: &Path) -> FsResult<()> {
    let metadata = fs::symlink_metadata(staged_to)
        .map_err(|error| FsError::io("Unable to read staged item", reported_path, error))?;

    if metadata.is_file() {
        File::open(staged_to)
            .and_then(|file| file.sync_all())
            .map_err(|error| FsError::io("Unable to flush staged item", reported_path, error))?;
    }

    Ok(())
}

fn place_staged_item(staged_to: &Path, to: &Path, overwrite: bool, action: &str) -> FsResult<()> {
    place_staged_item_with_rename(staged_to, to, overwrite, action, |from, to| {
        commit_staged_path(from, to, overwrite)
    })
}

fn place_staged_item_with_rename<R>(
    staged_to: &Path,
    to: &Path,
    overwrite: bool,
    action: &str,
    rename: R,
) -> FsResult<()>
where
    R: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let result = (|| {
        if path_exists(to)? {
            if !overwrite {
                return Err(destination_exists_error(to));
            }

            let metadata = fs::symlink_metadata(to)
                .map_err(|error| FsError::io("Unable to read destination metadata", to, error))?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                return Err(destination_type_error(to));
            }
        }

        rename(staged_to, to).map_err(|error| FsError::io(action, to, error))
    })();

    if result.is_err() {
        cleanup_partial_copy(staged_to);
    } else {
        // The file data is synced before commit. Syncing the containing
        // directory as well makes the rename durable across power loss on
        // platforms/filesystems that support directory fsync. Unsupported
        // directory handles are deliberately best-effort after a successful
        // commit because the replacement can no longer be rolled back safely.
        sync_parent_directory(to);
    }

    result
}

#[cfg(unix)]
fn commit_staged_path(staged_to: &Path, to: &Path, overwrite: bool) -> std::io::Result<()> {
    if overwrite {
        // POSIX rename replaces a non-directory destination atomically when
        // both paths are in the same directory/filesystem.
        fs::rename(staged_to, to)
    } else {
        commit_staged_path_no_clobber(staged_to, to)
    }
}

#[cfg(target_os = "linux")]
fn commit_staged_path_no_clobber(staged_to: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let staged = CString::new(staged_to.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "staged path contains an interior NUL byte",
        )
    })?;
    let destination = CString::new(to.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination path contains an interior NUL byte",
        )
    })?;

    // SAFETY: both C strings are NUL-terminated and remain alive for the call.
    // RENAME_NOREPLACE makes the existence check part of the rename itself,
    // closing the path_exists/rename race without ever deleting the contender.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            staged.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };

    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn commit_staged_path_no_clobber(staged_to: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    const RENAME_EXCL: u32 = 0x0000_0004;

    unsafe extern "C" {
        fn renamex_np(
            from: *const libc::c_char,
            to: *const libc::c_char,
            flags: u32,
        ) -> libc::c_int;
    }

    let staged = CString::new(staged_to.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "staged path contains an interior NUL byte",
        )
    })?;
    let destination = CString::new(to.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination path contains an interior NUL byte",
        )
    })?;

    // SAFETY: both C strings are NUL-terminated and remain alive for the call.
    let result = unsafe { renamex_np(staged.as_ptr(), destination.as_ptr(), RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn commit_staged_path_no_clobber(staged_to: &Path, to: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(staged_to)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-clobber directory placement is unsupported on this platform",
        ));
    }

    // link is atomic and fails if `to` already exists. Removing the staged hard
    // link afterwards leaves the committed name intact.
    fs::hard_link(staged_to, to)?;
    fs::remove_file(staged_to)
}

#[cfg(windows)]
fn commit_staged_path(staged_to: &Path, to: &Path, overwrite: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let staged: Vec<u16> = staged_to.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = MOVEFILE_WRITE_THROUGH
        | if overwrite {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };

    // SAFETY: both UTF-16 paths are NUL-terminated and remain alive for the
    // call. Without REPLACE_EXISTING, MoveFileExW is the Windows no-clobber
    // primitive; with it, Windows replaces the destination in one operation.
    let result = unsafe { MoveFileExW(staged.as_ptr(), destination.as_ptr(), flags) };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn commit_staged_path(staged_to: &Path, to: &Path, overwrite: bool) -> std::io::Result<()> {
    if !overwrite && to.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }

    fs::rename(staged_to, to)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) {}

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

    fn carelo_temporary_items(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .expect("read test directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".carelo-"))
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn directory_copy_and_move_reject_descendant_targets_before_mutation() {
        let root = test_root("descendant-target");
        let source = root.join("source");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("file.txt"), "data").expect("write source file");

        let sequential_destination = source.join("nested/sequential");
        let sequential_error = copy_items_with_progress(
            &[dir_item(&source, &sequential_destination)],
            |_| {},
            |_| Ok(()),
        )
        .expect_err("sequential copy into a descendant must fail");
        assert_eq!(sequential_error.code, "invalid_transfer_target");
        assert!(!sequential_destination.exists());

        let parallel_destination = source.join("nested/parallel");
        let parallel_error = copy_items_parallel(
            &[dir_item(&source, &parallel_destination)],
            4,
            |_| {},
            |_| Ok(()),
        )
        .expect_err("parallel copy into a descendant must fail");
        assert_eq!(parallel_error.code, "invalid_transfer_target");
        assert!(!parallel_destination.exists());

        let move_destination = source.join("nested/moved");
        let move_error =
            move_items_with_progress(&[dir_item(&source, &move_destination)], |_| {}, |_| Ok(()))
                .expect_err("move into a descendant must fail");
        assert_eq!(move_error.code, "invalid_transfer_target");
        assert!(source.exists());
        assert!(!move_destination.exists());

        cleanup(&root);
    }

    #[test]
    fn directory_reservation_never_merges_into_a_late_destination() {
        let root = test_root("directory-no-clobber");
        let destination = root.join("destination");
        fs::create_dir(&destination).expect("create racing destination");
        fs::write(destination.join("external.txt"), "external").expect("write external file");

        let error = create_destination_directory(&destination)
            .expect_err("an existing directory must not be merged into");

        assert_eq!(error.code, "destination_exists");
        assert_eq!(
            fs::read_to_string(destination.join("external.txt")).expect("read external file"),
            "external"
        );

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn descendant_guard_resolves_existing_symlinked_destination_parents() {
        let root = test_root("descendant-symlink-parent");
        let source = root.join("source");
        let inside = source.join("inside");
        let alias = root.join("inside-alias");
        fs::create_dir_all(&inside).expect("create source child");
        std::os::unix::fs::symlink(&inside, &alias).expect("create destination parent alias");
        let destination = alias.join("copy");

        let error =
            copy_items_with_progress(&[dir_item(&source, &destination)], |_| {}, |_| Ok(()))
                .expect_err("symlinked descendant must fail");

        assert_eq!(error.code, "invalid_transfer_target");
        assert!(!destination.exists());
        cleanup(&root);
    }

    #[test]
    fn overwrite_copy_cancellation_preserves_destination_and_cleans_stage() {
        let root = test_root("overwrite-copy-cancel");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "complete replacement").expect("write source");
        fs::write(&destination, "original destination").expect("write destination");
        let checkpoint_root = root.clone();

        let error = copy_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: true,
                symlink_mode: SymlinkMode::Preserve,
            }],
            |_| {},
            |_| {
                if !carelo_temporary_items(&checkpoint_root).is_empty() {
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
        .expect_err("overwrite should stop before committing the staged copy");

        assert_eq!(error.code, "operation_cancelled");
        assert_eq!(
            fs::read_to_string(&destination).expect("read original destination"),
            "original destination"
        );
        assert_eq!(
            fs::read_to_string(&source).expect("source should remain"),
            "complete replacement"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "cancelled overwrite left a Carelo temporary item"
        );

        cleanup(&root);
    }

    #[test]
    fn overwrite_copy_write_failure_preserves_destination_and_cleans_stage() {
        let root = test_root("overwrite-copy-write-fault");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "complete replacement").expect("write source");
        fs::write(&destination, "original destination").expect("write destination");
        let metadata = fs::metadata(&source).expect("source metadata");
        let mut progress = ProgressState::default();
        let mut on_progress = |_progress: OperationProgress| {};
        let mut checkpoint = |_path: Option<&Path>| Ok(());

        let error = copy_file_with_contents(
            &source,
            &destination,
            &metadata,
            true,
            &mut progress,
            &mut on_progress,
            &mut checkpoint,
            |_reader, writer, _progress, _on_progress, _checkpoint| {
                writer
                    .write_all(b"partial replacement")
                    .expect("stage partial data");
                Err(FsError::new(
                    "injected_write_failure",
                    "Injected destination write failure.",
                    Some(destination.to_string_lossy().into_owned()),
                ))
            },
        )
        .expect_err("injected write failure should abort replacement");

        assert_eq!(error.code, "injected_write_failure");
        assert_eq!(
            fs::read_to_string(&destination).expect("read original destination"),
            "original destination"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "failed overwrite left a Carelo temporary item"
        );

        cleanup(&root);
    }

    #[test]
    fn overwrite_copy_commits_replacement_and_leaves_no_stage() {
        let root = test_root("overwrite-copy-commit");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "complete replacement").expect("write source");
        fs::write(&destination, "original destination").expect("write destination");

        copy_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: true,
                symlink_mode: SymlinkMode::Preserve,
            }],
            |_| {},
            |_| Ok(()),
        )
        .expect("commit staged overwrite");

        assert_eq!(
            fs::read_to_string(&destination).expect("read replacement"),
            "complete replacement"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "successful overwrite left a Carelo temporary item"
        );

        cleanup(&root);
    }

    #[test]
    fn cross_volume_move_commit_failure_preserves_destination_and_cleans_stage() {
        let root = test_root("move-commit-fault");
        let destination = root.join("destination.txt");
        fs::write(&destination, "original destination").expect("write destination");
        let staged = root.join(format!(".carelo-move-{}-injected.tmp", std::process::id()));
        fs::write(&staged, "complete replacement").expect("write staged move");

        let error = place_temporary_move_with_rename(&staged, &destination, true, |_from, _to| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected final placement failure",
            ))
        })
        .expect_err("injected placement failure should abort move");

        assert_eq!(error.code, "permission_denied");
        assert_eq!(
            fs::read_to_string(&destination).expect("read original destination"),
            "original destination"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "failed move placement left a Carelo temporary item"
        );

        cleanup(&root);
    }

    #[test]
    fn late_destination_cannot_be_clobbered_by_no_overwrite_commit() {
        let root = test_root("move-late-destination");
        let destination = root.join("destination.txt");
        let staged = root.join(format!(
            ".carelo-move-{}-late-destination.tmp",
            std::process::id()
        ));
        fs::write(&staged, "staged move").expect("write staged move");

        let error = place_temporary_move_with_rename(&staged, &destination, false, |from, to| {
            // Simulate another process winning the race after the initial
            // path_exists check but before the actual placement syscall.
            fs::write(to, "racing destination")?;
            commit_staged_path(from, to, false)
        })
        .expect_err("no-overwrite placement must not clobber a late destination");

        assert_eq!(error.code, "io_error");
        assert_eq!(
            fs::read_to_string(&destination).expect("read racing destination"),
            "racing destination"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "no-clobber failure left a Carelo temporary item"
        );

        cleanup(&root);
    }

    #[test]
    fn temporary_copy_paths_are_unique_across_parallel_workers() {
        use std::collections::HashSet;
        use std::sync::Mutex;

        let root = test_root("parallel-temporary-paths");
        let destination = root.join("destination.txt");
        let paths = Mutex::new(Vec::new());

        std::thread::scope(|scope| {
            for _ in 0..64 {
                scope.spawn(|| {
                    let path = temporary_copy_path(&destination).expect("temporary copy path");
                    paths.lock().expect("path lock").push(path);
                });
            }
        });

        let paths = paths.into_inner().expect("collected paths");
        let unique: HashSet<_> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len());
        assert!(carelo_temporary_items(&root).is_empty());

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn preserved_symlink_commit_failure_keeps_old_link_and_cleans_stage() {
        let root = test_root("symlink-commit-fault");
        let old_target = root.join("old-target.txt");
        let new_target = root.join("new-target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&old_target, "old").expect("write old target");
        fs::write(&new_target, "new").expect("write new target");
        std::os::unix::fs::symlink("new-target.txt", &source).expect("create source link");
        std::os::unix::fs::symlink("old-target.txt", &destination)
            .expect("create destination link");

        let error = copy_symlink_with_rename(&source, &destination, true, |_from, _to| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected symlink placement failure",
            ))
        })
        .expect_err("injected placement failure should preserve destination link");

        assert_eq!(error.code, "permission_denied");
        assert_eq!(
            fs::read_link(&destination).expect("read destination link"),
            PathBuf::from("old-target.txt")
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("read old link target"),
            "old"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "failed symlink replacement left a Carelo temporary item"
        );

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn preserved_symlink_overwrite_commits_new_link_and_cleans_stage() {
        let root = test_root("symlink-overwrite-commit");
        let old_target = root.join("old-target.txt");
        let new_target = root.join("new-target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&old_target, "old").expect("write old target");
        fs::write(&new_target, "new").expect("write new target");
        std::os::unix::fs::symlink("new-target.txt", &source).expect("create source link");
        std::os::unix::fs::symlink("old-target.txt", &destination)
            .expect("create destination link");

        copy_symlink(&source, &destination, true).expect("replace preserved symlink");

        assert_eq!(
            fs::read_link(&destination).expect("read destination link"),
            PathBuf::from("new-target.txt")
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("read new link target"),
            "new"
        );
        assert!(
            carelo_temporary_items(&root).is_empty(),
            "successful symlink overwrite left a Carelo temporary item"
        );

        cleanup(&root);
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
                symlink_mode: SymlinkMode::Preserve,
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

    fn copy_one(from: &Path, to: &Path) -> FsResult<()> {
        copy_items_with_progress(
            &[LocalTransferItem {
                from: from.to_string_lossy().into_owned(),
                to: to.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Preserve,
            }],
            |_| {},
            |_| Ok(()),
        )
    }

    fn dir_item(from: &Path, to: &Path) -> LocalTransferItem {
        LocalTransferItem {
            from: from.to_string_lossy().into_owned(),
            to: to.to_string_lossy().into_owned(),
            overwrite: false,
            symlink_mode: SymlinkMode::Preserve,
        }
    }

    #[test]
    fn parallel_copy_replicates_a_nested_tree_byte_for_byte() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root("parallel-tree");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(src.join("sub/deeper")).expect("nested src");

        // Many files so the worker pool genuinely overlaps; varied content so a
        // mixed-up copy would be detected.
        for index in 0..120 {
            let payload = format!("file-{index}-").repeat(64);
            fs::write(src.join(format!("f{index}.bin")), &payload).expect("write file");
        }
        for index in 0..40 {
            fs::write(
                src.join("sub").join(format!("g{index}.bin")),
                format!("g{index}"),
            )
            .expect("write sub file");
        }
        fs::write(src.join("sub/deeper/leaf.txt"), "leaf").expect("write leaf");
        #[cfg(unix)]
        fs::set_permissions(&src.join("f0.bin"), fs::Permissions::from_mode(0o640)).expect("chmod");

        copy_items_parallel(&[dir_item(&src, &dst)], 8, |_progress| {}, |_path| Ok(()))
            .expect("parallel copy");

        for index in 0..120 {
            let payload = format!("file-{index}-").repeat(64);
            assert_eq!(
                fs::read(dst.join(format!("f{index}.bin"))).expect("read copied file"),
                payload.into_bytes(),
                "f{index} content mismatch"
            );
        }
        for index in 0..40 {
            assert_eq!(
                fs::read_to_string(dst.join("sub").join(format!("g{index}.bin")))
                    .expect("read sub file"),
                format!("g{index}")
            );
        }
        assert_eq!(
            fs::read_to_string(dst.join("sub/deeper/leaf.txt")).expect("read leaf"),
            "leaf"
        );
        // Per-file metadata is preserved on the parallel path too.
        assert_eq!(
            fs::metadata(dst.join("f0.bin"))
                .expect("dst meta")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );

        cleanup(&root);
    }

    #[test]
    fn parallel_copy_continues_past_a_failed_item_and_reports() {
        let root = test_root("parallel-continue");
        let src = root.join("src");
        let good_dst = root.join("good");
        fs::create_dir_all(&src).expect("src");
        for index in 0..10 {
            fs::write(src.join(format!("f{index}.txt")), format!("c{index}")).expect("write");
        }

        let result = copy_items_parallel(
            &[
                // A missing source aborts only its own item.
                dir_item(&root.join("does-not-exist"), &root.join("missing-dst")),
                dir_item(&src, &good_dst),
            ],
            4,
            |_progress| {},
            |_path| Ok(()),
        );

        assert!(result.is_err());
        for index in 0..10 {
            assert_eq!(
                fs::read_to_string(good_dst.join(format!("f{index}.txt"))).expect("good copy"),
                format!("c{index}")
            );
        }

        cleanup(&root);
    }

    #[test]
    fn parallel_copy_aborts_on_cancellation() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = test_root("parallel-cancel");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(&src).expect("src");
        for index in 0..40 {
            fs::write(src.join(format!("f{index}.txt")), "data").expect("write");
        }

        let calls = AtomicUsize::new(0);
        let result = copy_items_parallel(
            &[dir_item(&src, &dst)],
            4,
            |_progress| {},
            |_path| {
                if calls.fetch_add(1, Ordering::Relaxed) > 2 {
                    Err(FsError::new("operation_cancelled", "cancelled", None))
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(
            result.expect_err("cancelled copy should error").code,
            "operation_cancelled"
        );

        cleanup(&root);
    }

    // A broken symlink copied in Follow mode is a portable way to make one file
    // *inside* a tree fail while its siblings remain perfectly copyable.
    #[cfg(unix)]
    fn tree_with_one_bad_file(src: &Path) {
        fs::create_dir_all(src.join("sub")).expect("nested src");
        fs::write(src.join("a.txt"), "a").expect("a");
        fs::write(src.join("b.txt"), "b").expect("b");
        fs::write(src.join("sub/c.txt"), "c").expect("c");
        std::os::unix::fs::symlink("/no/such/target-xyz", src.join("broken")).expect("symlink");
    }

    #[cfg(unix)]
    #[test]
    fn within_tree_continue_on_error_sequential() {
        let root = test_root("within-tree-seq");
        let src = root.join("src");
        let dst = root.join("dst");
        tree_with_one_bad_file(&src);

        let result = copy_items_with_progress(
            &[LocalTransferItem {
                from: src.to_string_lossy().into_owned(),
                to: dst.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Follow,
            }],
            |_| {},
            |_| Ok(()),
        );

        // The bad file is reported, but its siblings (including a nested one)
        // are still copied.
        assert!(result.is_err(), "the broken link should be reported");
        assert_eq!(
            fs::read_to_string(dst.join("a.txt")).expect("a copied"),
            "a"
        );
        assert_eq!(
            fs::read_to_string(dst.join("b.txt")).expect("b copied"),
            "b"
        );
        assert_eq!(
            fs::read_to_string(dst.join("sub/c.txt")).expect("nested copied"),
            "c"
        );

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn within_tree_continue_on_error_parallel() {
        let root = test_root("within-tree-par");
        let src = root.join("src");
        let dst = root.join("dst");
        tree_with_one_bad_file(&src);

        let result = copy_items_parallel(
            &[LocalTransferItem {
                from: src.to_string_lossy().into_owned(),
                to: dst.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Follow,
            }],
            4,
            |_| {},
            |_| Ok(()),
        );

        assert!(result.is_err(), "the broken link should be reported");
        assert_eq!(
            fs::read_to_string(dst.join("a.txt")).expect("a copied"),
            "a"
        );
        assert_eq!(
            fs::read_to_string(dst.join("b.txt")).expect("b copied"),
            "b"
        );
        assert_eq!(
            fs::read_to_string(dst.join("sub/c.txt")).expect("nested copied"),
            "c"
        );

        cleanup(&root);
    }

    #[test]
    fn copy_handles_empty_file() {
        let root = test_root("copy-empty");
        let source = root.join("empty.bin");
        let destination = root.join("empty-copy.bin");
        fs::write(&source, b"").expect("write empty source");

        copy_one(&source, &destination).expect("copy empty file");

        let metadata = fs::metadata(&destination).expect("destination metadata");
        assert!(metadata.is_file());
        assert_eq!(metadata.len(), 0);

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_permissions_and_mtime() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root("copy-metadata");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "metadata test").expect("write source");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).expect("chmod source");

        // Stamp a distinctive past mtime so a preserved copy can't be confused
        // with a freshly-created "now" timestamp.
        let past = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_400_000_000);
        File::open(&source)
            .expect("open source")
            .set_times(fs::FileTimes::new().set_modified(past))
            .expect("set source mtime");

        let source_meta = fs::metadata(&source).expect("source metadata");
        copy_one(&source, &destination).expect("copy file");
        let dest_meta = fs::metadata(&destination).expect("destination metadata");

        assert_eq!(dest_meta.permissions().mode() & 0o777, 0o640);
        assert_eq!(
            dest_meta.modified().expect("destination mtime"),
            source_meta.modified().expect("source mtime"),
        );

        cleanup(&root);
    }

    fn missing_item(root: &Path, name: &str) -> LocalTransferItem {
        LocalTransferItem {
            from: root.join(name).to_string_lossy().into_owned(),
            to: root
                .join(format!("{name}-copy"))
                .to_string_lossy()
                .into_owned(),
            overwrite: false,
            symlink_mode: SymlinkMode::Preserve,
        }
    }

    #[test]
    fn copy_continues_after_failed_item_and_aggregates() {
        let root = test_root("copy-continue");
        let good_source = root.join("good.txt");
        let good_dest = root.join("good-copy.txt");
        fs::write(&good_source, "good").expect("write good source");

        // Two missing items surround a healthy one; the healthy one must still
        // copy and the (multiple) failures aggregate into one reported error.
        let error = copy_items_with_progress(
            &[
                missing_item(&root, "missing-a.txt"),
                LocalTransferItem {
                    from: good_source.to_string_lossy().into_owned(),
                    to: good_dest.to_string_lossy().into_owned(),
                    overwrite: false,
                    symlink_mode: SymlinkMode::Preserve,
                },
                missing_item(&root, "missing-b.txt"),
            ],
            |_| {},
            |_| Ok(()),
        )
        .expect_err("failed items should be reported");

        assert_eq!(error.code, "operation_partial_failure");
        assert_eq!(
            fs::read_to_string(&good_dest).expect("healthy item should still copy"),
            "good"
        );
        assert!(!root.join("missing-a.txt-copy").exists());

        cleanup(&root);
    }

    #[test]
    fn copy_with_single_failure_preserves_original_error() {
        let root = test_root("copy-single-failure");
        let good_source = root.join("good.txt");
        let good_dest = root.join("good-copy.txt");
        fs::write(&good_source, "good").expect("write good source");

        // A lone failure is reported verbatim (not aggregated) so its code still
        // drives sudo escalation; the healthy item is still copied.
        let error = copy_items_with_progress(
            &[
                missing_item(&root, "missing.txt"),
                LocalTransferItem {
                    from: good_source.to_string_lossy().into_owned(),
                    to: good_dest.to_string_lossy().into_owned(),
                    overwrite: false,
                    symlink_mode: SymlinkMode::Preserve,
                },
            ],
            |_| {},
            |_| Ok(()),
        )
        .expect_err("the failed item should be reported");

        assert_ne!(error.code, "operation_partial_failure");
        assert_eq!(
            fs::read_to_string(&good_dest).expect("healthy item should still copy"),
            "good"
        );

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
                symlink_mode: SymlinkMode::Preserve,
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

    #[cfg(unix)]
    #[test]
    fn copy_preserves_symlink_by_default() {
        let root = test_root("copy-symlink-preserve");
        let target = root.join("target.txt");
        let source = root.join("link.txt");
        let destination = root.join("copied-link.txt");
        fs::write(&target, "linked").expect("write target");
        std::os::unix::fs::symlink("target.txt", &source).expect("create symlink");

        copy_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Preserve,
            }],
            |_| {},
            |_| Ok(()),
        )
        .expect("copy symlink");

        assert!(fs::symlink_metadata(&destination)
            .expect("read copied link metadata")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&destination).expect("read copied link"),
            PathBuf::from("target.txt")
        );

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn copy_can_follow_symlink_targets() {
        let root = test_root("copy-symlink-follow");
        let target = root.join("target.txt");
        let source = root.join("link.txt");
        let destination = root.join("copied-target.txt");
        fs::write(&target, "linked").expect("write target");
        std::os::unix::fs::symlink("target.txt", &source).expect("create symlink");

        copy_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Follow,
            }],
            |_| {},
            |_| Ok(()),
        )
        .expect("copy symlink target");

        assert!(!fs::symlink_metadata(&destination)
            .expect("read copied target metadata")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(&destination).expect("read copied target"),
            "linked"
        );

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn move_can_follow_symlink_targets_on_same_volume() {
        let root = test_root("move-symlink-follow");
        let target = root.join("target.txt");
        let source = root.join("link.txt");
        let destination = root.join("moved-target.txt");
        fs::write(&target, "linked").expect("write target");
        std::os::unix::fs::symlink("target.txt", &source).expect("create symlink");

        move_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Follow,
            }],
            |_| {},
            |_| Ok(()),
        )
        .expect("move symlink target");

        assert!(!source.exists());
        assert!(target.exists());
        assert!(!fs::symlink_metadata(&destination)
            .expect("read moved target metadata")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(&destination).expect("read moved target"),
            "linked"
        );

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn move_follow_symlink_cleans_temporary_copy_on_cancel() {
        let root = test_root("move-symlink-follow-cancel");
        let target = root.join("target.txt");
        let source = root.join("link.txt");
        let destination = root.join("moved-target.txt");
        fs::write(&target, "linked").expect("write target");
        std::os::unix::fs::symlink("target.txt", &source).expect("create symlink");
        let checkpoint_root = root.clone();

        let error = move_items_with_progress(
            &[LocalTransferItem {
                from: source.to_string_lossy().into_owned(),
                to: destination.to_string_lossy().into_owned(),
                overwrite: false,
                symlink_mode: SymlinkMode::Follow,
            }],
            |_| {},
            |_| {
                let has_temporary_move = fs::read_dir(&checkpoint_root)
                    .expect("read test root")
                    .filter_map(Result::ok)
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".carelo-move-")
                    });

                if has_temporary_move {
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
        .expect_err("move should stop when cancelled");

        assert_eq!(error.code, "operation_cancelled");
        assert!(fs::symlink_metadata(&source)
            .expect("source link should remain")
            .file_type()
            .is_symlink());
        assert!(!destination.exists());
        assert!(!fs::read_dir(&root)
            .expect("read test root")
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".carelo-move-")));

        cleanup(&root);
    }
}
