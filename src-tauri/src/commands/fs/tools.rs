use super::*;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::{self, RecvTimeoutError};

#[tauri::command]
pub async fn open_with_default_app(
    app: AppHandle,
    path: String,
    store: tauri::State<'_, AppStoreState>,
    remotes: tauri::State<'_, RemoteVolumeState>,
    remote_edit_sync: tauri::State<'_, RemoteEditSyncState>,
) -> Result<(), FsError> {
    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        let file_type = open_with::file_type_for_path(Path::new(&path));
        let remembered = store.open_with_default(&file_type.key)?;
        let materialized_path =
            run_local(move |_| archive::materialize_archive_file(&archive_path)).await?;
        return open_with::open_with_default(&materialized_path, remembered);
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        let file_type = open_with::file_type_for_virtual_path(&remote_path.path);
        let remembered = store.open_with_default(&file_type.key)?;
        let materialized_path = materialize_remote_file(&remotes, remote_path.clone()).await?;
        open_with::open_with_default(&materialized_path, remembered)?;
        start_remote_edit_sync(
            app,
            &remote_edit_sync,
            vec![RemoteEditTarget {
                local_path: materialized_path,
                remote_path,
            }],
        );
        return Ok(());
    }

    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;
    open_with::open_with_default(&PathBuf::from(path), remembered)
}

#[tauri::command]
pub async fn list_open_with_apps(
    path: String,
    store: tauri::State<'_, AppStoreState>,
) -> Result<OpenWithContext, FsError> {
    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        let file_type = open_with::file_type_for_path(Path::new(&path));
        let remembered = store.open_with_default(&file_type.key)?;
        let materialized_path =
            run_local(move |_| archive::materialize_archive_file(&archive_path)).await?;
        return open_with::open_with_context(&materialized_path, remembered);
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        let file_type = open_with::file_type_for_virtual_path(&remote_path.path);
        let remembered = store.open_with_default(&file_type.key)?;
        return Ok(open_with::open_with_context_for_file_type(
            file_type, remembered,
        ));
    }

    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;
    open_with::open_with_context(&PathBuf::from(path), remembered)
}

#[tauri::command]
pub async fn open_with_app(
    app: AppHandle,
    path: String,
    app_id: String,
    remember: bool,
    store: tauri::State<'_, AppStoreState>,
    remotes: tauri::State<'_, RemoteVolumeState>,
    remote_edit_sync: tauri::State<'_, RemoteEditSyncState>,
) -> Result<(), FsError> {
    let remote_path_for_type = parse_remote_path(&path);
    let file_type = remote_path_for_type
        .as_ref()
        .map(|remote_path| open_with::file_type_for_virtual_path(&remote_path.path))
        .unwrap_or_else(|| open_with::file_type_for_path(Path::new(&path)));
    let remembered = store.open_with_default(&file_type.key)?;
    let mut remote_edit_target = None;
    let materialized_path = if let Some(archive_path) = archive::parse_archive_uri(&path) {
        run_local(move |_| archive::materialize_archive_file(&archive_path)).await?
    } else if let Some(remote_path) = remote_path_for_type {
        let materialized_path = materialize_remote_file(&remotes, remote_path.clone()).await?;
        remote_edit_target = Some(RemoteEditTarget {
            local_path: materialized_path.clone(),
            remote_path,
        });
        materialized_path
    } else {
        PathBuf::from(&path)
    };
    let context = if remote_edit_target.is_some() {
        open_with::open_with_context_for_file_type(file_type.clone(), remembered.clone())
    } else {
        open_with::open_with_context(&materialized_path, remembered)?
    };
    let Some(selected_app) = context.apps.iter().find(|app| app.id == app_id) else {
        return Err(FsError::new(
            "open_with_app_not_found",
            "The selected app is no longer available.",
            Some(path),
        ));
    };
    let app_name = selected_app.name.clone();

    open_with::open_with_app_id(&materialized_path, &app_id)?;

    if remember {
        store.save_open_with_default(&file_type.key, &app_id, &app_name)?;
    } else {
        store.clear_open_with_default(&file_type.key)?;
    }

    if let Some(target) = remote_edit_target {
        start_remote_edit_sync(app, &remote_edit_sync, vec![target]);
    }

    Ok(())
}

#[tauri::command]
pub async fn run_custom_tool(
    app: AppHandle,
    command: String,
    paths: Vec<String>,
    cwd: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
    remote_edit_sync: tauri::State<'_, RemoteEditSyncState>,
) -> Result<(), FsError> {
    if paths.iter().any(|path| parse_remote_path(path).is_some())
        || cwd.as_deref().and_then(parse_remote_path).is_some()
    {
        let workspace = create_persistent_temp_workspace("custom-tool")?;
        let mut staged_paths = Vec::with_capacity(paths.len());
        let mut remote_edit_targets = Vec::new();

        for path in paths {
            if let Some(remote_path) = parse_remote_path(&path) {
                let target = unique_child_path_in(
                    &workspace,
                    &remote_leaf_name(&remote_path, "remote-item"),
                );
                copy_remote_to_local_item(&remotes, remote_path.clone(), &target, true).await?;
                if target.is_file() {
                    remote_edit_targets.push(RemoteEditTarget {
                        local_path: target.clone(),
                        remote_path,
                    });
                }
                staged_paths.push(target.to_string_lossy().into_owned());
            } else {
                staged_paths.push(path);
            }
        }

        let staged_cwd = if let Some(cwd) = cwd {
            if let Some(remote_path) = parse_remote_path(&cwd) {
                let target =
                    unique_child_path_in(&workspace, &remote_leaf_name(&remote_path, "cwd"));
                copy_remote_to_local_item(&remotes, remote_path, &target, true).await?;
                Some(target.to_string_lossy().into_owned())
            } else {
                Some(cwd)
            }
        } else {
            None
        };

        let result = run_local(move |_| {
            run_local_custom_tool(&command, &staged_paths, staged_cwd.as_deref())
        })
        .await;

        if result.is_ok() && !remote_edit_targets.is_empty() {
            start_remote_edit_sync(app, &remote_edit_sync, remote_edit_targets);
        }

        return result;
    }

    run_local(move |_| run_local_custom_tool(&command, &paths, cwd.as_deref())).await
}

#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> Result<(), FsError> {
    let path = archive::parse_archive_uri(&path)
        .map(|archive_path| archive_path.archive_path)
        .unwrap_or_else(|| PathBuf::from(path));

    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|error| {
        FsError::new(
            "reveal_failed",
            format!("Unable to reveal item in the file manager: {error}"),
            Some(path.to_string_lossy().into_owned()),
        )
    })
}

const REMOTE_EDIT_SYNC_SETTLE: Duration = Duration::from_secs(2);
const REMOTE_EDIT_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const REMOTE_EDIT_SESSION_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);
const REMOTE_EDIT_WAKE_INTERVAL: Duration = Duration::from_millis(500);
const REMOTE_EDIT_FALLBACK_SCAN_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct RemoteEditSyncState {
    sender: Mutex<Option<mpsc::Sender<RemoteEditCommand>>>,
}

#[derive(Clone)]
struct RemoteEditTarget {
    local_path: PathBuf,
    remote_path: RemotePath,
}

#[derive(Clone)]
struct RemoteEditWatch {
    target: RemoteEditTarget,
    last_seen: Option<LocalFileSignature>,
    last_synced: Option<LocalFileSignature>,
    last_failure: Option<LocalFileSignature>,
    last_sync_attempt: Option<Instant>,
    next_sync_attempt_at: Option<Instant>,
    dirty_since: Option<Instant>,
    expires_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalFileSignature {
    len: u64,
    modified_secs: u64,
    modified_nanos: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteEditSyncEvent {
    path: String,
    local_path: String,
    message: Option<String>,
}

enum RemoteEditCommand {
    Watch(Vec<RemoteEditTarget>),
    LocalEvent(Vec<PathBuf>),
    ScanAll,
}

impl RemoteEditSyncState {
    fn watch(&self, app: AppHandle, mut targets: Vec<RemoteEditTarget>) {
        if targets.is_empty() {
            return;
        }

        for _ in 0..2 {
            let Ok(sender) = self.sender(app.clone()) else {
                return;
            };

            match sender.send(RemoteEditCommand::Watch(targets)) {
                Ok(()) => return,
                Err(error) => {
                    let RemoteEditCommand::Watch(returned_targets) = error.0 else {
                        return;
                    };
                    targets = returned_targets;
                }
            }

            if let Ok(mut stored_sender) = self.sender.lock() {
                *stored_sender = None;
            }
        }

        if let Ok(mut stored_sender) = self.sender.lock() {
            *stored_sender = None;
        }
    }

    fn sender(&self, app: AppHandle) -> FsResult<mpsc::Sender<RemoteEditCommand>> {
        let mut stored_sender = self.sender.lock().map_err(|error| {
            FsError::new(
                "remote_edit_sync_lock_failed",
                format!("Remote edit sync is unavailable: {error}"),
                None,
            )
        })?;

        if let Some(sender) = stored_sender.as_ref() {
            return Ok(sender.clone());
        }

        let (sender, receiver) = mpsc::channel();
        let worker_sender = sender.clone();
        thread::spawn(move || run_remote_edit_sync_worker(app, receiver, worker_sender));
        *stored_sender = Some(sender.clone());
        Ok(sender)
    }
}

fn start_remote_edit_sync(
    app: AppHandle,
    state: &RemoteEditSyncState,
    targets: Vec<RemoteEditTarget>,
) {
    state.watch(app, targets);
}

fn run_remote_edit_sync_worker(
    app: AppHandle,
    receiver: mpsc::Receiver<RemoteEditCommand>,
    sender: mpsc::Sender<RemoteEditCommand>,
) {
    let watcher_sender = sender.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        if let Ok(event) = result {
            if is_remote_edit_notify_event(&event) {
                let paths = if event.paths.is_empty() {
                    Vec::new()
                } else {
                    event.paths
                };
                let command = if paths.is_empty() {
                    RemoteEditCommand::ScanAll
                } else {
                    RemoteEditCommand::LocalEvent(paths)
                };
                let _ = watcher_sender.send(command);
            }
        }
    })
    .ok();
    let mut watches = HashMap::<PathBuf, RemoteEditWatch>::new();
    let mut watched_directories = HashSet::<PathBuf>::new();
    let mut next_fallback_scan = Instant::now() + REMOTE_EDIT_FALLBACK_SCAN_INTERVAL;

    loop {
        let command = if watches.is_empty() {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        } else {
            match receiver.recv_timeout(REMOTE_EDIT_WAKE_INTERVAL) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };

        if let Some(command) = command {
            handle_remote_edit_command(
                command,
                &mut watches,
                watcher.as_mut(),
                &mut watched_directories,
            );
        }

        let now = Instant::now();

        if now >= next_fallback_scan {
            mark_changed_remote_edit_watches_dirty(&mut watches, now);
            next_fallback_scan = now + REMOTE_EDIT_FALLBACK_SCAN_INTERVAL;
        }

        prune_remote_edit_watches(
            &mut watches,
            watcher.as_mut(),
            &mut watched_directories,
            now,
        );

        for watch in watches.values_mut() {
            let Some(dirty_since) = watch.dirty_since else {
                continue;
            };

            if let Some(next_sync_attempt_at) = watch.next_sync_attempt_at {
                if now < next_sync_attempt_at {
                    continue;
                }
            }

            if now.duration_since(dirty_since) < REMOTE_EDIT_SYNC_SETTLE {
                continue;
            }

            tauri::async_runtime::block_on(sync_remote_edit_watch_if_needed(&app, watch, now));
        }
    }
}

fn handle_remote_edit_command(
    command: RemoteEditCommand,
    watches: &mut HashMap<PathBuf, RemoteEditWatch>,
    watcher: Option<&mut RecommendedWatcher>,
    watched_directories: &mut HashSet<PathBuf>,
) {
    match command {
        RemoteEditCommand::Watch(targets) => {
            add_remote_edit_watches(targets, watches, watcher, watched_directories);
        }
        RemoteEditCommand::LocalEvent(paths) => {
            mark_remote_edit_paths_dirty(&paths, watches);
        }
        RemoteEditCommand::ScanAll => {
            mark_all_remote_edit_watches_dirty(watches);
        }
    }
}

fn add_remote_edit_watches(
    targets: Vec<RemoteEditTarget>,
    watches: &mut HashMap<PathBuf, RemoteEditWatch>,
    watcher: Option<&mut RecommendedWatcher>,
    watched_directories: &mut HashSet<PathBuf>,
) {
    let now = Instant::now();
    let mut watcher = watcher;

    for target in targets {
        let Some(signature) = local_file_signature(&target.local_path) else {
            continue;
        };

        if let Some(parent) = target.local_path.parent().map(Path::to_path_buf) {
            if watched_directories.insert(parent.clone()) {
                if let Some(watcher) = watcher.as_deref_mut() {
                    let _ = watcher.watch(&parent, RecursiveMode::NonRecursive);
                }
            }
        }

        watches.insert(
            target.local_path.clone(),
            RemoteEditWatch {
                target,
                last_seen: Some(signature),
                last_synced: Some(signature),
                last_failure: None,
                last_sync_attempt: None,
                next_sync_attempt_at: None,
                dirty_since: None,
                expires_at: now + REMOTE_EDIT_SESSION_TIMEOUT,
            },
        );
    }
}

fn prune_remote_edit_watches(
    watches: &mut HashMap<PathBuf, RemoteEditWatch>,
    watcher: Option<&mut RecommendedWatcher>,
    watched_directories: &mut HashSet<PathBuf>,
    now: Instant,
) {
    watches.retain(|_, watch| watch.expires_at > now);

    let needed_directories = watches
        .keys()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<HashSet<_>>();

    if let Some(watcher) = watcher {
        for directory in watched_directories
            .difference(&needed_directories)
            .cloned()
            .collect::<Vec<_>>()
        {
            let _ = watcher.unwatch(&directory);
        }
    }

    *watched_directories = needed_directories;
}

fn mark_remote_edit_paths_dirty(
    paths: &[PathBuf],
    watches: &mut HashMap<PathBuf, RemoteEditWatch>,
) {
    let now = Instant::now();

    for path in paths {
        if let Some(watch) = watches.get_mut(path) {
            mark_remote_edit_watch_dirty(watch, now);
            continue;
        }

        for (local_path, watch) in watches.iter_mut() {
            if path == local_path || path.starts_with(local_path) || local_path.starts_with(path) {
                mark_remote_edit_watch_dirty(watch, now);
            }
        }
    }
}

fn mark_all_remote_edit_watches_dirty(watches: &mut HashMap<PathBuf, RemoteEditWatch>) {
    let now = Instant::now();

    for watch in watches.values_mut() {
        mark_remote_edit_watch_dirty(watch, now);
    }
}

fn mark_changed_remote_edit_watches_dirty(
    watches: &mut HashMap<PathBuf, RemoteEditWatch>,
    now: Instant,
) {
    for watch in watches.values_mut() {
        let Some(signature) = local_file_signature(&watch.target.local_path) else {
            watch.last_seen = None;
            continue;
        };

        if Some(signature) != watch.last_seen {
            watch.last_seen = Some(signature);
            mark_remote_edit_watch_dirty(watch, now);
        }
    }
}

fn mark_remote_edit_watch_dirty(watch: &mut RemoteEditWatch, now: Instant) {
    watch.dirty_since = Some(now);
    watch.next_sync_attempt_at = None;
}

fn is_remote_edit_notify_event(event: &Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
}

async fn sync_remote_edit_watch_if_needed(
    app: &AppHandle,
    watch: &mut RemoteEditWatch,
    now: Instant,
) {
    let Some(signature) = local_file_signature(&watch.target.local_path) else {
        watch.last_seen = None;
        watch.dirty_since = None;
        watch.next_sync_attempt_at = None;
        return;
    };

    watch.last_seen = Some(signature);

    if Some(signature) == watch.last_synced {
        watch.dirty_since = None;
        watch.next_sync_attempt_at = None;
        return;
    }

    if Some(signature) == watch.last_failure {
        if let Some(last_attempt) = watch.last_sync_attempt {
            if now.duration_since(last_attempt) < REMOTE_EDIT_RETRY_INTERVAL {
                watch.next_sync_attempt_at = Some(last_attempt + REMOTE_EDIT_RETRY_INTERVAL);
                return;
            }
        }
    }

    watch.last_sync_attempt = Some(now);
    watch.next_sync_attempt_at = None;
    let remotes = app.state::<RemoteVolumeState>();
    let path = remote_edit_path(&watch.target);

    match copy_local_to_remote_item(
        &remotes,
        &watch.target.local_path,
        watch.target.remote_path.clone(),
        true,
        operations::SymlinkMode::Preserve,
    )
    .await
    {
        Ok(()) => {
            watch.last_synced = Some(signature);
            watch.last_failure = None;
            watch.dirty_since = None;
            watch.next_sync_attempt_at = None;
            let _ = app.emit(
                "remote-edit-synced",
                RemoteEditSyncEvent {
                    path,
                    local_path: watch.target.local_path.to_string_lossy().into_owned(),
                    message: None,
                },
            );
        }
        Err(error) => {
            watch.last_failure = Some(signature);
            watch.dirty_since = Some(now);
            watch.next_sync_attempt_at = Some(now + REMOTE_EDIT_RETRY_INTERVAL);
            let _ = app.emit(
                "remote-edit-sync-failed",
                RemoteEditSyncEvent {
                    path,
                    local_path: watch.target.local_path.to_string_lossy().into_owned(),
                    message: Some(error.message.clone()),
                },
            );
            eprintln!("Unable to sync remote edit: {}", error.message);
        }
    }
}

fn local_file_signature(path: &Path) -> Option<LocalFileSignature> {
    let metadata = fs::metadata(path).ok()?;

    if !metadata.is_file() {
        return None;
    }

    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;

    Some(LocalFileSignature {
        len: metadata.len(),
        modified_secs: modified.as_secs(),
        modified_nanos: modified.subsec_nanos(),
    })
}

fn remote_edit_path(target: &RemoteEditTarget) -> String {
    format_remote_uri(&target.remote_path.volume_id, &target.remote_path.path)
}

fn run_local_custom_tool(command: &str, paths: &[String], cwd: Option<&str>) -> FsResult<()> {
    let command = command.trim();

    if command.is_empty() {
        return Err(FsError::new(
            "custom_tool_empty_command",
            "Custom tool command is empty.",
            None,
        ));
    }

    if paths.is_empty() {
        return Err(FsError::new(
            "custom_tool_no_paths",
            "Choose at least one local item before running a custom tool.",
            None,
        ));
    }

    let local_paths = paths
        .iter()
        .map(|path| expand_custom_tool_path(path))
        .collect::<FsResult<Vec<_>>>()?;
    let path_values = local_paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let first_path = local_paths.first().expect("paths checked above");
    let first_path_value = path_values.first().cloned().unwrap_or_default();
    let first_name = first_path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();
    let parent_path = custom_tool_cwd(cwd, first_path)?;
    let parent_value = parent_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tokens = split_custom_tool_command(command)?;
    let mut expanded = expand_custom_tool_tokens(
        &tokens,
        &path_values,
        &first_path_value,
        &first_name,
        &parent_value,
    );

    if !tokens_have_custom_tool_placeholders(&tokens) {
        expanded.extend(path_values.iter().cloned());
    }

    let Some(program) = expanded
        .first()
        .cloned()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(FsError::new(
            "custom_tool_empty_command",
            "Custom tool command is empty.",
            Some(first_path_value),
        ));
    };
    let args = expanded.drain(1..).collect::<Vec<_>>();
    let mut child = Command::new(&program);
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(parent_path) = parent_path.filter(|path| path.is_dir()) {
        child.current_dir(parent_path);
    }

    child.spawn().map_err(|error| {
        FsError::new(
            "custom_tool_spawn_failed",
            format!("Unable to run custom tool: {error}"),
            Some(first_path_value),
        )
    })?;

    Ok(())
}

fn expand_custom_tool_path(path: &str) -> FsResult<PathBuf> {
    if archive::is_archive_uri(path) || parse_remote_path(path).is_some() {
        return Err(FsError::new(
            "custom_tool_unsupported_path",
            "Custom tools can run on local files and folders only.",
            Some(path.to_string()),
        ));
    }

    expand_local_path(path)
}

fn custom_tool_cwd(cwd: Option<&str>, first_path: &Path) -> FsResult<Option<PathBuf>> {
    if let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) {
        if archive::is_archive_uri(cwd) || parse_remote_path(cwd).is_some() {
            return Err(FsError::new(
                "custom_tool_unsupported_path",
                "Custom tools can run from local folders only.",
                Some(cwd.to_string()),
            ));
        }

        return expand_local_path(cwd).map(Some);
    }

    Ok(first_path.parent().map(Path::to_path_buf))
}

fn split_custom_tool_command(command: &str) -> FsResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut token_started = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(quote_char) = quote {
            if ch == quote_char {
                quote = None;
            } else if ch == '\\' && quote_char == '"' {
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    current.push(ch);
                }
            } else {
                current.push(ch);
            }
            token_started = true;
            continue;
        }

        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                token_started = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    current.push(ch);
                }
                token_started = true;
            }
            ch if ch.is_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            ch => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if let Some(quote_char) = quote {
        return Err(FsError::new(
            "custom_tool_invalid_command",
            format!("Custom tool command has an unclosed {quote_char} quote."),
            None,
        ));
    }

    if token_started {
        tokens.push(current);
    }

    if tokens.is_empty() {
        return Err(FsError::new(
            "custom_tool_empty_command",
            "Custom tool command is empty.",
            None,
        ));
    }

    Ok(tokens)
}

fn tokens_have_custom_tool_placeholders(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        token.contains("%path%")
            || token.contains("%paths%")
            || token.contains("%name%")
            || token.contains("%parent%")
    })
}

fn expand_custom_tool_tokens(
    tokens: &[String],
    paths: &[String],
    first_path: &str,
    first_name: &str,
    parent_path: &str,
) -> Vec<String> {
    let joined_paths = paths.join(" ");
    let mut expanded = Vec::new();

    for token in tokens {
        if token == "%paths%" {
            expanded.extend(paths.iter().cloned());
            continue;
        }

        expanded.push(
            token
                .replace("%path%", first_path)
                .replace("%paths%", &joined_paths)
                .replace("%name%", first_name)
                .replace("%parent%", parent_path),
        );
    }

    expanded
}
