use super::*;

#[tauri::command]
pub async fn open_with_default_app(
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
        let materialized_path = materialize_remote_file(&remotes, remote_path).await?;
        return open_with::open_with_default(&materialized_path, remembered);
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
    path: String,
    app_id: String,
    remember: bool,
    store: tauri::State<'_, AppStoreState>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let file_type = open_with::file_type_for_path(Path::new(&path));
    let remembered = store.open_with_default(&file_type.key)?;
    let materialized_path = if let Some(archive_path) = archive::parse_archive_uri(&path) {
        run_local(move |_| archive::materialize_archive_file(&archive_path)).await?
    } else if let Some(remote_path) = parse_remote_path(&path) {
        materialize_remote_file(&remotes, remote_path).await?
    } else {
        PathBuf::from(&path)
    };
    let context = open_with::open_with_context(&materialized_path, remembered)?;
    let Some(app) = context.apps.iter().find(|app| app.id == app_id) else {
        return Err(FsError::new(
            "open_with_app_not_found",
            "The selected app is no longer available.",
            Some(path),
        ));
    };
    let app_name = app.name.clone();

    open_with::open_with_app_id(&materialized_path, &app_id)?;

    if remember {
        store.save_open_with_default(&file_type.key, &app_id, &app_name)?;
    } else {
        store.clear_open_with_default(&file_type.key)?;
    }

    Ok(())
}

#[tauri::command]
pub async fn run_custom_tool(
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

        for path in paths {
            if let Some(remote_path) = parse_remote_path(&path) {
                let target = unique_child_path_in(
                    &workspace,
                    &remote_leaf_name(&remote_path, "remote-item"),
                );
                copy_remote_to_local_item(&remotes, remote_path, &target, true).await?;
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

        return run_local(move |_| {
            run_local_custom_tool(&command, &staged_paths, staged_cwd.as_deref())
        })
        .await;
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
