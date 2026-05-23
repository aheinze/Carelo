use super::*;

#[tauri::command]
pub async fn archive_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    destination: String,
    options: Option<archive::ArchiveOptions>,
    overwrite: bool,
    job_id: Option<String>,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<(), FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let has_remote_source = paths.iter().any(|path| parse_remote_path(path).is_some());
    let remote_destination = parse_remote_path(&destination);

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }
    }

    if archive::is_archive_uri(&destination) {
        return Err(archive_read_only_error(&destination));
    }

    if has_remote_source || remote_destination.is_some() {
        return archive_items_with_remote_support(
            app,
            operation_state.inner().clone(),
            remotes,
            paths,
            destination,
            options.unwrap_or_default(),
            overwrite,
            job_id,
            remote_destination,
        )
        .await;
    }

    let sudo_paths = paths.clone();
    let sudo_destination = destination.clone();
    let options = options.unwrap_or_default();
    let sudo_options = options.clone();
    let operation_state = operation_state.inner().clone();
    let cleanup_operation_state = operation_state.clone();
    let native_app = app.clone();
    let native_job_id = job_id.clone();
    let native_operation_state = operation_state.clone();
    let sudo_app = app.clone();
    let sudo_job_id = job_id.clone();
    let sudo_operation_state = operation_state.clone();
    let result = run_local_with_sudo(
        sudo_password,
        move |_| {
            archive::archive_items_with_progress(
                &paths,
                &destination,
                overwrite,
                &options,
                |progress| {
                    emit_file_operation_progress(
                        &native_app,
                        &native_job_id,
                        "archive",
                        "running",
                        progress,
                    );
                },
                || native_operation_state.wait_if_paused_or_cancelled(&native_job_id),
            )
        },
        move |password| {
            sudo_operation_state.checkpoint(&sudo_job_id, None)?;
            emit_file_operation_status(&sudo_app, &sudo_job_id, "archive", "running");
            sudo::archive_items(
                &password,
                &sudo_paths,
                &sudo_destination,
                overwrite,
                &sudo_options,
            )
        },
    )
    .await;

    if let Some(job_id) = &job_id {
        cleanup_operation_state.clear_cancel(job_id);
        cleanup_operation_state.clear_pause(job_id);
    }

    result
}

#[tauri::command]
pub async fn unarchive_items(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    destination_directory: String,
    job_id: Option<String>,
    sudo_password: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<String>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());
    let has_remote_source = paths.iter().any(|path| parse_remote_path(path).is_some());
    let remote_destination = parse_remote_path(&destination_directory);

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }
    }

    if archive::is_archive_uri(&destination_directory) {
        return Err(archive_read_only_error(&destination_directory));
    }

    if has_remote_source || remote_destination.is_some() {
        return unarchive_items_with_remote_support(
            app,
            operation_state.inner().clone(),
            remotes,
            paths,
            destination_directory,
            job_id,
            remote_destination,
        )
        .await;
    }

    let sudo_paths = paths.clone();
    let sudo_destination_directory = destination_directory.clone();
    let operation_state = operation_state.inner().clone();
    let cleanup_operation_state = operation_state.clone();
    let native_app = app.clone();
    let native_job_id = job_id.clone();
    let native_operation_state = operation_state.clone();
    let sudo_app = app.clone();
    let sudo_job_id = job_id.clone();
    let sudo_operation_state = operation_state.clone();
    let result = run_local_with_sudo(
        sudo_password,
        move |_| {
            archive::unarchive_items_with_progress(
                &paths,
                &destination_directory,
                |progress| {
                    emit_file_operation_progress(
                        &native_app,
                        &native_job_id,
                        "unarchive",
                        "running",
                        progress,
                    );
                },
                || native_operation_state.wait_if_paused_or_cancelled(&native_job_id),
            )
        },
        move |password| {
            sudo_operation_state.checkpoint(&sudo_job_id, None)?;
            emit_file_operation_status(&sudo_app, &sudo_job_id, "unarchive", "running");
            sudo::unarchive_items(&password, &sudo_paths, &sudo_destination_directory)
        },
    )
    .await;

    if let Some(job_id) = &job_id {
        cleanup_operation_state.clear_cancel(job_id);
        cleanup_operation_state.clear_pause(job_id);
    }

    result
}

async fn archive_items_with_remote_support(
    app: AppHandle,
    operation_state: FileOperationState,
    remotes: tauri::State<'_, RemoteVolumeState>,
    paths: Vec<String>,
    destination: String,
    options: archive::ArchiveOptions,
    overwrite: bool,
    job_id: Option<String>,
    remote_destination: Option<RemotePath>,
) -> Result<(), FsError> {
    let workspace = TemporaryWorkspace::new("archive")?;
    let staged_paths = materialize_remote_sources(&remotes, &paths, &workspace).await?;
    let local_destination = if let Some(remote_destination) = remote_destination.as_ref() {
        workspace
            .unique_child_path(&remote_leaf_name(remote_destination, "archive"))
            .to_string_lossy()
            .into_owned()
    } else {
        destination.clone()
    };
    let local_destination_path = PathBuf::from(&local_destination);
    let archive_app = app.clone();
    let archive_job_id = job_id.clone();
    let archive_operation_state = operation_state.clone();

    run_local(move |_| {
        archive::archive_items_with_progress(
            &staged_paths,
            &local_destination,
            overwrite,
            &options,
            |progress| {
                emit_file_operation_progress(
                    &archive_app,
                    &archive_job_id,
                    "archive",
                    "running",
                    progress,
                );
            },
            || archive_operation_state.wait_if_paused_or_cancelled(&archive_job_id),
        )
    })
    .await?;

    if let Some(remote_destination) = remote_destination {
        copy_local_to_remote_item(
            &remotes,
            &local_destination_path,
            remote_destination,
            overwrite,
            operations::SymlinkMode::Preserve,
        )
        .await?;
    }

    Ok(())
}

async fn unarchive_items_with_remote_support(
    app: AppHandle,
    operation_state: FileOperationState,
    remotes: tauri::State<'_, RemoteVolumeState>,
    paths: Vec<String>,
    destination_directory: String,
    job_id: Option<String>,
    remote_destination: Option<RemotePath>,
) -> Result<Vec<String>, FsError> {
    let workspace = TemporaryWorkspace::new("unarchive")?;
    let staged_paths = materialize_remote_sources(&remotes, &paths, &workspace).await?;
    let local_destination = if remote_destination.is_some() {
        workspace
            .path()
            .join("extracted")
            .to_string_lossy()
            .into_owned()
    } else {
        destination_directory.clone()
    };
    let unarchive_app = app.clone();
    let unarchive_job_id = job_id.clone();
    let unarchive_operation_state = operation_state.clone();

    let extracted_paths = run_local(move |_| {
        archive::unarchive_items_with_progress(
            &staged_paths,
            &local_destination,
            |progress| {
                emit_file_operation_progress(
                    &unarchive_app,
                    &unarchive_job_id,
                    "unarchive",
                    "running",
                    progress,
                );
            },
            || unarchive_operation_state.wait_if_paused_or_cancelled(&unarchive_job_id),
        )
    })
    .await?;

    let Some(remote_destination) = remote_destination else {
        return Ok(extracted_paths);
    };

    let mut remote_results = Vec::new();

    for extracted_path in extracted_paths {
        let extracted_path = PathBuf::from(extracted_path);
        let name = extracted_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("extracted");
        let target_path = join_remote_object_path(&remote_destination.path, name);
        let target = RemotePath {
            volume_id: remote_destination.volume_id.clone(),
            path: target_path,
        };
        copy_local_to_remote_item(
            &remotes,
            &extracted_path,
            target.clone(),
            false,
            operations::SymlinkMode::Preserve,
        )
        .await?;
        remote_results.push(crate::fs::remote::format_remote_uri(
            &target.volume_id,
            &target.path,
        ));
    }

    Ok(remote_results)
}
