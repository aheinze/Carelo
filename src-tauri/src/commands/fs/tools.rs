use super::*;

#[tauri::command]
pub async fn open_with_default_app(
    app: AppHandle,
    path: String,
    store: tauri::State<'_, AppStoreState>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;

    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        let materialized_path =
            run_local(move |_| archive::materialize_archive_file(&archive_path)).await?;
        return open_with::open_with_default(&materialized_path, remembered);
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        let materialized_path = materialize_remote_file(&remotes, remote_path.clone()).await?;
        open_with::open_with_default(&materialized_path, remembered)?;
        start_remote_edit_sync(
            app,
            vec![RemoteEditTarget {
                local_path: materialized_path,
                remote_path,
            }],
        );
        return Ok(());
    }

    open_with::open_with_default(&PathBuf::from(path), remembered)
}

#[tauri::command]
pub async fn list_open_with_apps(
    path: String,
    store: tauri::State<'_, AppStoreState>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<OpenWithContext, FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;

    if let Some(archive_path) = archive::parse_archive_uri(&path) {
        let materialized_path =
            run_local(move |_| archive::materialize_archive_file(&archive_path)).await?;
        return open_with::open_with_context(&materialized_path, remembered);
    }

    if let Some(remote_path) = parse_remote_path(&path) {
        let materialized_path = materialize_remote_file(&remotes, remote_path).await?;
        return open_with::open_with_context(&materialized_path, remembered);
    }

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
) -> Result<(), FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;
    let mut remote_edit_target = None;
    let materialized_path = if let Some(archive_path) = archive::parse_archive_uri(&path) {
        run_local(move |_| archive::materialize_archive_file(&archive_path)).await?
    } else if let Some(remote_path) = parse_remote_path(&path) {
        let materialized_path = materialize_remote_file(&remotes, remote_path.clone()).await?;
        remote_edit_target = Some(RemoteEditTarget {
            local_path: materialized_path.clone(),
            remote_path,
        });
        materialized_path
    } else {
        PathBuf::from(&path)
    };
    let context = open_with::open_with_context(&materialized_path, remembered)?;
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
        start_remote_edit_sync(app, vec![target]);
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
            start_remote_edit_sync(app, remote_edit_targets);
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

const REMOTE_EDIT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const REMOTE_EDIT_SYNC_SETTLE: Duration = Duration::from_secs(2);
const REMOTE_EDIT_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const REMOTE_EDIT_SESSION_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);

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
    stable_since: Instant,
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

fn start_remote_edit_sync(app: AppHandle, targets: Vec<RemoteEditTarget>) {
    let mut watches = targets
        .into_iter()
        .filter_map(|target| {
            let signature = local_file_signature(&target.local_path)?;
            Some(RemoteEditWatch {
                target,
                last_seen: Some(signature),
                last_synced: Some(signature),
                last_failure: None,
                last_sync_attempt: None,
                stable_since: Instant::now(),
            })
        })
        .collect::<Vec<_>>();

    if watches.is_empty() {
        return;
    }

    thread::spawn(move || {
        let deadline = Instant::now() + REMOTE_EDIT_SESSION_TIMEOUT;

        while Instant::now() < deadline {
            thread::sleep(REMOTE_EDIT_POLL_INTERVAL);
            let now = Instant::now();

            for watch in &mut watches {
                tauri::async_runtime::block_on(update_remote_edit_watch(&app, watch, now));
            }
        }
    });
}

async fn update_remote_edit_watch(app: &AppHandle, watch: &mut RemoteEditWatch, now: Instant) {
    let Some(signature) = local_file_signature(&watch.target.local_path) else {
        watch.last_seen = None;
        watch.stable_since = now;
        return;
    };

    if Some(signature) != watch.last_seen {
        watch.last_seen = Some(signature);
        watch.stable_since = now;
        return;
    }

    if Some(signature) == watch.last_synced
        || now.duration_since(watch.stable_since) < REMOTE_EDIT_SYNC_SETTLE
    {
        return;
    }

    let remotes = app.state::<RemoteVolumeState>();
    let path = remote_edit_path(&watch.target);

    if Some(signature) == watch.last_failure {
        if let Some(last_attempt) = watch.last_sync_attempt {
            if now.duration_since(last_attempt) < REMOTE_EDIT_RETRY_INTERVAL {
                return;
            }
        }
    }

    watch.last_sync_attempt = Some(now);

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
