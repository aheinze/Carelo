use super::*;
use lopdf::{Document, Object, ObjectId};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PdfToolKind {
    Merge,
    ExtractPages,
    SplitPages,
    RotatePages,
    Unlock,
}

impl PdfToolKind {
    fn operation_name(self) -> &'static str {
        match self {
            Self::Merge => "pdf-merge",
            Self::ExtractPages => "pdf-extract-pages",
            Self::SplitPages => "pdf-split-pages",
            Self::RotatePages => "pdf-rotate-pages",
            Self::Unlock => "pdf-unlock",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Merge => "merged",
            Self::ExtractPages => "extracted",
            Self::SplitPages => "split",
            Self::RotatePages => "rotated",
            Self::Unlock => "unlocked",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfToolOptions {
    pub tool: PdfToolKind,
    #[serde(default)]
    pub conflict: PdfCompressConflictPolicy,
    #[serde(default)]
    pub destination_directory: Option<String>,
    #[serde(default)]
    pub page_ranges: Option<String>,
    #[serde(default)]
    pub rotation: Option<i64>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfToolResult {
    pub source_path: Option<String>,
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

struct PdfCreatedOutput {
    output_path: String,
    output_name: String,
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

#[tauri::command]
pub async fn run_pdf_tool(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    paths: Vec<String>,
    options: PdfToolOptions,
    job_id: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<PdfToolResult>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if paths.is_empty() {
        return Err(FsError::new(
            "pdf_tool_no_paths",
            "Choose at least one PDF.",
            None,
        ));
    }

    if options.tool == PdfToolKind::Merge && paths.len() < 2 {
        return Err(FsError::new(
            "pdf_merge_needs_multiple",
            "Choose at least two PDFs to merge.",
            None,
        ));
    }

    for path in &paths {
        if archive::is_archive_uri(path) {
            return Err(archive_read_only_error(path));
        }
    }

    let workspace = TemporaryWorkspace::new(options.tool.operation_name())?;
    let destination_parent = match options.destination_directory.as_deref() {
        Some(destination) => Some(resolve_pdf_output_parent(destination)?),
        None => None,
    };
    let total_entries = paths.len() as u64;
    let mut processed_bytes = 0_u64;
    let mut sources = Vec::with_capacity(paths.len());

    for (index, path) in paths.into_iter().enumerate() {
        operation_state.checkpoint(&job_id, None)?;
        emit_file_operation_progress(
            &app,
            &job_id,
            options.tool.operation_name(),
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

        if let Some(bytes) = source.input_bytes {
            processed_bytes = processed_bytes.saturating_add(bytes);
        }

        sources.push(source);
    }

    match options.tool {
        PdfToolKind::Merge => {
            run_pdf_merge_tool(
                &app,
                &operation_state,
                &job_id,
                &remotes,
                &workspace,
                sources,
                destination_parent,
                options.conflict,
            )
            .await
        }
        PdfToolKind::ExtractPages => {
            run_pdf_per_source_tool(
                &app,
                &operation_state,
                &job_id,
                &remotes,
                &workspace,
                sources,
                destination_parent,
                options.conflict,
                options.tool,
                |source, output_path| {
                    let range_text = options.page_ranges.as_deref().unwrap_or("");
                    extract_pdf_pages(&source.local_path, output_path, range_text)
                },
                |source| {
                    let range_text = options.page_ranges.as_deref().unwrap_or("");
                    pdf_tool_output_name(
                        &source.name,
                        &format!("pages {}", pdf_range_name_fragment(range_text)),
                    )
                },
            )
            .await
        }
        PdfToolKind::SplitPages => {
            run_pdf_split_tool(
                &app,
                &operation_state,
                &job_id,
                &remotes,
                &workspace,
                sources,
                destination_parent,
                options.conflict,
            )
            .await
        }
        PdfToolKind::RotatePages => {
            let rotation = normalized_pdf_rotation(options.rotation)?;
            run_pdf_per_source_tool(
                &app,
                &operation_state,
                &job_id,
                &remotes,
                &workspace,
                sources,
                destination_parent,
                options.conflict,
                options.tool,
                |source, output_path| {
                    rotate_pdf_pages(
                        &source.local_path,
                        output_path,
                        rotation,
                        options.page_ranges.as_deref(),
                    )
                },
                |source| pdf_tool_output_name(&source.name, "rotated"),
            )
            .await
        }
        PdfToolKind::Unlock => {
            run_pdf_per_source_tool(
                &app,
                &operation_state,
                &job_id,
                &remotes,
                &workspace,
                sources,
                destination_parent,
                options.conflict,
                options.tool,
                |source, output_path| {
                    unlock_pdf(
                        &source.local_path,
                        output_path,
                        options.password.as_deref().unwrap_or(""),
                    )
                },
                |source| pdf_tool_output_name(&source.name, "unlocked"),
            )
            .await
        }
    }
}

async fn run_pdf_merge_tool(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    sources: Vec<PdfCompressSource>,
    destination_parent: Option<PdfOutputParent>,
    conflict: PdfCompressConflictPolicy,
) -> FsResult<Vec<PdfToolResult>> {
    let first_source = sources.first().ok_or_else(|| {
        FsError::new(
            "pdf_merge_no_paths",
            "Choose at least two PDFs to merge.",
            None,
        )
    })?;
    let output_parent = destination_parent
        .clone()
        .unwrap_or_else(|| first_source.source_parent.clone());
    let seed_name = pdf_tool_output_name(&first_source.name, "merged");
    let mut planned_names = HashMap::<String, HashSet<String>>::new();
    let mut remote_name_cache = HashMap::<String, HashSet<String>>::new();
    let destination = resolve_pdf_destination(
        remotes,
        output_parent,
        &seed_name,
        conflict,
        &mut planned_names,
        &mut remote_name_cache,
    )
    .await?;

    let Some(destination) = destination else {
        return Ok(vec![PdfToolResult {
            source_path: None,
            output_path: None,
            output_name: Some(seed_name),
            status: "skipped".to_string(),
            message: Some("Output already exists.".to_string()),
            input_bytes: sources
                .iter()
                .map(|source| source.input_bytes)
                .try_fold(0_u64, |total, bytes| {
                    bytes.map(|bytes| total.saturating_add(bytes))
                }),
            output_bytes: None,
        }]);
    };

    operation_state.checkpoint(job_id, None)?;
    let source_paths = sources
        .iter()
        .map(|source| source.local_path.clone())
        .collect::<Vec<_>>();
    let created = create_pdf_output(remotes, workspace, destination, |output_path| {
        merge_pdf_documents(&source_paths, output_path)
    })
    .await?;

    emit_file_operation_progress(
        app,
        job_id,
        PdfToolKind::Merge.operation_name(),
        "running",
        ProgressSnapshot {
            processed_entries: sources.len() as u64,
            total_entries: sources.len() as u64,
            ..ProgressSnapshot::default()
        },
    );

    Ok(vec![PdfToolResult {
        source_path: None,
        output_path: Some(created.output_path),
        output_name: Some(created.output_name),
        status: PdfToolKind::Merge.label().to_string(),
        message: Some(format!("Merged {} PDFs", sources.len())),
        input_bytes: sources
            .iter()
            .map(|source| source.input_bytes)
            .try_fold(0_u64, |total, bytes| {
                bytes.map(|bytes| total.saturating_add(bytes))
            }),
        output_bytes: Some(created.output_bytes),
    }])
}

async fn run_pdf_per_source_tool<WriteOutput, OutputName>(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    sources: Vec<PdfCompressSource>,
    destination_parent: Option<PdfOutputParent>,
    conflict: PdfCompressConflictPolicy,
    tool: PdfToolKind,
    write_output: WriteOutput,
    output_name: OutputName,
) -> FsResult<Vec<PdfToolResult>>
where
    WriteOutput: Fn(&PdfCompressSource, &Path) -> FsResult<()>,
    OutputName: Fn(&PdfCompressSource) -> String,
{
    let mut results = Vec::with_capacity(sources.len());
    let mut planned_names = HashMap::<String, HashSet<String>>::new();
    let mut remote_name_cache = HashMap::<String, HashSet<String>>::new();
    let total_entries = sources.len() as u64;

    for (index, source) in sources.into_iter().enumerate() {
        operation_state.checkpoint(job_id, Some(&source.local_path))?;
        emit_file_operation_progress(
            app,
            job_id,
            tool.operation_name(),
            "running",
            ProgressSnapshot {
                processed_entries: index as u64,
                total_entries,
                current_path: Some(source.original_path.clone()),
                ..ProgressSnapshot::default()
            },
        );

        let output_parent = destination_parent
            .clone()
            .unwrap_or_else(|| source.source_parent.clone());
        let seed_name = output_name(&source);
        let destination = resolve_pdf_destination(
            remotes,
            output_parent,
            &seed_name,
            conflict,
            &mut planned_names,
            &mut remote_name_cache,
        )
        .await?;

        let Some(destination) = destination else {
            results.push(PdfToolResult {
                source_path: Some(source.original_path),
                output_path: None,
                output_name: Some(seed_name),
                status: "skipped".to_string(),
                message: Some("Output already exists.".to_string()),
                input_bytes: source.input_bytes,
                output_bytes: None,
            });
            emit_file_operation_progress(
                app,
                job_id,
                tool.operation_name(),
                "running",
                ProgressSnapshot {
                    processed_entries: index as u64 + 1,
                    total_entries,
                    ..ProgressSnapshot::default()
                },
            );
            continue;
        };

        let created = create_pdf_output(remotes, workspace, destination, |output_path| {
            write_output(&source, output_path)
        })
        .await?;

        results.push(PdfToolResult {
            source_path: Some(source.original_path),
            output_path: Some(created.output_path),
            output_name: Some(created.output_name),
            status: tool.label().to_string(),
            message: Some(pdf_tool_success_message(tool)),
            input_bytes: source.input_bytes,
            output_bytes: Some(created.output_bytes),
        });

        emit_file_operation_progress(
            app,
            job_id,
            tool.operation_name(),
            "running",
            ProgressSnapshot {
                processed_entries: index as u64 + 1,
                total_entries,
                ..ProgressSnapshot::default()
            },
        );
    }

    Ok(results)
}

async fn run_pdf_split_tool(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    sources: Vec<PdfCompressSource>,
    destination_parent: Option<PdfOutputParent>,
    conflict: PdfCompressConflictPolicy,
) -> FsResult<Vec<PdfToolResult>> {
    let mut results = Vec::new();
    let mut planned_names = HashMap::<String, HashSet<String>>::new();
    let mut remote_name_cache = HashMap::<String, HashSet<String>>::new();
    let total_entries = sources.len() as u64;

    for (source_index, source) in sources.into_iter().enumerate() {
        operation_state.checkpoint(job_id, Some(&source.local_path))?;
        emit_file_operation_progress(
            app,
            job_id,
            PdfToolKind::SplitPages.operation_name(),
            "running",
            ProgressSnapshot {
                processed_entries: source_index as u64,
                total_entries,
                current_path: Some(source.original_path.clone()),
                ..ProgressSnapshot::default()
            },
        );

        let page_count = pdf_document_page_count(&source.local_path)?;
        let width = page_count.to_string().len().max(2);

        for page_number in 1..=page_count {
            operation_state.checkpoint(job_id, Some(&source.local_path))?;
            let output_parent = destination_parent
                .clone()
                .unwrap_or_else(|| source.source_parent.clone());
            let seed_name = split_pdf_output_name(&source.name, page_number, width);
            let destination = resolve_pdf_destination(
                remotes,
                output_parent,
                &seed_name,
                conflict,
                &mut planned_names,
                &mut remote_name_cache,
            )
            .await?;

            let Some(destination) = destination else {
                results.push(PdfToolResult {
                    source_path: Some(source.original_path.clone()),
                    output_path: None,
                    output_name: Some(seed_name),
                    status: "skipped".to_string(),
                    message: Some("Output already exists.".to_string()),
                    input_bytes: source.input_bytes,
                    output_bytes: None,
                });
                continue;
            };

            let created = create_pdf_output(remotes, workspace, destination, |output_path| {
                extract_single_pdf_page(&source.local_path, output_path, page_number)
            })
            .await?;

            results.push(PdfToolResult {
                source_path: Some(source.original_path.clone()),
                output_path: Some(created.output_path),
                output_name: Some(created.output_name),
                status: PdfToolKind::SplitPages.label().to_string(),
                message: Some(format!("Page {page_number} extracted")),
                input_bytes: source.input_bytes,
                output_bytes: Some(created.output_bytes),
            });
        }

        emit_file_operation_progress(
            app,
            job_id,
            PdfToolKind::SplitPages.operation_name(),
            "running",
            ProgressSnapshot {
                processed_entries: source_index as u64 + 1,
                total_entries,
                ..ProgressSnapshot::default()
            },
        );
    }

    Ok(results)
}

async fn create_pdf_output<WriteOutput>(
    remotes: &RemoteVolumeState,
    workspace: &TemporaryWorkspace,
    destination: PdfDestination,
    write_output: WriteOutput,
) -> FsResult<PdfCreatedOutput>
where
    WriteOutput: FnOnce(&Path) -> FsResult<()>,
{
    match destination {
        PdfDestination::Local {
            path,
            name,
            overwrite,
        } => {
            let temp_path = local_temp_pdf_output_path(&path)?;
            if let Err(error) = write_output(&temp_path) {
                let _ = fs::remove_file(&temp_path);
                return Err(error);
            }
            let output_bytes = match fs::metadata(&temp_path) {
                Ok(metadata) => metadata.len(),
                Err(error) => {
                    let _ = fs::remove_file(&temp_path);
                    return Err(FsError::io(
                        "Unable to read PDF output metadata",
                        &temp_path,
                        error,
                    ));
                }
            };
            move_compressed_local_pdf(&temp_path, &path, overwrite)?;

            Ok(PdfCreatedOutput {
                output_path: path.to_string_lossy().into_owned(),
                output_name: name,
                output_bytes,
            })
        }
        PdfDestination::Remote {
            path,
            name,
            overwrite,
        } => {
            let temp_path = workspace.unique_child_path(&name);
            if let Err(error) = write_output(&temp_path) {
                let _ = fs::remove_file(&temp_path);
                return Err(error);
            }
            let output_bytes = match fs::metadata(&temp_path) {
                Ok(metadata) => metadata.len(),
                Err(error) => {
                    let _ = fs::remove_file(&temp_path);
                    return Err(FsError::io(
                        "Unable to read PDF output metadata",
                        &temp_path,
                        error,
                    ));
                }
            };
            if let Err(error) = copy_local_to_remote_item(
                remotes,
                &temp_path,
                path.clone(),
                overwrite,
                operations::SymlinkMode::Preserve,
            )
            .await
            {
                let _ = fs::remove_file(&temp_path);
                return Err(error);
            }
            let _ = fs::remove_file(&temp_path);

            Ok(PdfCreatedOutput {
                output_path: format_remote_uri(&path.volume_id, &path.path),
                output_name: name,
                output_bytes,
            })
        }
    }
}

fn merge_pdf_documents(source_paths: &[PathBuf], output_path: &Path) -> FsResult<()> {
    let mut max_id = 1;
    let mut documents_pages = Vec::<(ObjectId, Object)>::new();
    let mut documents_objects = BTreeMap::<ObjectId, Object>::new();
    let mut document = Document::with_version("1.5");

    for source_path in source_paths {
        let mut source_document = load_editable_pdf_document(source_path)?;
        source_document.renumber_objects_with(max_id);
        max_id = source_document.max_id + 1;

        for page_id in source_document.get_pages().into_values() {
            let object = source_document.get_object(page_id).map_err(|error| {
                pdf_processing_error(
                    "pdf_page_missing",
                    "Unable to read PDF page",
                    source_path,
                    error,
                )
            })?;
            documents_pages.push((page_id, object.to_owned()));
        }

        documents_objects.extend(source_document.objects);
    }

    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    for (object_id, object) in documents_objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" => {
                catalog_object = Some((
                    catalog_object.map(|(id, _)| id).unwrap_or(object_id),
                    object,
                ));
            }
            b"Pages" => {
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();

                    if let Some((_, existing_object)) = &pages_object {
                        if let Ok(existing_dictionary) = existing_object.as_dict() {
                            dictionary.extend(existing_dictionary);
                        }
                    }

                    pages_object = Some((
                        pages_object.map(|(id, _)| id).unwrap_or(object_id),
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                document.objects.insert(object_id, object);
            }
        }
    }

    let Some((page_id, page_object)) = pages_object else {
        return Err(FsError::new(
            "pdf_pages_missing",
            "Unable to find pages in the selected PDFs.",
            None,
        ));
    };
    let Some((catalog_id, catalog_object)) = catalog_object else {
        return Err(FsError::new(
            "pdf_catalog_missing",
            "Unable to find a PDF catalog in the selected PDFs.",
            None,
        ));
    };

    for (object_id, object) in &documents_pages {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", page_id);
            document
                .objects
                .insert(*object_id, Object::Dictionary(dictionary));
        }
    }

    if let Ok(dictionary) = page_object.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Count", documents_pages.len() as u32);
        dictionary.set(
            "Kids",
            documents_pages
                .iter()
                .map(|(object_id, _)| Object::Reference(*object_id))
                .collect::<Vec<_>>(),
        );
        document
            .objects
            .insert(page_id, Object::Dictionary(dictionary));
    }

    if let Ok(dictionary) = catalog_object.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", page_id);
        dictionary.remove(b"Outlines");
        document
            .objects
            .insert(catalog_id, Object::Dictionary(dictionary));
    }

    document.trailer.set("Root", catalog_id);
    document.max_id = document
        .objects
        .keys()
        .map(|object_id| object_id.0)
        .max()
        .unwrap_or(0);
    document.renumber_objects();
    document.adjust_zero_pages();
    save_pdf_document(document, output_path)
}

fn extract_pdf_pages(source_path: &Path, output_path: &Path, range_text: &str) -> FsResult<()> {
    let mut document = load_editable_pdf_document(source_path)?;
    let selected_pages =
        parse_pdf_page_ranges(range_text, document.get_pages().len() as u32, false)?;
    keep_pdf_pages(&mut document, &selected_pages);
    save_pdf_document(document, output_path)
}

fn extract_single_pdf_page(
    source_path: &Path,
    output_path: &Path,
    page_number: u32,
) -> FsResult<()> {
    let mut document = load_editable_pdf_document(source_path)?;
    keep_pdf_pages(&mut document, &[page_number]);
    save_pdf_document(document, output_path)
}

fn rotate_pdf_pages(
    source_path: &Path,
    output_path: &Path,
    rotation: i64,
    range_text: Option<&str>,
) -> FsResult<()> {
    let mut document = load_editable_pdf_document(source_path)?;
    let page_count = document.get_pages().len() as u32;
    let selected_pages = match range_text.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => parse_pdf_page_ranges(value, page_count, false)?,
        None => (1..=page_count).collect(),
    };
    let pages = document.get_pages();

    for page_number in selected_pages {
        let Some(page_id) = pages.get(&page_number) else {
            continue;
        };
        let page_dict = document
            .get_object_mut(*page_id)
            .and_then(|object| object.as_dict_mut())
            .map_err(|error| {
                pdf_processing_error(
                    "pdf_rotate_failed",
                    "Unable to rotate PDF page",
                    source_path,
                    error,
                )
            })?;
        let current_rotation = page_dict
            .get(b"Rotate")
            .and_then(|object| object.as_i64())
            .unwrap_or(0);

        page_dict.set("Rotate", (current_rotation + rotation).rem_euclid(360));
    }

    save_pdf_document(document, output_path)
}

fn unlock_pdf(source_path: &Path, output_path: &Path, password: &str) -> FsResult<()> {
    let mut document = Document::load(source_path).map_err(|error| {
        pdf_processing_error(
            "pdf_unlock_failed",
            "Unable to read PDF",
            source_path,
            error,
        )
    })?;

    if document.is_encrypted() {
        document.decrypt(password).map_err(|error| {
            pdf_processing_error(
                "pdf_unlock_failed",
                "Unable to unlock PDF with that password",
                source_path,
                error,
            )
        })?;
    }

    save_pdf_document(document, output_path)
}

fn load_editable_pdf_document(path: &Path) -> FsResult<Document> {
    let document = Document::load(path).map_err(|error| {
        pdf_processing_error("pdf_read_failed", "Unable to read PDF", path, error)
    })?;

    if document.is_encrypted() {
        return Err(FsError::new(
            "pdf_encrypted",
            "Unlock this PDF before using this tool.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(document)
}

fn save_pdf_document(mut document: Document, output_path: &Path) -> FsResult<()> {
    document.prune_objects();
    document.renumber_objects();
    document.save(output_path).map(|_| ()).map_err(|error| {
        pdf_processing_error(
            "pdf_save_failed",
            "Unable to save PDF output",
            output_path,
            error,
        )
    })
}

fn keep_pdf_pages(document: &mut Document, selected_pages: &[u32]) {
    let selected = selected_pages.iter().copied().collect::<HashSet<_>>();
    let pages_to_delete = document
        .get_pages()
        .keys()
        .copied()
        .filter(|page_number| !selected.contains(page_number))
        .collect::<Vec<_>>();

    document.delete_pages(&pages_to_delete);
}

fn pdf_document_page_count(path: &Path) -> FsResult<u32> {
    let document = load_editable_pdf_document(path)?;
    Ok(document.get_pages().len() as u32)
}

fn parse_pdf_page_ranges(
    input: &str,
    page_count: u32,
    allow_empty_all: bool,
) -> FsResult<Vec<u32>> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        if allow_empty_all {
            return Ok((1..=page_count).collect());
        }

        return Err(FsError::new(
            "pdf_page_range_empty",
            "Enter one or more pages, for example 1-3,5.",
            None,
        ));
    }

    if page_count == 0 {
        return Err(FsError::new(
            "pdf_has_no_pages",
            "This PDF does not contain any pages.",
            None,
        ));
    }

    let mut pages = Vec::new();
    let mut seen = HashSet::new();

    for token in trimmed
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let (start, end) = if let Some((start, end)) = token.split_once('-') {
            let start = parse_pdf_page_number(start.trim())?;
            let end = parse_pdf_page_number(end.trim())?;

            if end < start {
                return Err(FsError::new(
                    "pdf_page_range_invalid",
                    "Page ranges must go from lower to higher pages.",
                    Some(token.to_string()),
                ));
            }

            (start, end)
        } else {
            let page = parse_pdf_page_number(token)?;
            (page, page)
        };

        if start > page_count || end > page_count {
            return Err(FsError::new(
                "pdf_page_range_out_of_bounds",
                format!("Page range exceeds the {page_count} pages in this PDF."),
                Some(token.to_string()),
            ));
        }

        for page in start..=end {
            if seen.insert(page) {
                pages.push(page);
            }
        }
    }

    if pages.is_empty() {
        return Err(FsError::new(
            "pdf_page_range_empty",
            "Enter one or more pages, for example 1-3,5.",
            None,
        ));
    }

    Ok(pages)
}

fn parse_pdf_page_number(input: &str) -> FsResult<u32> {
    let value = input.parse::<u32>().map_err(|_| {
        FsError::new(
            "pdf_page_range_invalid",
            "Page ranges can only contain page numbers.",
            Some(input.to_string()),
        )
    })?;

    if value == 0 {
        return Err(FsError::new(
            "pdf_page_range_invalid",
            "PDF page numbers start at 1.",
            Some(input.to_string()),
        ));
    }

    Ok(value)
}

fn normalized_pdf_rotation(rotation: Option<i64>) -> FsResult<i64> {
    match rotation.unwrap_or(90).rem_euclid(360) {
        90 => Ok(90),
        180 => Ok(180),
        270 => Ok(270),
        _ => Err(FsError::new(
            "pdf_rotation_invalid",
            "Choose a rotation of 90, 180, or 270 degrees.",
            None,
        )),
    }
}

fn pdf_tool_success_message(tool: PdfToolKind) -> String {
    match tool {
        PdfToolKind::Merge => "PDFs merged".to_string(),
        PdfToolKind::ExtractPages => "Pages extracted".to_string(),
        PdfToolKind::SplitPages => "PDF split into pages".to_string(),
        PdfToolKind::RotatePages => "Pages rotated".to_string(),
        PdfToolKind::Unlock => "PDF unlocked".to_string(),
    }
}

fn pdf_processing_error(
    code: &'static str,
    action: &'static str,
    path: &Path,
    error: impl std::fmt::Display,
) -> FsError {
    FsError::new(
        code,
        format!("{action}: {error}"),
        Some(path.to_string_lossy().into_owned()),
    )
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

fn pdf_tool_output_name(source_name: &str, suffix: &str) -> String {
    let stem = pdf_file_stem(source_name);
    let suffix = pdf_safe_name_fragment(suffix);
    let lower_stem = stem.to_lowercase();
    let lower_suffix = suffix.to_lowercase();

    if lower_stem.ends_with(&format!(" {lower_suffix}")) {
        format!("{stem} 2.pdf")
    } else {
        format!("{stem} {suffix}.pdf")
    }
}

fn split_pdf_output_name(source_name: &str, page_number: u32, width: usize) -> String {
    let stem = pdf_file_stem(source_name);

    format!("{stem} page {page_number:0width$}.pdf")
}

fn pdf_range_name_fragment(range_text: &str) -> String {
    let fragment = pdf_safe_name_fragment(range_text);

    if fragment.is_empty() {
        "selection".to_string()
    } else {
        fragment
    }
}

fn pdf_safe_name_fragment(value: &str) -> String {
    let mut fragment = String::new();
    let mut last_was_space = true;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '-' {
            fragment.push(character);
            last_was_space = false;
        } else if !last_was_space {
            fragment.push(' ');
            last_was_space = true;
        }
    }

    fragment.trim().to_string()
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
    fn names_pdf_tool_outputs_safely() {
        assert_eq!(
            pdf_tool_output_name("Report.pdf", "merged"),
            "Report merged.pdf"
        );
        assert_eq!(
            pdf_tool_output_name("Report merged.pdf", "merged"),
            "Report merged 2.pdf"
        );
        assert_eq!(
            pdf_tool_output_name("Report.pdf", "pages 1-3, 5"),
            "Report pages 1-3 5.pdf"
        );
        assert_eq!(
            split_pdf_output_name("Report.pdf", 3, 2),
            "Report page 03.pdf"
        );
    }

    #[test]
    fn parses_pdf_page_ranges_in_order_without_duplicates() {
        assert_eq!(
            parse_pdf_page_ranges("1-3, 2, 5,8", 8, false).expect("parse page ranges"),
            vec![1, 2, 3, 5, 8]
        );
        assert!(parse_pdf_page_ranges("0", 8, false).is_err());
        assert!(parse_pdf_page_ranges("9", 8, false).is_err());
        assert!(parse_pdf_page_ranges("5-3", 8, false).is_err());
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
    fn removes_temp_pdf_output_after_write_failure() {
        let root = std::env::temp_dir().join(format!(
            "carelo-pdf-temp-cleanup-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let output = root.join("output.pdf");
        let workspace = TemporaryWorkspace::new("pdf-temp-cleanup-test").expect("create workspace");

        let result = tauri::async_runtime::block_on(create_pdf_output(
            &RemoteVolumeState::default(),
            &workspace,
            PdfDestination::Local {
                path: output.clone(),
                name: "output.pdf".to_string(),
                overwrite: false,
            },
            |temp_path| {
                fs::write(temp_path, b"partial").expect("write partial output");
                Err(FsError::new("test_error", "test failure", None))
            },
        ));

        assert!(result.is_err());
        assert!(!output.exists());
        let leaked_temp = fs::read_dir(&root)
            .expect("read test directory")
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".carelo-pdf-compress-")
            });
        assert!(!leaked_temp);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merges_and_rotates_pdf_documents() {
        let root = std::env::temp_dir().join(format!(
            "carelo-pdf-tools-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let first = root.join("first.pdf");
        let second = root.join("second.pdf");
        let merged = root.join("merged.pdf");
        let rotated = root.join("rotated.pdf");

        fs::write(&first, minimal_pdf_bytes()).expect("write first PDF");
        fs::write(&second, minimal_pdf_bytes()).expect("write second PDF");
        merge_pdf_documents(&[first.clone(), second], &merged).expect("merge PDFs");

        let merged_document = Document::load(&merged).expect("load merged PDF");
        assert_eq!(merged_document.get_pages().len(), 2);

        rotate_pdf_pages(&merged, &rotated, 90, Some("2")).expect("rotate PDF");
        let rotated_document = Document::load(&rotated).expect("load rotated PDF");
        let pages = rotated_document.get_pages();
        let first_rotation = pages
            .get(&1)
            .and_then(|page_id| rotated_document.get_object(*page_id).ok())
            .and_then(|object| object.as_dict().ok())
            .and_then(|dict| dict.get(b"Rotate").ok())
            .and_then(|object| object.as_i64().ok());
        let second_rotation = pages
            .get(&2)
            .and_then(|page_id| rotated_document.get_object(*page_id).ok())
            .and_then(|object| object.as_dict().ok())
            .and_then(|dict| dict.get(b"Rotate").ok())
            .and_then(|object| object.as_i64().ok());

        assert_eq!(first_rotation, None);
        assert_eq!(second_rotation, Some(90));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_single_pdf_page() {
        let root = std::env::temp_dir().join(format!(
            "carelo-pdf-extract-test-{}-{}",
            std::process::id(),
            random_token(8)
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let first = root.join("first.pdf");
        let second = root.join("second.pdf");
        let merged = root.join("merged.pdf");
        let extracted = root.join("page.pdf");

        fs::write(&first, minimal_pdf_bytes()).expect("write first PDF");
        fs::write(&second, minimal_pdf_bytes()).expect("write second PDF");
        merge_pdf_documents(&[first, second], &merged).expect("merge PDFs");
        extract_single_pdf_page(&merged, &extracted, 2).expect("extract page");

        let extracted_document = Document::load(&extracted).expect("load extracted PDF");
        assert_eq!(extracted_document.get_pages().len(), 1);

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
