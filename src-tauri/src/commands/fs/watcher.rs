use super::*;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::{self, RecvTimeoutError};

const DIRECTORY_WATCH_SETTLE: Duration = Duration::from_millis(220);

#[derive(Default)]
pub struct DirectoryWatchState {
    sender: Mutex<Option<mpsc::Sender<DirectoryWatchCommand>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryWatchChangedEvent {
    path: String,
}

enum DirectoryWatchCommand {
    Set(Vec<PathBuf>),
    Notify(Vec<PathBuf>),
}

#[tauri::command]
pub async fn watch_active_directories(
    app: AppHandle,
    paths: Vec<String>,
    state: tauri::State<'_, DirectoryWatchState>,
) -> Result<Vec<String>, FsError> {
    let watch_paths = tauri::async_runtime::spawn_blocking(move || normalize_watch_paths(paths))
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Directory watch setup failed: {error}"),
                None,
            )
        })??;
    let watched_paths = watch_paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    state.watch(app, watch_paths)?;
    Ok(watched_paths)
}

impl DirectoryWatchState {
    fn watch(&self, app: AppHandle, paths: Vec<PathBuf>) -> FsResult<()> {
        let mut command = DirectoryWatchCommand::Set(paths);

        for _ in 0..2 {
            let sender = self.sender(app.clone())?;

            match sender.send(command) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    command = error.0;
                }
            }

            if let Ok(mut stored_sender) = self.sender.lock() {
                *stored_sender = None;
            }
        }

        Err(FsError::new(
            "directory_watch_unavailable",
            "Directory watching is unavailable.",
            None,
        ))
    }

    fn sender(&self, app: AppHandle) -> FsResult<mpsc::Sender<DirectoryWatchCommand>> {
        let mut stored_sender = self.sender.lock().map_err(|error| {
            FsError::new(
                "directory_watch_lock_failed",
                format!("Directory watching is unavailable: {error}"),
                None,
            )
        })?;

        if let Some(sender) = stored_sender.as_ref() {
            return Ok(sender.clone());
        }

        let (sender, receiver) = mpsc::channel();
        let watcher_sender = sender.clone();
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if let Ok(event) = result {
                if is_directory_watch_event(&event) {
                    let _ = watcher_sender.send(DirectoryWatchCommand::Notify(event.paths));
                }
            }
        })
        .map_err(|error| {
            FsError::new(
                "directory_watch_unavailable",
                format!("Directory watching is unavailable: {error}"),
                None,
            )
        })?;

        thread::spawn(move || run_directory_watch_worker(app, receiver, watcher));
        *stored_sender = Some(sender.clone());
        Ok(sender)
    }
}

fn normalize_watch_paths(paths: Vec<String>) -> FsResult<Vec<PathBuf>> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();

    for path in paths {
        let trimmed = path.trim();

        if trimmed.is_empty()
            || trimmed.starts_with("remote://")
            || archive::is_archive_uri(trimmed)
        {
            continue;
        }

        let path = expand_local_path(trimmed)?;
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        if !metadata.is_dir() {
            continue;
        }

        if seen.insert(path.clone()) {
            normalized.push(path);
        }
    }

    Ok(normalized)
}

fn run_directory_watch_worker(
    app: AppHandle,
    receiver: mpsc::Receiver<DirectoryWatchCommand>,
    mut watcher: RecommendedWatcher,
) {
    let mut watched_directories = HashSet::<PathBuf>::new();
    let mut pending_directories = HashSet::<PathBuf>::new();
    let mut next_emit_at: Option<Instant> = None;

    loop {
        let command = if let Some(emit_at) = next_emit_at {
            let timeout = emit_at.saturating_duration_since(Instant::now());

            if timeout.is_zero() {
                None
            } else {
                match receiver.recv_timeout(timeout) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };

        if let Some(command) = command {
            match command {
                DirectoryWatchCommand::Set(paths) => {
                    set_watched_directories(&mut watcher, &mut watched_directories, paths);
                    pending_directories.retain(|path| watched_directories.contains(path));

                    if pending_directories.is_empty() {
                        next_emit_at = None;
                    }
                }
                DirectoryWatchCommand::Notify(paths) => {
                    for path in affected_watch_directories(&paths, &watched_directories) {
                        pending_directories.insert(path);
                    }

                    if !pending_directories.is_empty() {
                        next_emit_at = Some(Instant::now() + DIRECTORY_WATCH_SETTLE);
                    }
                }
            }
        }

        if next_emit_at
            .map(|emit_at| Instant::now() >= emit_at)
            .unwrap_or(false)
        {
            emit_pending_directories(&app, &mut pending_directories);
            next_emit_at = None;
        }
    }
}

fn set_watched_directories(
    watcher: &mut RecommendedWatcher,
    watched_directories: &mut HashSet<PathBuf>,
    paths: Vec<PathBuf>,
) {
    let next_directories = paths.into_iter().collect::<HashSet<_>>();

    for directory in watched_directories.difference(&next_directories) {
        let _ = watcher.unwatch(directory);
    }

    let mut active_directories = watched_directories
        .intersection(&next_directories)
        .cloned()
        .collect::<HashSet<_>>();

    for directory in next_directories.difference(watched_directories) {
        if watcher
            .watch(directory, RecursiveMode::NonRecursive)
            .is_ok()
        {
            active_directories.insert(directory.clone());
        }
    }

    *watched_directories = active_directories;
}

fn affected_watch_directories(
    paths: &[PathBuf],
    watched_directories: &HashSet<PathBuf>,
) -> Vec<PathBuf> {
    if paths.is_empty() {
        return watched_directories.iter().cloned().collect();
    }

    let mut affected = HashSet::new();

    for event_path in paths {
        for watched_directory in watched_directories {
            if event_path == watched_directory
                || event_path.parent() == Some(watched_directory.as_path())
                || event_path.starts_with(watched_directory)
            {
                affected.insert(watched_directory.clone());
            }
        }
    }

    affected.into_iter().collect()
}

fn emit_pending_directories(app: &AppHandle, pending_directories: &mut HashSet<PathBuf>) {
    let directories = std::mem::take(pending_directories);

    for directory in directories {
        let _ = app.emit(
            "directory-watch-changed",
            DirectoryWatchChangedEvent {
                path: directory.to_string_lossy().into_owned(),
            },
        );
    }
}

fn is_directory_watch_event(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}
