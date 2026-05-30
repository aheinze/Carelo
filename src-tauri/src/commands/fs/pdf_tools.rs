use super::*;

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PdfCompressProfile {
    Smallest,
    #[default]
    Balanced,
    Print,
    Prepress,
}

impl PdfCompressProfile {
    fn ghostscript_setting(self) -> &'static str {
        match self {
            Self::Smallest => "/screen",
            Self::Balanced => "/ebook",
            Self::Print => "/printer",
            Self::Prepress => "/prepress",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Smallest => "smallest",
            Self::Balanced => "balanced",
            Self::Print => "print",
            Self::Prepress => "prepress",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PdfCompressConflictPolicy {
    #[default]
    KeepBoth,
    Replace,
    Skip,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfCompressOptions {
    #[serde(default)]
    pub profile: PdfCompressProfile,
    #[serde(default)]
    pub conflict: PdfCompressConflictPolicy,
    #[serde(default)]
    pub keep_only_smaller: bool,
    #[serde(default)]
    pub destination_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfCompressResult {
    pub source_path: String,
    pub output_path: Option<String>,
    pub output_name: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub input_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
}

#[derive(Clone)]
enum PdfOutputParent {
    Local(PathBuf),
    Remote(RemotePath),
}

struct PdfCompressSource {
    original_path: String,
    local_path: PathBuf,
    name: String,
    source_parent: PdfOutputParent,
    input_bytes: Option<u64>,
}

enum PdfCompressOutcome {
    Compressed(CompressedPdf),
    Skipped {
        output_name: String,
        message: String,
        input_bytes: Option<u64>,
        output_bytes: Option<u64>,
    },
}

struct CompressedPdf {
    output_path: String,
    output_name: String,
    input_bytes: Option<u64>,
    output_bytes: u64,
}

#[tauri::command]
pub async fn compress_pdfs(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    options: PdfCompressOptions,
    job_id: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<PdfCompressResult>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if paths.is_empty() {
        return Err(FsError::new(
            "pdf_compress_no_paths",
            "Choose at least one PDF to compress.",
            None,
        ));
    }

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }
    }

    let ghostscript = ghostscript_program()?;
    let workspace = TemporaryWorkspace::new("pdf-compress")?;
    let destination_parent = match options.destination_directory.as_deref() {
        Some(destination) => Some(resolve_pdf_output_parent(destination)?),
        None => None,
    };
    let mut results = Vec::with_capacity(paths.len());
    let mut planned_names = HashMap::<String, HashSet<String>>::new();
    let mut remote_name_cache = HashMap::<String, HashSet<String>>::new();
    let total_entries = paths.len() as u64;
    let mut processed_bytes = 0_u64;

    for (index, path) in paths.into_iter().enumerate() {
        operation_state.checkpoint(&job_id, None)?;
        emit_file_operation_progress(
            &app,
            &job_id,
            "pdf-compress",
            "running",
            ProgressSnapshot {
                processed_bytes,
                processed_entries: index as u64,
                total_entries,
                current_path: Some(path.clone()),
                ..ProgressSnapshot::default()
            },
        );

        let source = materialize_pdf_compress_source(
            &remotes,
            &workspace,
            &path,
            destination_parent.clone(),
        )
        .await?;
        let output_parent = destination_parent
            .clone()
            .unwrap_or_else(|| source.source_parent.clone());
        let seed_name = pdf_output_name(&source.name);
        let destination = resolve_pdf_destination(
            &remotes,
            output_parent,
            &seed_name,
            options.conflict,
            &mut planned_names,
            &mut remote_name_cache,
        )
        .await?;

        let Some(destination) = destination else {
            results.push(PdfCompressResult {
                source_path: source.original_path,
                output_path: None,
                output_name: Some(seed_name),
                status: "skipped".to_string(),
                message: Some("Output already exists.".to_string()),
                input_bytes: source.input_bytes,
                output_bytes: None,
            });
            emit_file_operation_progress(
                &app,
                &job_id,
                "pdf-compress",
                "running",
                ProgressSnapshot {
                    processed_bytes,
                    processed_entries: index as u64 + 1,
                    total_entries,
                    ..ProgressSnapshot::default()
                },
            );
            continue;
        };

        let outcome = compress_pdf_to_destination(
            &remotes,
            &workspace,
            &operation_state,
            &job_id,
            &ghostscript,
            &source.local_path,
            source.input_bytes,
            destination,
            options.profile,
            options.keep_only_smaller,
        )
        .await?;

        if let Some(bytes) = source.input_bytes {
            processed_bytes = processed_bytes.saturating_add(bytes);
        }

        match outcome {
            PdfCompressOutcome::Compressed(compressed) => {
                results.push(PdfCompressResult {
                    source_path: source.original_path,
                    output_path: Some(compressed.output_path),
                    output_name: Some(compressed.output_name),
                    status: "compressed".to_string(),
                    message: Some(format!(
                        "Compressed with {} profile",
                        options.profile.label()
                    )),
                    input_bytes: compressed.input_bytes,
                    output_bytes: Some(compressed.output_bytes),
                });
            }
            PdfCompressOutcome::Skipped {
                output_name,
                message,
                input_bytes,
                output_bytes,
            } => {
                results.push(PdfCompressResult {
                    source_path: source.original_path,
                    output_path: None,
                    output_name: Some(output_name),
                    status: "skipped".to_string(),
                    message: Some(message),
                    input_bytes,
                    output_bytes,
                });
            }
        }

        emit_file_operation_progress(
            &app,
            &job_id,
            "pdf-compress",
            "running",
            ProgressSnapshot {
                processed_bytes,
                processed_entries: index as u64 + 1,
                total_entries,
                ..ProgressSnapshot::default()
            },
        );
    }

    Ok(results)
}

fn resolve_pdf_output_parent(path: &str) -> FsResult<PdfOutputParent> {
    if archive::is_archive_uri(path) {
        return Err(archive_read_only_error(path));
    }

    if let Some(remote_path) = parse_remote_path(path) {
        return Ok(PdfOutputParent::Remote(remote_path));
    }

    Ok(PdfOutputParent::Local(expand_local_path(path)?))
}

async fn materialize_pdf_compress_source(
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    path: &str,
    destination_parent: Option<PdfOutputParent>,
) -> FsResult<PdfCompressSource> {
    if let Some(remote_path) = parse_remote_path(path) {
        let name = remote_leaf_name(&remote_path, "document.pdf");

        if !is_pdf_name(&name) {
            return Err(FsError::new(
                "pdf_compress_not_pdf",
                "Only PDF files can be compressed.",
                Some(path.to_string()),
            ));
        }

        let local_path = workspace.unique_child_path(&name);
        copy_remote_to_local_item(remotes, remote_path.clone(), &local_path, true).await?;
        let metadata = fs::metadata(&local_path)
            .map_err(|error| FsError::io("Unable to read PDF metadata", &local_path, error))?;

        if !metadata.is_file() {
            return Err(FsError::new(
                "pdf_compress_not_file",
                "Only PDF files can be compressed.",
                Some(path.to_string()),
            ));
        }

        return Ok(PdfCompressSource {
            original_path: path.to_string(),
            local_path,
            name,
            source_parent: destination_parent
                .unwrap_or_else(|| PdfOutputParent::Remote(remote_parent_path(&remote_path))),
            input_bytes: Some(metadata.len()),
        });
    }

    let local_path = expand_local_path(path)?;
    let metadata = fs::metadata(&local_path)
        .map_err(|error| FsError::io("Unable to read PDF metadata", &local_path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "pdf_compress_not_file",
            "Only PDF files can be compressed.",
            Some(path.to_string()),
        ));
    }

    let name = local_path
        .file_name()
        .unwrap_or_else(|| OsStr::new("document.pdf"))
        .to_string_lossy()
        .into_owned();

    if !is_pdf_name(&name) {
        return Err(FsError::new(
            "pdf_compress_not_pdf",
            "Only PDF files can be compressed.",
            Some(path.to_string()),
        ));
    }

    let source_parent = destination_parent.unwrap_or_else(|| {
        PdfOutputParent::Local(
            local_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        )
    });

    Ok(PdfCompressSource {
        original_path: path.to_string(),
        local_path,
        name,
        source_parent,
        input_bytes: Some(metadata.len()),
    })
}

fn remote_parent_path(path: &RemotePath) -> RemotePath {
    let trimmed = path.path.trim_matches('/');
    let parent = trimmed
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
        .to_string();

    RemotePath {
        volume_id: path.volume_id.clone(),
        path: parent,
    }
}

async fn resolve_pdf_destination(
    remotes: &RemoteVolumeState,
    parent: PdfOutputParent,
    seed_name: &str,
    conflict: PdfCompressConflictPolicy,
    planned_names: &mut HashMap<String, HashSet<String>>,
    remote_name_cache: &mut HashMap<String, HashSet<String>>,
) -> FsResult<Option<PdfDestination>> {
    match parent {
        PdfOutputParent::Local(parent) => {
            resolve_local_pdf_destination(parent, seed_name, conflict, planned_names)
        }
        PdfOutputParent::Remote(parent) => {
            resolve_remote_pdf_destination(
                remotes,
                parent,
                seed_name,
                conflict,
                planned_names,
                remote_name_cache,
            )
            .await
        }
    }
}

enum PdfDestination {
    Local {
        path: PathBuf,
        name: String,
        overwrite: bool,
    },
    Remote {
        path: RemotePath,
        name: String,
        overwrite: bool,
    },
}

fn resolve_local_pdf_destination(
    parent: PathBuf,
    seed_name: &str,
    conflict: PdfCompressConflictPolicy,
    planned_names: &mut HashMap<String, HashSet<String>>,
) -> FsResult<Option<PdfDestination>> {
    let key = format!("local:{}", parent.to_string_lossy());
    let planned = planned_names.entry(key).or_default();
    let mut name = seed_name.to_string();
    let planned_exists = planned.contains(&name.to_lowercase());
    let exists = parent.join(&name).exists();

    if planned_exists {
        name = unique_pdf_output_name(seed_name, |candidate| {
            parent.join(candidate).exists() || planned.contains(&candidate.to_lowercase())
        });
    } else if exists {
        match conflict {
            PdfCompressConflictPolicy::Skip => return Ok(None),
            PdfCompressConflictPolicy::Replace => {}
            PdfCompressConflictPolicy::KeepBoth => {
                name = unique_pdf_output_name(seed_name, |candidate| {
                    parent.join(candidate).exists() || planned.contains(&candidate.to_lowercase())
                });
            }
        }
    }

    planned.insert(name.to_lowercase());
    Ok(Some(PdfDestination::Local {
        path: parent.join(&name),
        name,
        overwrite: conflict == PdfCompressConflictPolicy::Replace,
    }))
}

async fn resolve_remote_pdf_destination(
    remotes: &RemoteVolumeState,
    parent: RemotePath,
    seed_name: &str,
    conflict: PdfCompressConflictPolicy,
    planned_names: &mut HashMap<String, HashSet<String>>,
    remote_name_cache: &mut HashMap<String, HashSet<String>>,
) -> FsResult<Option<PdfDestination>> {
    let key = format_remote_uri(&parent.volume_id, &parent.path);
    let existing_names = if let Some(names) = remote_name_cache.get(&key) {
        names.clone()
    } else {
        let names = list_remote_directory(remotes, parent.clone())
            .await?
            .into_iter()
            .map(|entry| entry.name.to_lowercase())
            .collect::<HashSet<_>>();
        remote_name_cache.insert(key.clone(), names.clone());
        names
    };
    let planned = planned_names.entry(format!("remote:{key}")).or_default();
    let mut name = seed_name.to_string();
    let planned_exists = planned.contains(&name.to_lowercase());
    let exists = existing_names.contains(&name.to_lowercase());

    if planned_exists {
        name = unique_pdf_output_name(seed_name, |candidate| {
            let key = candidate.to_lowercase();
            existing_names.contains(&key) || planned.contains(&key)
        });
    } else if exists {
        match conflict {
            PdfCompressConflictPolicy::Skip => return Ok(None),
            PdfCompressConflictPolicy::Replace => {}
            PdfCompressConflictPolicy::KeepBoth => {
                name = unique_pdf_output_name(seed_name, |candidate| {
                    let key = candidate.to_lowercase();
                    existing_names.contains(&key) || planned.contains(&key)
                });
            }
        }
    }

    planned.insert(name.to_lowercase());
    let object_path = join_remote_object_path(&parent.path, &name);
    Ok(Some(PdfDestination::Remote {
        path: RemotePath {
            volume_id: parent.volume_id,
            path: object_path,
        },
        name,
        overwrite: conflict == PdfCompressConflictPolicy::Replace,
    }))
}

async fn compress_pdf_to_destination(
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    ghostscript: &str,
    source_path: &Path,
    input_bytes: Option<u64>,
    destination: PdfDestination,
    profile: PdfCompressProfile,
    keep_only_smaller: bool,
) -> FsResult<PdfCompressOutcome> {
    match destination {
        PdfDestination::Local {
            path,
            name,
            overwrite,
        } => {
            let temp_path = local_temp_pdf_output_path(&path)?;
            run_ghostscript_compression(
                operation_state,
                job_id,
                ghostscript,
                source_path,
                &temp_path,
                profile,
            )?;
            let output_bytes = fs::metadata(&temp_path)
                .map_err(|error| {
                    FsError::io("Unable to read compressed PDF metadata", &temp_path, error)
                })?
                .len();

            if keep_only_smaller && input_bytes.is_some_and(|bytes| output_bytes >= bytes) {
                let _ = fs::remove_file(&temp_path);
                return Ok(PdfCompressOutcome::Skipped {
                    output_name: name,
                    message: "Compressed PDF would not be smaller.".to_string(),
                    input_bytes,
                    output_bytes: Some(output_bytes),
                });
            }

            move_compressed_local_pdf(&temp_path, &path, overwrite)?;

            Ok(PdfCompressOutcome::Compressed(CompressedPdf {
                output_path: path.to_string_lossy().into_owned(),
                output_name: name,
                input_bytes,
                output_bytes,
            }))
        }
        PdfDestination::Remote {
            path,
            name,
            overwrite,
        } => {
            let temp_path = workspace.unique_child_path(&name);
            run_ghostscript_compression(
                operation_state,
                job_id,
                ghostscript,
                source_path,
                &temp_path,
                profile,
            )?;
            let output_bytes = fs::metadata(&temp_path)
                .map_err(|error| {
                    FsError::io("Unable to read compressed PDF metadata", &temp_path, error)
                })?
                .len();

            if keep_only_smaller && input_bytes.is_some_and(|bytes| output_bytes >= bytes) {
                let _ = fs::remove_file(&temp_path);
                return Ok(PdfCompressOutcome::Skipped {
                    output_name: name,
                    message: "Compressed PDF would not be smaller.".to_string(),
                    input_bytes,
                    output_bytes: Some(output_bytes),
                });
            }

            copy_local_to_remote_item(
                remotes,
                &temp_path,
                path.clone(),
                overwrite,
                operations::SymlinkMode::Preserve,
            )
            .await?;

            Ok(PdfCompressOutcome::Compressed(CompressedPdf {
                output_path: format_remote_uri(&path.volume_id, &path.path),
                output_name: name,
                input_bytes,
                output_bytes,
            }))
        }
    }
}

fn ghostscript_program() -> FsResult<String> {
    for candidate in ["gs", "gswin64c", "gswin32c"] {
        if Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Ok(candidate.to_string());
        }
    }

    Err(FsError::new(
        "pdf_compress_unavailable",
        "Ghostscript is required to compress PDFs. Install Ghostscript and try again.",
        None,
    ))
}

fn run_ghostscript_compression(
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    ghostscript: &str,
    source_path: &Path,
    output_path: &Path,
    profile: PdfCompressProfile,
) -> FsResult<()> {
    let stderr_path = ghostscript_stderr_path(output_path);
    let stderr_file = fs::File::create(&stderr_path).map_err(|error| {
        FsError::io("Unable to prepare PDF compression log", &stderr_path, error)
    })?;
    let mut child = Command::new(ghostscript)
        .arg("-sDEVICE=pdfwrite")
        .arg("-dCompatibilityLevel=1.6")
        .arg(format!("-dPDFSETTINGS={}", profile.ghostscript_setting()))
        .arg("-dDetectDuplicateImages=true")
        .arg("-dCompressFonts=true")
        .arg("-dSubsetFonts=true")
        .arg("-dNOPAUSE")
        .arg("-dQUIET")
        .arg("-dBATCH")
        .arg(format!("-sOutputFile={}", output_path.to_string_lossy()))
        .arg(source_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr_file)
        .spawn()
        .map_err(|error| FsError::io("Unable to start PDF compression", source_path, error))?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr = read_ghostscript_stderr(&stderr_path);
                let _ = fs::remove_file(&stderr_path);

                if status.success() {
                    return Ok(());
                }

                let _ = fs::remove_file(output_path);
                let detail = stderr.trim();
                return Err(FsError::new(
                    "pdf_compress_failed",
                    if detail.is_empty() {
                        "Unable to compress PDF.".to_string()
                    } else {
                        format!("Unable to compress PDF: {detail}")
                    },
                    Some(source_path.to_string_lossy().into_owned()),
                ));
            }
            Ok(None) => {
                if let Err(error) = operation_state.checkpoint(job_id, Some(source_path)) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_file(output_path);
                    let _ = fs::remove_file(&stderr_path);
                    return Err(error);
                }

                thread::sleep(Duration::from_millis(120));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(output_path);
                let _ = fs::remove_file(&stderr_path);
                return Err(FsError::io(
                    "Unable to monitor PDF compression",
                    source_path,
                    error,
                ));
            }
        }
    }
}

fn ghostscript_stderr_path(output_path: &Path) -> PathBuf {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let name = output_path
        .file_name()
        .unwrap_or_else(|| OsStr::new("compressed.pdf"))
        .to_string_lossy();

    unique_child_path_in(
        parent,
        &format!(".carelo-pdf-compress-{}-{name}.log", random_token(8)),
    )
}

fn read_ghostscript_stderr(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn move_compressed_local_pdf(from: &Path, to: &Path, overwrite: bool) -> FsResult<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| FsError::io("Unable to create PDF output folder", parent, error))?;
    }

    if to.exists() {
        if !overwrite {
            let _ = fs::remove_file(from);
            return Err(FsError::new(
                "pdf_output_exists",
                "A file already exists with that compressed PDF name.",
                Some(to.to_string_lossy().into_owned()),
            ));
        }

        if to.is_dir() {
            let _ = fs::remove_file(from);
            return Err(FsError::new(
                "pdf_output_is_directory",
                "A folder already exists with that compressed PDF name.",
                Some(to.to_string_lossy().into_owned()),
            ));
        }

        fs::remove_file(to)
            .map_err(|error| FsError::io("Unable to replace compressed PDF", to, error))?;
    }

    fs::rename(from, to).map_err(|error| {
        let _ = fs::remove_file(from);
        FsError::io("Unable to place compressed PDF", to, error)
    })
}

fn local_temp_pdf_output_path(path: &Path) -> FsResult<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        FsError::new(
            "invalid_pdf_output",
            "Unable to resolve the PDF output folder.",
            Some(path.to_string_lossy().into_owned()),
        )
    })?;
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("compressed.pdf"))
        .to_string_lossy();

    Ok(unique_child_path_in(
        parent,
        &format!(".carelo-pdf-compress-{}-{name}", random_token(8)),
    ))
}

fn pdf_output_name(source_name: &str) -> String {
    let stem = pdf_file_stem(source_name);

    if stem.to_lowercase().ends_with(" compressed") {
        format!("{stem} 2.pdf")
    } else {
        format!("{stem} compressed.pdf")
    }
}

fn is_pdf_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn pdf_file_stem(name: &str) -> String {
    let clean_name = name
        .replace(['/', '\\'], " ")
        .trim()
        .trim_matches('.')
        .to_string();

    if clean_name.is_empty() {
        return "Document".to_string();
    }

    Path::new(&clean_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .unwrap_or("Document")
        .to_string()
}

fn unique_pdf_output_name<F>(seed_name: &str, exists: F) -> String
where
    F: Fn(&str) -> bool,
{
    if !exists(seed_name) {
        return seed_name.to_string();
    }

    let path = Path::new(seed_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Document");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    for index in 2..1000 {
        let candidate = format!("{stem} {index}{extension}");

        if !exists(&candidate) {
            return candidate;
        }
    }

    format!("{stem} {}{extension}", random_token(8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_compressed_pdf_output_name() {
        assert_eq!(pdf_output_name("Report.pdf"), "Report compressed.pdf");
        assert_eq!(
            pdf_output_name("Report compressed.pdf"),
            "Report compressed 2.pdf"
        );
        assert_eq!(pdf_output_name(""), "Document compressed.pdf");
    }

    #[test]
    fn detects_pdf_names_case_insensitively() {
        assert!(is_pdf_name("Report.PDF"));
        assert!(!is_pdf_name("Report.txt"));
        assert!(!is_pdf_name("Report"));
    }

    #[test]
    fn keeps_pdf_output_names_unique() {
        let existing = HashSet::from([
            "Report compressed.pdf".to_string(),
            "Report compressed 2.pdf".to_string(),
        ]);

        assert_eq!(
            unique_pdf_output_name("Report compressed.pdf", |name| existing.contains(name)),
            "Report compressed 3.pdf"
        );
    }

    #[test]
    fn maps_pdf_compression_profiles_to_ghostscript_settings() {
        assert_eq!(
            PdfCompressProfile::Smallest.ghostscript_setting(),
            "/screen"
        );
        assert_eq!(PdfCompressProfile::Balanced.ghostscript_setting(), "/ebook");
        assert_eq!(PdfCompressProfile::Print.ghostscript_setting(), "/printer");
        assert_eq!(
            PdfCompressProfile::Prepress.ghostscript_setting(),
            "/prepress"
        );
    }

    #[test]
    fn keeps_compressed_pdf_outputs_by_default() {
        let options: PdfCompressOptions = serde_json::from_str("{}").expect("deserialize options");

        assert_eq!(options.profile, PdfCompressProfile::Balanced);
        assert_eq!(options.conflict, PdfCompressConflictPolicy::KeepBoth);
        assert!(!options.keep_only_smaller);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_non_smaller_pdf_output_when_smaller_only_is_disabled() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "carelo-pdf-compress-keep-output-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");

        let fake_ghostscript = root.join("fake-gs");
        fs::write(
            &fake_ghostscript,
            "#!/bin/sh\nout=\"\"\nfor arg in \"$@\"; do\n  case \"$arg\" in\n    -sOutputFile=*) out=${arg#-sOutputFile=} ;;\n  esac\ndone\nprintf '%s' '%PDF larger generated output' > \"$out\"\n",
        )
        .expect("write fake ghostscript");
        let mut permissions = fs::metadata(&fake_ghostscript)
            .expect("read fake ghostscript metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_ghostscript, permissions)
            .expect("make fake ghostscript executable");

        let source = root.join("source.pdf");
        let output = root.join("source compressed.pdf");
        fs::write(&source, b"%PDF").expect("write source PDF");
        let workspace =
            TemporaryWorkspace::new("pdf-compress-keep-output-test").expect("create workspace");
        let outcome = tauri::async_runtime::block_on(compress_pdf_to_destination(
            &RemoteVolumeState::default(),
            &workspace,
            &FileOperationState::default(),
            &None,
            &fake_ghostscript.to_string_lossy(),
            &source,
            Some(4),
            PdfDestination::Local {
                path: output.clone(),
                name: "source compressed.pdf".to_string(),
                overwrite: false,
            },
            PdfCompressProfile::Balanced,
            false,
        ))
        .expect("compress PDF");

        assert!(matches!(outcome, PdfCompressOutcome::Compressed(_)));
        assert_eq!(
            fs::read(&output).expect("read compressed PDF"),
            b"%PDF larger generated output"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compresses_pdf_with_ghostscript_when_available() {
        let Ok(ghostscript) = ghostscript_program() else {
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "carelo-pdf-compress-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let source = root.join("source.pdf");
        let output = root.join("source compressed.pdf");

        fs::write(&source, minimal_pdf_bytes()).expect("write source PDF");
        run_ghostscript_compression(
            &FileOperationState::default(),
            &None,
            &ghostscript,
            &source,
            &output,
            PdfCompressProfile::Balanced,
        )
        .expect("compress PDF");

        let compressed = fs::read(&output).expect("read compressed PDF");
        assert!(compressed.starts_with(b"%PDF"));
        assert!(compressed.len() > 100);

        let _ = fs::remove_dir_all(root);
    }

    fn minimal_pdf_bytes() -> Vec<u8> {
        let mut pdf = Vec::new();
        let mut offsets = Vec::new();

        writeln!(&mut pdf, "%PDF-1.4").unwrap();
        write_pdf_object(
            &mut pdf,
            &mut offsets,
            1,
            "<< /Type /Catalog /Pages 2 0 R >>",
        );
        write_pdf_object(
            &mut pdf,
            &mut offsets,
            2,
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        );
        write_pdf_object(
            &mut pdf,
            &mut offsets,
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 120] /Resources << >> /Contents 4 0 R >>",
        );
        let content = "0.8 g\n10 10 100 100 re f\n";
        write_pdf_object(
            &mut pdf,
            &mut offsets,
            4,
            &format!(
                "<< /Length {} >>\nstream\n{}endstream",
                content.len(),
                content
            ),
        );

        let xref_offset = pdf.len();
        writeln!(&mut pdf, "xref").unwrap();
        writeln!(&mut pdf, "0 {}", offsets.len() + 1).unwrap();
        writeln!(&mut pdf, "0000000000 65535 f ").unwrap();

        for offset in offsets {
            writeln!(&mut pdf, "{offset:010} 00000 n ").unwrap();
        }

        write!(
            &mut pdf,
            "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .unwrap();

        pdf
    }

    fn write_pdf_object(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, id: usize, body: &str) {
        offsets.push(pdf.len());
        writeln!(pdf, "{id} 0 obj").unwrap();
        writeln!(pdf, "{body}").unwrap();
        writeln!(pdf, "endobj").unwrap();
    }
}
