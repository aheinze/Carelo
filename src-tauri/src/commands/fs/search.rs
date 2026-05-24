use super::*;
use std::fs::File;
use std::io::{BufRead, BufReader};

const FILE_RESULT_BUFFER_MULTIPLIER: usize = 4;
const FILE_RESULT_RETAIN_MULTIPLIER: usize = 2;
const FILE_SEARCH_PROGRESS_INTERVAL: Duration = Duration::from_millis(80);
const FILE_SEARCH_CACHE_TTL: Duration = Duration::from_secs(8);
const FILE_SEARCH_CACHE_MAX_ENTRIES: usize = 8;
const FILE_SEARCH_CACHE_MAX_CANDIDATES: usize = 50_000;
const CONTENT_RESULT_BUFFER_MULTIPLIER: usize = 4;
const CONTENT_RESULT_RETAIN_MULTIPLIER: usize = 2;
const CONTENT_PROGRESS_INTERVAL: Duration = Duration::from_millis(140);
const CONTENT_SNIPPET_CONTEXT_CHARS: usize = 90;
const CONTENT_SNIPPET_MAX_CHARS: usize = 260;

#[derive(Debug, Clone, Default)]
struct FileSearchProgress {
    scanned_entries: u64,
    matched_entries: u64,
    total_entries: u64,
    current_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSearchResultsPayload {
    job_id: String,
    root: String,
    query: String,
    scanned_entries: u64,
    matched_entries: u64,
    done: bool,
    results: Vec<FileSearchResult>,
}

#[derive(Debug, Clone)]
struct FileSearchCandidate {
    name: String,
    path: String,
    parent_path: String,
    kind: String,
    candidate: String,
    size: Option<u64>,
    modified_at: Option<u64>,
}

#[derive(Debug, Clone)]
struct FileSearchCacheEntry {
    created_at: Instant,
    candidates: Vec<FileSearchCandidate>,
}

#[derive(Clone, Default)]
pub struct FileSearchIndexState {
    entries: Arc<Mutex<HashMap<String, FileSearchCacheEntry>>>,
}

impl FileSearchIndexState {
    fn get(&self, key: &str) -> Option<Vec<FileSearchCandidate>> {
        let mut entries = self.entries.lock().ok()?;
        let now = Instant::now();
        entries.retain(|_, entry| now.duration_since(entry.created_at) <= FILE_SEARCH_CACHE_TTL);
        entries.get(key).map(|entry| entry.candidates.clone())
    }

    fn put(&self, key: String, candidates: Vec<FileSearchCandidate>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let now = Instant::now();
        entries.retain(|_, entry| now.duration_since(entry.created_at) <= FILE_SEARCH_CACHE_TTL);

        if entries.len() >= FILE_SEARCH_CACHE_MAX_ENTRIES {
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.created_at)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest_key);
            }
        }

        entries.insert(
            key,
            FileSearchCacheEntry {
                created_at: now,
                candidates,
            },
        );
    }
}

#[derive(Debug, Clone, Default)]
struct ContentSearchProgress {
    scanned_files: u64,
    matched_files: u64,
    current_path: Option<String>,
}

#[tauri::command]
pub async fn search_files(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    root: String,
    query: String,
    options: Option<FileSearchOptions>,
    job_id: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
    file_search_index: tauri::State<'_, FileSearchIndexState>,
) -> Result<Vec<FileSearchResult>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if let Some(remote_root) = parse_remote_path(&root) {
        return search_remote_files(
            &app,
            operation_state.inner(),
            &job_id,
            &remotes,
            file_search_index.inner(),
            remote_root,
            &query,
            options.unwrap_or_else(default_search_options),
        )
        .await;
    }

    let search_app = app.clone();
    let search_job_id = job_id.clone();
    let search_operation_state = operation_state.inner().clone();
    let search_index = file_search_index.inner().clone();
    run_local(move |_| {
        search_local_files(
            &search_app,
            &search_operation_state,
            &search_job_id,
            &search_index,
            &root,
            &query,
            options.unwrap_or_else(default_search_options),
        )
    })
    .await
}

#[tauri::command]
pub async fn search_content(
    app: AppHandle,
    operation_state: tauri::State<'_, FileOperationState>,
    root: String,
    query: String,
    options: Option<ContentSearchOptions>,
    job_id: Option<String>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<ContentSearchResult>, FsError> {
    let _operation_cleanup =
        OperationStateCleanup::new(operation_state.inner().clone(), job_id.clone());

    if let Some(remote_root) = parse_remote_path(&root) {
        return search_remote_content(
            &app,
            operation_state.inner(),
            &job_id,
            &remotes,
            remote_root,
            &query,
            options.unwrap_or_else(default_content_search_options),
        )
        .await;
    }

    let search_app = app.clone();
    let search_job_id = job_id.clone();
    let search_operation_state = operation_state.inner().clone();
    run_local(move |_| {
        search_local_content(
            &root,
            &query,
            options.unwrap_or_else(default_content_search_options),
            |progress| {
                emit_content_search_progress(&search_app, &search_job_id, progress);
            },
            |path| search_operation_state.checkpoint(&search_job_id, path),
        )
    })
    .await
}

fn default_search_options() -> FileSearchOptions {
    FileSearchOptions {
        limit: default_file_search_limit(),
        include_hidden: false,
        respect_ignore: true,
        include_files: true,
        include_directories: true,
        follow_symlinks: false,
        max_depth: None,
    }
}

fn default_content_search_options() -> ContentSearchOptions {
    ContentSearchOptions {
        limit: default_content_search_limit(),
        include_hidden: false,
        respect_ignore: true,
        case_sensitive: false,
        regex: false,
        max_file_bytes: default_content_search_max_file_bytes(),
        max_depth: None,
    }
}

fn emit_content_search_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    progress: ContentSearchProgress,
) {
    emit_file_operation_progress(
        app,
        job_id,
        "content-search",
        "running",
        ProgressSnapshot {
            processed_entries: progress.scanned_files,
            total_entries: 0,
            current_path: progress.current_path,
            current_bytes: progress.matched_files,
            ..ProgressSnapshot::default()
        },
    );
}

fn emit_file_search_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    progress: &FileSearchProgress,
) {
    emit_file_operation_progress(
        app,
        job_id,
        "file-search",
        "running",
        ProgressSnapshot {
            processed_entries: progress.scanned_entries,
            total_entries: progress.total_entries,
            current_path: progress.current_path.clone(),
            current_bytes: progress.matched_entries,
            ..ProgressSnapshot::default()
        },
    );
}

fn emit_file_search_results(
    app: &AppHandle,
    job_id: &Option<String>,
    root: &str,
    query: &str,
    progress: &FileSearchProgress,
    results: &[FileSearchResult],
    done: bool,
) {
    let Some(job_id) = job_id else {
        return;
    };

    let _ = app.emit(
        "file-search-results",
        FileSearchResultsPayload {
            job_id: job_id.clone(),
            root: root.to_string(),
            query: query.to_string(),
            scanned_entries: progress.scanned_entries,
            matched_entries: progress.matched_entries,
            done,
            results: results.to_vec(),
        },
    );
}

fn emit_file_search_update(
    app: &AppHandle,
    job_id: &Option<String>,
    root: &str,
    query: &str,
    progress: &FileSearchProgress,
    results: &[FileSearchResult],
    done: bool,
) {
    emit_file_search_progress(app, job_id, progress);
    emit_file_search_results(app, job_id, root, query, progress, results, done);
}

pub(super) fn expand_local_search_root(root: &str) -> FsResult<PathBuf> {
    let trimmed = root.trim();

    if trimmed.is_empty() || trimmed == "~" {
        return LocalFileProvider::home_dir();
    }

    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Ok(LocalFileProvider::home_dir()?.join(rest));
    }

    if archive::is_archive_uri(trimmed) || parse_remote_path(trimmed).is_some() {
        return Err(FsError::new(
            "unsupported_search_root",
            "Fuzzy file search currently supports local folders only.",
            Some(trimmed.to_string()),
        ));
    }

    Ok(PathBuf::from(trimmed))
}

fn configure_walk_builder(
    root_path: &Path,
    include_hidden: bool,
    respect_ignore: bool,
    follow_symlinks: bool,
    max_depth: Option<usize>,
) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root_path);
    builder
        .hidden(!include_hidden)
        .follow_links(follow_symlinks)
        .min_depth(Some(1));

    if let Some(max_depth) = max_depth {
        builder.max_depth(Some(max_depth.max(1)));
    }

    if !respect_ignore {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    }

    builder
}

fn search_result_kind(metadata: &fs::Metadata, is_symlink: bool) -> &'static str {
    if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else if is_symlink {
        "symlink"
    } else {
        "other"
    }
}

fn file_search_cache_key(scope: &str, root: &str, options: &FileSearchOptions) -> String {
    format!(
        "{scope}\0{root}\0hidden={}\0ignore={}\0files={}\0dirs={}\0links={}\0depth={:?}",
        options.include_hidden,
        options.respect_ignore,
        options.include_files,
        options.include_directories,
        options.follow_symlinks,
        options.max_depth
    )
}

fn sort_file_results(results: &mut [FileSearchResult]) {
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
}

fn sort_and_truncate_file_results(results: &mut Vec<FileSearchResult>, limit: usize) {
    sort_file_results(results);
    results.truncate(limit);
}

fn maybe_trim_file_results(results: &mut Vec<FileSearchResult>, limit: usize) {
    let buffer_limit = limit
        .saturating_mul(FILE_RESULT_BUFFER_MULTIPLIER)
        .max(limit);

    if results.len() <= buffer_limit {
        return;
    }

    let retain_limit = limit
        .saturating_mul(FILE_RESULT_RETAIN_MULTIPLIER)
        .max(limit);
    sort_and_truncate_file_results(results, retain_limit);
}

fn top_file_results(results: &[FileSearchResult], limit: usize) -> Vec<FileSearchResult> {
    let mut top_results = results.to_vec();
    sort_and_truncate_file_results(&mut top_results, limit);
    top_results
}

fn name_match_indices(candidate: &str, name: &str, indices: &[u32]) -> Vec<usize> {
    let candidate_chars = candidate.chars().count();
    let name_chars = name.chars().count();
    let name_start = candidate_chars.saturating_sub(name_chars);
    let name_end = name_start + name_chars;
    let mut name_indices = indices
        .iter()
        .filter_map(|index| usize::try_from(*index).ok())
        .filter_map(|index| {
            if (name_start..name_end).contains(&index) {
                Some(index - name_start)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    name_indices.sort_unstable();
    name_indices.dedup();
    name_indices
}

fn file_search_result_from_candidate(
    candidate: &FileSearchCandidate,
    query: &str,
    pattern: &Pattern,
    matcher: &mut Matcher,
    haystack_buf: &mut Vec<char>,
    indices_buf: &mut Vec<u32>,
) -> Option<FileSearchResult> {
    indices_buf.clear();

    let score = if query.is_empty() {
        0
    } else {
        pattern.indices(
            Utf32Str::new(candidate.candidate.as_str(), haystack_buf),
            matcher,
            indices_buf,
        )?
    };

    Some(FileSearchResult {
        name: candidate.name.clone(),
        path: candidate.path.clone(),
        parent_path: candidate.parent_path.clone(),
        kind: candidate.kind.clone(),
        score: i64::from(score),
        match_indices: name_match_indices(&candidate.candidate, &candidate.name, indices_buf),
        size: candidate.size,
        modified_at: candidate.modified_at,
    })
}

fn emit_file_scan_progress(
    app: &AppHandle,
    job_id: &Option<String>,
    root: &str,
    query: &str,
    last_emit: &mut Instant,
    progress: &FileSearchProgress,
    results: &[FileSearchResult],
    limit: usize,
    force: bool,
) {
    if !force && last_emit.elapsed() < FILE_SEARCH_PROGRESS_INTERVAL {
        return;
    }

    *last_emit = Instant::now();
    let top_results = top_file_results(results, limit);
    emit_file_search_update(app, job_id, root, query, progress, &top_results, false);
}

fn score_file_candidates(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    root: &str,
    query: &str,
    candidates: &[FileSearchCandidate],
    options: &FileSearchOptions,
) -> FsResult<Vec<FileSearchResult>> {
    let limit = options.limit.clamp(1, 500);
    let query = query.trim();
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut haystack_buf = Vec::new();
    let mut indices_buf = Vec::new();
    let mut results = Vec::new();
    let mut progress = FileSearchProgress {
        total_entries: u64::try_from(candidates.len()).unwrap_or(u64::MAX),
        ..FileSearchProgress::default()
    };
    let mut last_progress_emit = Instant::now()
        .checked_sub(FILE_SEARCH_PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    emit_file_scan_progress(
        app,
        job_id,
        root,
        query,
        &mut last_progress_emit,
        &progress,
        &results,
        limit,
        true,
    );

    for candidate in candidates {
        operation_state.checkpoint(job_id, None)?;

        progress.scanned_entries = progress.scanned_entries.saturating_add(1);
        progress.current_path = Some(candidate.path.clone());

        if let Some(result) = file_search_result_from_candidate(
            candidate,
            query,
            &pattern,
            &mut matcher,
            &mut haystack_buf,
            &mut indices_buf,
        ) {
            progress.matched_entries = progress.matched_entries.saturating_add(1);
            results.push(result);
            maybe_trim_file_results(&mut results, limit);
        }

        emit_file_scan_progress(
            app,
            job_id,
            root,
            query,
            &mut last_progress_emit,
            &progress,
            &results,
            limit,
            false,
        );
    }

    progress.current_path = None;
    sort_and_truncate_file_results(&mut results, limit);
    emit_file_search_update(app, job_id, root, query, &progress, &results, true);
    Ok(results)
}

fn local_file_search_candidate(
    root_path: &Path,
    path: &Path,
    metadata: &fs::Metadata,
    kind: &str,
) -> FileSearchCandidate {
    let candidate = path
        .strip_prefix(root_path)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();
    let parent_path = path
        .parent()
        .unwrap_or(root_path)
        .to_string_lossy()
        .into_owned();

    FileSearchCandidate {
        name,
        path: path.to_string_lossy().into_owned(),
        parent_path,
        kind: kind.to_string(),
        candidate,
        size: metadata.is_file().then_some(metadata.len()),
        modified_at: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs()),
    }
}

fn search_local_files(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    file_search_index: &FileSearchIndexState,
    root: &str,
    query: &str,
    options: FileSearchOptions,
) -> FsResult<Vec<FileSearchResult>> {
    let root_path = expand_local_search_root(root)?;
    let root_metadata = fs::metadata(&root_path)
        .map_err(|error| FsError::io("Unable to read search root", &root_path, error))?;

    if !root_metadata.is_dir() {
        return Err(FsError::new(
            "search_root_not_directory",
            "Search root must be a local folder.",
            Some(root_path.to_string_lossy().into_owned()),
        ));
    }

    let query = query.trim();
    let limit = options.limit.clamp(1, 500);
    let cache_key = file_search_cache_key("local", &root_path.to_string_lossy(), &options);

    if let Some(candidates) = file_search_index.get(&cache_key) {
        return score_file_candidates(
            app,
            operation_state,
            job_id,
            &root_path.to_string_lossy(),
            query,
            &candidates,
            &options,
        );
    }

    let builder = configure_walk_builder(
        &root_path,
        options.include_hidden,
        options.respect_ignore,
        options.follow_symlinks,
        options.max_depth,
    );

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut results = Vec::new();
    let mut haystack_buf = Vec::new();
    let mut indices_buf = Vec::new();
    let mut candidates = Vec::new();
    let mut cacheable = true;
    let mut progress = FileSearchProgress::default();
    let root_label = root_path.to_string_lossy().into_owned();
    let mut last_progress_emit = Instant::now()
        .checked_sub(FILE_SEARCH_PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    emit_file_scan_progress(
        app,
        job_id,
        &root_label,
        query,
        &mut last_progress_emit,
        &progress,
        &results,
        limit,
        true,
    );

    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        operation_state.checkpoint(job_id, Some(path))?;

        if path == root_path {
            continue;
        }

        let symlink_metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let is_symlink = symlink_metadata.file_type().is_symlink();
        let metadata = if is_symlink {
            fs::metadata(path).unwrap_or(symlink_metadata)
        } else {
            symlink_metadata
        };
        let kind = search_result_kind(&metadata, is_symlink);

        if (kind == "directory" && !options.include_directories)
            || (kind != "directory" && !options.include_files)
        {
            continue;
        }

        let candidate = local_file_search_candidate(&root_path, path, &metadata, kind);
        progress.scanned_entries = progress.scanned_entries.saturating_add(1);
        progress.current_path = Some(candidate.path.clone());

        if let Some(result) = file_search_result_from_candidate(
            &candidate,
            query,
            &pattern,
            &mut matcher,
            &mut haystack_buf,
            &mut indices_buf,
        ) {
            progress.matched_entries = progress.matched_entries.saturating_add(1);
            results.push(result);
            maybe_trim_file_results(&mut results, limit);
        }

        if cacheable {
            if candidates.len() < FILE_SEARCH_CACHE_MAX_CANDIDATES {
                candidates.push(candidate);
            } else {
                candidates.clear();
                cacheable = false;
            }
        }

        emit_file_scan_progress(
            app,
            job_id,
            &root_label,
            query,
            &mut last_progress_emit,
            &progress,
            &results,
            limit,
            false,
        );
    }

    if cacheable {
        file_search_index.put(cache_key, candidates);
    }
    progress.current_path = None;
    sort_and_truncate_file_results(&mut results, limit);
    emit_file_search_update(app, job_id, &root_label, query, &progress, &results, true);
    Ok(results)
}

fn remote_file_search_candidate(root_uri: &str, entry: &FileEntry) -> FileSearchCandidate {
    FileSearchCandidate {
        name: entry.name.clone(),
        path: entry.path.clone(),
        parent_path: parent_path_for_remote_uri(&entry.path),
        kind: match entry.kind {
            FileEntryKind::Directory => "directory",
            FileEntryKind::File => "file",
            FileEntryKind::Symlink => "symlink",
            FileEntryKind::Other => "other",
        }
        .to_string(),
        candidate: remote_search_candidate(root_uri, &entry.path),
        size: entry.size,
        modified_at: entry.modified_at,
    }
}

async fn search_remote_files(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    remotes: &RemoteVolumeState,
    file_search_index: &FileSearchIndexState,
    root: RemotePath,
    query: &str,
    options: FileSearchOptions,
) -> FsResult<Vec<FileSearchResult>> {
    let limit = options.limit.clamp(1, 500);
    let query = query.trim();
    let root_uri = crate::fs::remote::format_remote_uri(&root.volume_id, &root.path);
    let cache_key = file_search_cache_key("remote", &root_uri, &options);

    if let Some(candidates) = file_search_index.get(&cache_key) {
        return score_file_candidates(
            app,
            operation_state,
            job_id,
            &root_uri,
            query,
            &candidates,
            &options,
        );
    }

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut results = Vec::new();
    let mut haystack_buf = Vec::new();
    let mut indices_buf = Vec::new();
    let mut candidates = Vec::new();
    let mut cacheable = true;
    let mut stack = vec![(root, 0_usize)];
    let mut progress = FileSearchProgress::default();
    let mut last_progress_emit = Instant::now()
        .checked_sub(FILE_SEARCH_PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    emit_file_scan_progress(
        app,
        job_id,
        &root_uri,
        query,
        &mut last_progress_emit,
        &progress,
        &results,
        limit,
        true,
    );

    while let Some((directory, depth)) = stack.pop() {
        operation_state.checkpoint(job_id, None)?;

        let entries = list_remote_directory(remotes, directory.clone()).await?;

        for entry in entries {
            operation_state.checkpoint(job_id, None)?;

            if !options.include_hidden && entry.is_hidden {
                continue;
            }

            let is_directory = entry.kind == FileEntryKind::Directory;

            if (is_directory && !options.include_directories)
                || (!is_directory && !options.include_files)
            {
                if is_directory && should_descend_remote(depth, options.max_depth) {
                    if let Some(remote_path) = parse_remote_path(&entry.path) {
                        stack.push((remote_path, depth + 1));
                    }
                }
                continue;
            }

            let candidate = remote_file_search_candidate(&root_uri, &entry);
            progress.scanned_entries = progress.scanned_entries.saturating_add(1);
            progress.current_path = Some(candidate.path.clone());

            if let Some(result) = file_search_result_from_candidate(
                &candidate,
                query,
                &pattern,
                &mut matcher,
                &mut haystack_buf,
                &mut indices_buf,
            ) {
                progress.matched_entries = progress.matched_entries.saturating_add(1);
                results.push(result);
                maybe_trim_file_results(&mut results, limit);
            } else {
                if is_directory && should_descend_remote(depth, options.max_depth) {
                    if let Some(remote_path) = parse_remote_path(&entry.path) {
                        stack.push((remote_path, depth + 1));
                    }
                }
                if cacheable {
                    if candidates.len() < FILE_SEARCH_CACHE_MAX_CANDIDATES {
                        candidates.push(candidate);
                    } else {
                        candidates.clear();
                        cacheable = false;
                    }
                }
                emit_file_scan_progress(
                    app,
                    job_id,
                    &root_uri,
                    query,
                    &mut last_progress_emit,
                    &progress,
                    &results,
                    limit,
                    false,
                );
                continue;
            }

            if cacheable {
                if candidates.len() < FILE_SEARCH_CACHE_MAX_CANDIDATES {
                    candidates.push(candidate);
                } else {
                    candidates.clear();
                    cacheable = false;
                }
            }

            if is_directory && should_descend_remote(depth, options.max_depth) {
                if let Some(remote_path) = parse_remote_path(&entry.path) {
                    stack.push((remote_path, depth + 1));
                }
            }

            emit_file_scan_progress(
                app,
                job_id,
                &root_uri,
                query,
                &mut last_progress_emit,
                &progress,
                &results,
                limit,
                false,
            );
        }
    }

    if cacheable {
        file_search_index.put(cache_key, candidates);
    }
    progress.current_path = None;
    sort_and_truncate_file_results(&mut results, limit);
    emit_file_search_update(app, job_id, &root_uri, query, &progress, &results, true);
    Ok(results)
}

fn should_descend_remote(current_depth: usize, max_depth: Option<usize>) -> bool {
    max_depth
        .map(|max_depth| current_depth + 1 < max_depth.max(1))
        .unwrap_or(true)
}

fn remote_search_candidate(root_uri: &str, path: &str) -> String {
    path.strip_prefix(root_uri)
        .unwrap_or(path)
        .trim_start_matches('/')
        .to_string()
}

fn parent_path_for_remote_uri(path: &str) -> String {
    let Some(remote_path) = parse_remote_path(path) else {
        return String::new();
    };
    let object_path = remote_path.path.trim_matches('/');
    let Some(index) = object_path.rfind('/') else {
        return crate::fs::remote::format_remote_uri(&remote_path.volume_id, "");
    };

    crate::fs::remote::format_remote_uri(&remote_path.volume_id, &object_path[..index])
}

pub(super) fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

fn lower_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.to_ascii_lowercase())
}

fn office_document_kind(extension: &str) -> Option<&'static str> {
    match extension {
        "docx" | "docm" | "dotx" | "dotm" => Some("word"),
        "xlsx" | "xlsm" | "xltx" | "xltm" => Some("excel"),
        "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" => Some("powerpoint"),
        "odt" | "ods" | "odp" => Some("opendocument"),
        _ => None,
    }
}

fn searchable_content_for_path<'a>(path: &Path, bytes: &'a [u8]) -> Option<Cow<'a, str>> {
    match lower_extension(path).as_deref() {
        Some("pdf") => extract_pdf_search_text(bytes).map(Cow::Owned),
        Some(extension) if office_document_kind(extension).is_some() => {
            extract_office_document_text(bytes, extension).map(Cow::Owned)
        }
        _ if !is_probably_binary(bytes) => Some(String::from_utf8_lossy(bytes)),
        _ => None,
    }
}

fn extract_pdf_search_text(bytes: &[u8]) -> Option<String> {
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .ok()?
        .ok()?;
    non_empty_extracted_text(text)
}

fn extract_office_document_text(bytes: &[u8], extension: &str) -> Option<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut text = String::new();

    for index in 0..archive.len() {
        if text.len() >= EXTRACTED_TEXT_MAX_BYTES {
            break;
        }

        let file = match archive.by_index(index) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let part_name = file.name().to_ascii_lowercase();

        if part_name.ends_with('/')
            || !is_office_text_part(extension, &part_name)
            || file.size() > OFFICE_XML_PART_MAX_BYTES
        {
            continue;
        }

        let mut part_bytes =
            Vec::with_capacity(file.size().min(OFFICE_XML_PART_MAX_BYTES) as usize);
        let mut limited_file = file.take(OFFICE_XML_PART_MAX_BYTES + 1);

        if limited_file.read_to_end(&mut part_bytes).is_err()
            || part_bytes.len() as u64 > OFFICE_XML_PART_MAX_BYTES
        {
            continue;
        }

        let xml = String::from_utf8_lossy(&part_bytes);
        append_office_xml_text(&xml, &mut text);
    }

    non_empty_extracted_text(text)
}

fn is_office_text_part(extension: &str, name: &str) -> bool {
    match office_document_kind(extension) {
        Some("word") => {
            matches!(
                name,
                "word/document.xml"
                    | "word/footnotes.xml"
                    | "word/endnotes.xml"
                    | "word/comments.xml"
            ) || ((name.starts_with("word/header") || name.starts_with("word/footer"))
                && name.ends_with(".xml"))
        }
        Some("excel") => {
            name == "xl/sharedstrings.xml"
                || ((name.starts_with("xl/worksheets/")
                    || name.starts_with("xl/chartsheets/")
                    || name.starts_with("xl/comments"))
                    && name.ends_with(".xml"))
        }
        Some("powerpoint") => {
            ((name.starts_with("ppt/slides/slide")
                || name.starts_with("ppt/notesslides/notesslide")
                || name.starts_with("ppt/comments/comment"))
                && name.ends_with(".xml"))
                || name == "ppt/presentation.xml"
        }
        Some("opendocument") => name == "content.xml" || name == "meta.xml",
        _ => false,
    }
}

fn append_office_xml_text(xml: &str, output: &mut String) {
    let mut reader = Reader::from_str(xml);

    loop {
        if output.len() >= EXTRACTED_TEXT_MAX_BYTES {
            break;
        }

        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Text(event)) => {
                if let Ok(decoded) = event.decode() {
                    let fragment = quick_xml::escape::unescape(&decoded)
                        .map(Cow::into_owned)
                        .unwrap_or_else(|_| decoded.into_owned());
                    push_extracted_text_fragment(output, &fragment);
                }
            }
            Ok(Event::CData(event)) => {
                if let Ok(decoded) = event.decode() {
                    push_extracted_text_fragment(output, &decoded);
                }
            }
            Ok(Event::GeneralRef(event)) => {
                if let Ok(Some(character)) = event.resolve_char_ref() {
                    let mut buffer = [0; 4];
                    push_extracted_text_fragment(output, character.encode_utf8(&mut buffer));
                } else if let Ok(name) = event.decode() {
                    if let Some(value) = quick_xml::escape::resolve_predefined_entity(&name) {
                        push_extracted_text_fragment(output, value);
                    }
                }
            }
            Ok(Event::End(event)) => {
                if is_office_xml_line_break(event.name().as_ref()) {
                    push_extracted_text_break(output);
                }
            }
            Ok(Event::Empty(event)) => {
                if is_office_xml_inline_break(event.name().as_ref()) {
                    push_extracted_text_break(output);
                }
            }
            Err(_) => break,
            _ => {}
        }
    }
}

fn is_office_xml_line_break(name: &[u8]) -> bool {
    let name = xml_local_name(name);
    name == b"p" || name == b"row" || name == b"tr"
}

fn is_office_xml_inline_break(name: &[u8]) -> bool {
    let name = xml_local_name(name);
    name == b"br" || name == b"cr"
}

fn xml_local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn push_extracted_text_fragment(output: &mut String, fragment: &str) {
    if fragment.trim().is_empty() || output.len() >= EXTRACTED_TEXT_MAX_BYTES {
        return;
    }

    let mut pending_space = false;

    for character in fragment.chars() {
        if output.len() >= EXTRACTED_TEXT_MAX_BYTES {
            break;
        }

        if character.is_whitespace() {
            pending_space = true;
            continue;
        }

        if pending_space {
            push_extracted_pending_space(output);
            pending_space = false;
        }

        let mut buffer = [0; 4];
        append_extracted_text_with_limit(output, character.encode_utf8(&mut buffer));
    }

    if pending_space {
        push_extracted_pending_space(output);
    }
}

fn push_extracted_pending_space(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') && !output.ends_with(' ') {
        append_extracted_text_with_limit(output, " ");
    }
}

fn push_extracted_text_break(output: &mut String) {
    while output.ends_with(' ') || output.ends_with('\t') {
        output.pop();
    }

    if !output.is_empty() && !output.ends_with('\n') {
        append_extracted_text_with_limit(output, "\n");
    }
}

fn append_extracted_text_with_limit(output: &mut String, text: &str) {
    let remaining = EXTRACTED_TEXT_MAX_BYTES.saturating_sub(output.len());

    if remaining == 0 {
        return;
    }

    if text.len() <= remaining {
        output.push_str(text);
        return;
    }

    let mut end = remaining;

    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    output.push_str(&text[..end]);
}

fn non_empty_extracted_text(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn find_plain_match(line: &str, query: &str, case_sensitive: bool) -> Option<(usize, usize)> {
    if case_sensitive {
        return line.find(query).map(|start| (start, start + query.len()));
    }

    let line_lower = line.to_lowercase();
    let query_lower = query.to_lowercase();
    line_lower
        .find(&query_lower)
        .map(|start| (start, start + query_lower.len()))
}

fn byte_to_char_index(value: &str, byte_index: usize) -> usize {
    value
        .char_indices()
        .take_while(|(index, _)| *index < byte_index)
        .count()
}

fn char_to_byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn content_snippet_for_match(
    line: &str,
    match_start: usize,
    match_end: usize,
) -> (String, usize, usize) {
    let line = line.trim_end_matches(['\r', '\n']);
    let total_chars = line.chars().count();

    if total_chars <= CONTENT_SNIPPET_MAX_CHARS {
        return (
            line.to_string(),
            match_start.min(line.len()),
            match_end.min(line.len()),
        );
    }

    let match_start_char = byte_to_char_index(line, match_start.min(line.len()));
    let match_end_char = byte_to_char_index(line, match_end.min(line.len()));
    let mut start_char = match_start_char.saturating_sub(CONTENT_SNIPPET_CONTEXT_CHARS);
    let mut end_char = (match_end_char + CONTENT_SNIPPET_CONTEXT_CHARS).min(total_chars);

    if end_char.saturating_sub(start_char) > CONTENT_SNIPPET_MAX_CHARS {
        end_char = (start_char + CONTENT_SNIPPET_MAX_CHARS).min(total_chars);
    }

    if end_char.saturating_sub(start_char) < CONTENT_SNIPPET_MAX_CHARS && end_char < total_chars {
        start_char = end_char.saturating_sub(CONTENT_SNIPPET_MAX_CHARS);
    }

    let start_byte = char_to_byte_index(line, start_char);
    let end_byte = char_to_byte_index(line, end_char);
    let has_prefix = start_char > 0;
    let has_suffix = end_char < total_chars;
    let mut snippet = String::new();

    if has_prefix {
        snippet.push_str("... ");
    }

    let prefix_len = snippet.len();
    snippet.push_str(&line[start_byte..end_byte]);

    if has_suffix {
        snippet.push_str(" ...");
    }

    let adjusted_start = prefix_len + match_start.saturating_sub(start_byte).min(snippet.len());
    let adjusted_end = prefix_len + match_end.saturating_sub(start_byte).min(snippet.len());

    (snippet, adjusted_start, adjusted_end.max(adjusted_start))
}

fn content_match_score(
    name: &str,
    parent_path: &str,
    line_number: usize,
    line: &str,
    match_start: usize,
    query: &str,
    options: &ContentSearchOptions,
) -> i64 {
    let mut score = 1_000_i64;
    let match_start_index = match_start;
    let line_number = i64::try_from(line_number).unwrap_or(i64::MAX);
    let match_start = i64::try_from(match_start).unwrap_or(i64::MAX);

    score += (160 - line_number.min(160)).max(0);
    score += (120 - match_start.min(120)).max(0);

    if !options.regex {
        let query = query.to_ascii_lowercase();
        let name_lower = name.to_ascii_lowercase();
        let parent_lower = parent_path.to_ascii_lowercase();

        if name_lower.contains(&query) {
            score += 420;
        }

        if parent_lower.contains(&query) {
            score += 120;
        }
    }

    if line
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .is_some_and(|(index, _)| index == match_start_index)
    {
        score += 20;
    }

    score
}

fn sort_and_truncate_content_results(results: &mut Vec<ContentSearchResult>, limit: usize) {
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.match_count.cmp(&a.match_count))
            .then_with(|| a.line_number.cmp(&b.line_number))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    results.truncate(limit);
}

fn maybe_trim_content_results(results: &mut Vec<ContentSearchResult>, limit: usize) {
    let buffer_limit = limit
        .saturating_mul(CONTENT_RESULT_BUFFER_MULTIPLIER)
        .max(limit);

    if results.len() <= buffer_limit {
        return;
    }

    let retain_limit = limit
        .saturating_mul(CONTENT_RESULT_RETAIN_MULTIPLIER)
        .max(limit);
    sort_and_truncate_content_results(results, retain_limit);
}

struct ContentMatchAccumulator {
    name: String,
    path: String,
    parent_path: String,
    best_result: Option<ContentSearchResult>,
    best_score: i64,
    match_count: usize,
}

impl ContentMatchAccumulator {
    fn new(name: String, path: String, parent_path: String) -> Self {
        Self {
            name,
            path,
            parent_path,
            best_result: None,
            best_score: i64::MIN,
            match_count: 0,
        }
    }

    fn push_line(
        &mut self,
        line_number: usize,
        line: &str,
        query: &str,
        options: &ContentSearchOptions,
        matcher: Option<&regex::Regex>,
    ) {
        let Some((match_start, match_end)) = find_content_line_match(line, query, options, matcher)
        else {
            return;
        };

        self.match_count += 1;
        let score = content_match_score(
            &self.name,
            &self.parent_path,
            line_number,
            line,
            match_start,
            query,
            options,
        );

        if score <= self.best_score {
            return;
        }

        let (line_text, snippet_match_start, snippet_match_end) =
            content_snippet_for_match(line, match_start, match_end);
        self.best_score = score;
        self.best_result = Some(ContentSearchResult {
            name: self.name.clone(),
            path: self.path.clone(),
            parent_path: self.parent_path.clone(),
            line_number,
            line_text,
            match_start: snippet_match_start,
            match_end: snippet_match_end,
            match_count: 1,
            score,
        });
    }

    fn finish(self) -> Option<ContentSearchResult> {
        self.best_result.map(|mut result| {
            result.match_count = self.match_count;
            result.score = result
                .score
                .saturating_add(i64::try_from(self.match_count.min(24)).unwrap_or(0) * 35);
            result
        })
    }
}

fn search_local_content(
    root: &str,
    query: &str,
    options: ContentSearchOptions,
    mut on_progress: impl FnMut(ContentSearchProgress),
    mut checkpoint: impl FnMut(Option<&Path>) -> FsResult<()>,
) -> FsResult<Vec<ContentSearchResult>> {
    let root_path = expand_local_search_root(root)?;
    let root_metadata = fs::metadata(&root_path)
        .map_err(|error| FsError::io("Unable to read search root", &root_path, error))?;

    if !root_metadata.is_dir() {
        return Err(FsError::new(
            "search_root_not_directory",
            "Search root must be a local folder.",
            Some(root_path.to_string_lossy().into_owned()),
        ));
    }

    let query = query.trim();

    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = options.limit.clamp(1, 500);
    let max_file_bytes = options.max_file_bytes.max(1024);
    let matcher = if options.regex {
        Some(
            RegexBuilder::new(query)
                .case_insensitive(!options.case_sensitive)
                .build()
                .map_err(|error| {
                    FsError::new(
                        "invalid_regex",
                        format!("Invalid search regex: {error}"),
                        None,
                    )
                })?,
        )
    } else {
        None
    };
    let builder = configure_walk_builder(
        &root_path,
        options.include_hidden,
        options.respect_ignore,
        false,
        options.max_depth,
    );
    let mut results = Vec::new();
    let mut scanned_files = 0_u64;
    let mut matched_files = 0_u64;
    let mut last_progress_emit = Instant::now()
        .checked_sub(CONTENT_PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    emit_content_scan_progress(
        &mut on_progress,
        &mut last_progress_emit,
        scanned_files,
        matched_files,
        None,
        true,
    );

    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();

        checkpoint(Some(path))?;

        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        if !metadata.is_file() || metadata.len() > max_file_bytes {
            continue;
        }

        scanned_files = scanned_files.saturating_add(1);
        emit_content_scan_progress(
            &mut on_progress,
            &mut last_progress_emit,
            scanned_files,
            matched_files,
            Some(path),
            false,
        );

        let result = match lower_extension(path).as_deref() {
            Some("pdf") | Some("docx") | Some("docm") | Some("dotx") | Some("dotm")
            | Some("xlsx") | Some("xlsm") | Some("xltx") | Some("xltm") | Some("pptx")
            | Some("pptm") | Some("potx") | Some("potm") | Some("ppsx") | Some("ppsm")
            | Some("odt") | Some("ods") | Some("odp") => {
                search_extracted_content_file(&root_path, path, query, &options, matcher.as_ref())
            }
            _ => search_plain_text_file(
                &root_path,
                path,
                query,
                &options,
                matcher.as_ref(),
                &mut checkpoint,
            ),
        };

        match result {
            Ok(Some(result)) => {
                matched_files = matched_files.saturating_add(1);
                results.push(result);
                maybe_trim_content_results(&mut results, limit);
            }
            Ok(None) => {}
            Err(error) if error.code == "operation_cancelled" => return Err(error),
            Err(_) => {}
        }
    }

    emit_content_scan_progress(
        &mut on_progress,
        &mut last_progress_emit,
        scanned_files,
        matched_files,
        None,
        true,
    );
    sort_and_truncate_content_results(&mut results, limit);
    Ok(results)
}

fn emit_content_scan_progress(
    on_progress: &mut impl FnMut(ContentSearchProgress),
    last_emit: &mut Instant,
    scanned_files: u64,
    matched_files: u64,
    current_path: Option<&Path>,
    force: bool,
) {
    if !force && last_emit.elapsed() < CONTENT_PROGRESS_INTERVAL {
        return;
    }

    *last_emit = Instant::now();
    on_progress(ContentSearchProgress {
        scanned_files,
        matched_files,
        current_path: current_path.map(|path| path.to_string_lossy().into_owned()),
    });
}

fn search_extracted_content_file(
    root_path: &Path,
    path: &Path,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
) -> FsResult<Option<ContentSearchResult>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };

    let Some(content) = searchable_content_for_path(path, &bytes) else {
        return Ok(None);
    };

    Ok(best_content_file_match(
        root_path,
        path,
        content.as_ref(),
        query,
        options,
        matcher,
    ))
}

fn search_plain_text_file(
    root_path: &Path,
    path: &Path,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
    checkpoint: &mut impl FnMut(Option<&Path>) -> FsResult<()>,
) -> FsResult<Option<ContentSearchResult>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut probe = [0_u8; 8192];
    let bytes_read = file
        .read(&mut probe)
        .map_err(|error| FsError::io("Unable to read search candidate", path, error))?;

    if is_probably_binary(&probe[..bytes_read]) {
        return Ok(None);
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|error| FsError::io("Unable to read search candidate", path, error))?;

    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();
    let path_string = path.to_string_lossy().into_owned();
    let parent_path = path
        .parent()
        .unwrap_or(root_path)
        .to_string_lossy()
        .into_owned();
    let mut accumulator = ContentMatchAccumulator::new(name, path_string, parent_path);
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut line_number = 0_usize;

    loop {
        checkpoint(Some(path))?;
        buffer.clear();

        let bytes_read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| FsError::io("Unable to read search candidate", path, error))?;

        if bytes_read == 0 {
            break;
        }

        line_number += 1;
        let line = String::from_utf8_lossy(&buffer);
        accumulator.push_line(line_number, &line, query, options, matcher);
    }

    Ok(accumulator.finish())
}

fn best_content_file_match(
    root_path: &Path,
    path: &Path,
    content: &str,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
) -> Option<ContentSearchResult> {
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();
    let path_string = path.to_string_lossy().into_owned();
    let parent_path = path
        .parent()
        .unwrap_or(root_path)
        .to_string_lossy()
        .into_owned();
    let mut accumulator = ContentMatchAccumulator::new(name, path_string, parent_path);

    for (line_index, line) in content.lines().enumerate() {
        accumulator.push_line(line_index + 1, line, query, options, matcher);
    }

    accumulator.finish()
}

async fn search_remote_content(
    app: &AppHandle,
    operation_state: &FileOperationState,
    job_id: &Option<String>,
    remotes: &RemoteVolumeState,
    root: RemotePath,
    query: &str,
    options: ContentSearchOptions,
) -> FsResult<Vec<ContentSearchResult>> {
    let query = query.trim();

    if query.is_empty() {
        return Ok(Vec::new());
    }

    let limit = options.limit.clamp(1, 500);
    let max_file_bytes = options.max_file_bytes.max(1024);
    let matcher = if options.regex {
        Some(
            RegexBuilder::new(query)
                .case_insensitive(!options.case_sensitive)
                .build()
                .map_err(|error| {
                    FsError::new(
                        "invalid_regex",
                        format!("Invalid search regex: {error}"),
                        None,
                    )
                })?,
        )
    } else {
        None
    };
    let mut results = Vec::new();
    let mut stack = vec![(root, 0_usize)];
    let mut scanned_files = 0_u64;
    let mut matched_files = 0_u64;
    let mut last_progress_emit = Instant::now()
        .checked_sub(CONTENT_PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    emit_content_search_progress(
        app,
        job_id,
        ContentSearchProgress {
            scanned_files,
            matched_files,
            current_path: None,
        },
    );

    while let Some((directory, depth)) = stack.pop() {
        operation_state.checkpoint(job_id, None)?;

        let entries = list_remote_directory(remotes, directory).await?;

        for entry in entries {
            operation_state.checkpoint(job_id, None)?;

            if !options.include_hidden && entry.is_hidden {
                continue;
            }

            if entry.kind == FileEntryKind::Directory {
                if should_descend_remote(depth, options.max_depth) {
                    if let Some(remote_path) = parse_remote_path(&entry.path) {
                        stack.push((remote_path, depth + 1));
                    }
                }
                continue;
            }

            if entry.kind != FileEntryKind::File || entry.size.unwrap_or(0) > max_file_bytes {
                continue;
            }

            scanned_files = scanned_files.saturating_add(1);
            if last_progress_emit.elapsed() >= CONTENT_PROGRESS_INTERVAL {
                last_progress_emit = Instant::now();
                emit_content_search_progress(
                    app,
                    job_id,
                    ContentSearchProgress {
                        scanned_files,
                        matched_files,
                        current_path: Some(entry.path.clone()),
                    },
                );
            }

            let Some(remote_path) = parse_remote_path(&entry.path) else {
                continue;
            };
            let preview = match read_remote_file_prefix(remotes, remote_path, max_file_bytes).await
            {
                Ok(preview) if !preview.truncated => preview,
                _ => continue,
            };
            let Some(content) = searchable_content_for_path(Path::new(&entry.name), &preview.bytes)
            else {
                continue;
            };

            if let Some(result) = best_remote_content_file_match(
                &entry,
                content.as_ref(),
                query,
                &options,
                matcher.as_ref(),
            ) {
                matched_files = matched_files.saturating_add(1);
                results.push(result);
                maybe_trim_content_results(&mut results, limit);
            }
        }
    }

    emit_content_search_progress(
        app,
        job_id,
        ContentSearchProgress {
            scanned_files,
            matched_files,
            current_path: None,
        },
    );
    sort_and_truncate_content_results(&mut results, limit);
    Ok(results)
}

fn find_content_line_match(
    line: &str,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
) -> Option<(usize, usize)> {
    if let Some(regex) = matcher {
        regex
            .find(line)
            .map(|match_| (match_.start(), match_.end()))
    } else {
        find_plain_match(line, query, options.case_sensitive)
    }
}

fn best_remote_content_file_match(
    entry: &FileEntry,
    content: &str,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
) -> Option<ContentSearchResult> {
    let mut accumulator = ContentMatchAccumulator::new(
        entry.name.clone(),
        entry.path.clone(),
        parent_path_for_remote_uri(&entry.path),
    );

    for (line_index, line) in content.lines().enumerate() {
        accumulator.push_line(line_index + 1, line, query, options, matcher);
    }

    accumulator.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_local_content_for_test(
        root: &Path,
        query: &str,
        options: ContentSearchOptions,
    ) -> FsResult<Vec<ContentSearchResult>> {
        search_local_content(root.to_str().unwrap(), query, options, |_| {}, |_| Ok(()))
    }

    fn office_zip_bytes(parts: &[(&str, &str)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for (name, content) in parts {
            writer.start_file(*name, options).expect("start zip file");
            writer
                .write_all(content.as_bytes())
                .expect("write zip file");
        }

        writer.finish().expect("finish zip").into_inner()
    }

    fn simple_pdf_bytes(text: &str) -> Vec<u8> {
        let escaped_text = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = format!("BT /F1 12 Tf 72 720 Td ({escaped_text}) Tj ET\n");
        let objects = [
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n".to_string(),
            format!(
                "4 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
                stream.len(),
                stream
            ),
            "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
                .to_string(),
        ];
        let mut pdf = Vec::new();
        let mut offsets = Vec::new();

        pdf.extend_from_slice(b"%PDF-1.4\n");

        for object in objects {
            offsets.push(pdf.len());
            pdf.extend_from_slice(object.as_bytes());
        }

        let xref_start = pdf.len();
        write!(
            &mut pdf,
            "xref\n0 {}\n0000000000 65535 f \n",
            offsets.len() + 1
        )
        .expect("write xref header");

        for offset in &offsets {
            write!(&mut pdf, "{offset:010} 00000 n \n").expect("write xref entry");
        }

        write!(
            &mut pdf,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF\n",
            offsets.len() + 1
        )
        .expect("write trailer");

        pdf
    }

    #[test]
    fn file_search_result_marks_filename_match_indices() {
        let candidate = FileSearchCandidate {
            name: "CommandPalette.vue".to_string(),
            path: "/tmp/CommandPalette.vue".to_string(),
            parent_path: "/tmp".to_string(),
            kind: "file".to_string(),
            candidate: "src/components/CommandPalette.vue".to_string(),
            size: Some(42),
            modified_at: Some(7),
        };
        let pattern = Pattern::new(
            "cp",
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut haystack_buf = Vec::new();
        let mut indices_buf = Vec::new();

        let result = file_search_result_from_candidate(
            &candidate,
            "cp",
            &pattern,
            &mut matcher,
            &mut haystack_buf,
            &mut indices_buf,
        )
        .expect("candidate should fuzzy match");

        assert_eq!(result.name, "CommandPalette.vue");
        assert!(!result.match_indices.is_empty());
        assert!(result
            .match_indices
            .iter()
            .all(|index| *index < result.name.chars().count()));
    }

    #[test]
    fn file_search_top_results_keep_best_late_match() {
        let mut results = (0..20)
            .map(|index| FileSearchResult {
                name: format!("low-{index}.txt"),
                path: format!("/tmp/low-{index}.txt"),
                parent_path: "/tmp".to_string(),
                kind: "file".to_string(),
                score: 10,
                match_indices: Vec::new(),
                size: None,
                modified_at: None,
            })
            .collect::<Vec<_>>();

        results.push(FileSearchResult {
            name: "needle.txt".to_string(),
            path: "/tmp/needle.txt".to_string(),
            parent_path: "/tmp".to_string(),
            kind: "file".to_string(),
            score: 999,
            match_indices: vec![0, 1, 2],
            size: None,
            modified_at: None,
        });

        sort_and_truncate_file_results(&mut results, 1);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "needle.txt");
    }

    #[test]
    fn extracts_office_text_from_docx_zip() {
        let bytes = office_zip_bytes(&[(
            "word/document.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
              <w:body>
                <w:p><w:r><w:t>Quarterly &amp; budget</w:t></w:r></w:p>
                <w:p><w:r><w:t>Carelo roadmap</w:t></w:r></w:p>
                <w:p><w:r><w:t>R&amp;D planning</w:t></w:r></w:p>
              </w:body>
            </w:document>"#,
        )]);

        let text = extract_office_document_text(&bytes, "docx").expect("extract docx text");

        assert!(text.contains("Quarterly & budget"), "{text:?}");
        assert!(text.contains("Carelo roadmap"), "{text:?}");
        assert!(text.contains("R&D planning"), "{text:?}");
        assert!(text.contains('\n'));
    }

    #[test]
    fn content_search_matches_office_documents() {
        let root = std::env::temp_dir().join(format!("carelo-search-{}", random_token(10)));
        let path = root.join("proposal.docx");
        let bytes = office_zip_bytes(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Needle in an office document</w:t></w:r></w:p></w:body></w:document>"#,
        )]);

        fs::create_dir_all(&root).expect("create search root");
        fs::write(&path, bytes).expect("write docx");

        let mut options = default_content_search_options();
        options.limit = 10;
        options.max_file_bytes = 1024 * 1024;

        let results = search_local_content_for_test(&root, "office document", options).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "proposal.docx");
        assert!(results[0]
            .line_text
            .contains("Needle in an office document"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_search_returns_one_result_per_file() {
        let root = std::env::temp_dir().join(format!("carelo-dedupe-search-{}", random_token(10)));
        let repeated_path = root.join("repeated.txt");
        let single_path = root.join("single.txt");

        fs::create_dir_all(&root).expect("create search root");
        fs::write(
            &repeated_path,
            "Needle on the first line\nquiet line\nNeedle on another line",
        )
        .expect("write repeated file");
        fs::write(&single_path, "Needle once").expect("write single file");

        let mut options = default_content_search_options();
        options.limit = 10;

        let results = search_local_content_for_test(&root, "needle", options)
            .expect("search repeated matches");

        let repeated = results
            .iter()
            .find(|result| result.name == "repeated.txt")
            .expect("repeated file result");
        let single = results
            .iter()
            .find(|result| result.name == "single.txt")
            .expect("single file result");

        assert_eq!(results.len(), 2);
        assert_eq!(repeated.match_count, 2);
        assert_eq!(repeated.line_number, 1);
        assert_eq!(single.match_count, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_search_streams_plain_text_files() {
        let root = std::env::temp_dir().join(format!("carelo-stream-search-{}", random_token(10)));
        let path = root.join("large.txt");
        let mut content = "quiet line\n".repeat(20_000);
        content.push_str("Needle after a large prefix\n");

        fs::create_dir_all(&root).expect("create search root");
        fs::write(&path, content).expect("write large text file");

        let mut options = default_content_search_options();
        options.limit = 10;

        let results =
            search_local_content_for_test(&root, "needle", options).expect("search large text");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "large.txt");
        assert_eq!(results[0].line_number, 20_001);
        assert!(results[0].line_text.contains("Needle after a large prefix"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_search_ranks_filename_matches_and_trims_snippets() {
        let root = std::env::temp_dir().join(format!("carelo-ranked-search-{}", random_token(10)));
        let long_path = root.join("alpha.txt");
        let named_path = root.join("needle-report.txt");
        let long_line = format!(
            "{}Needle appears after a long prefix{}",
            "context ".repeat(45),
            " trailing".repeat(45),
        );

        fs::create_dir_all(&root).expect("create search root");
        fs::write(&long_path, long_line).expect("write long text file");
        fs::write(
            &named_path,
            "overview\nNeedle appears in a filename-ranked document\n",
        )
        .expect("write named text file");

        let mut options = default_content_search_options();
        options.limit = 10;

        let results =
            search_local_content_for_test(&root, "needle", options).expect("search ranked text");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "needle-report.txt");

        let long_result = results
            .iter()
            .find(|result| result.name == "alpha.txt")
            .expect("long-line result");
        assert!(long_result
            .line_text
            .contains("Needle appears after a long prefix"));
        assert!(long_result.line_text.starts_with("... "));
        assert!(long_result.line_text.ends_with(" ..."));
        assert!(long_result.line_text.chars().count() <= CONTENT_SNIPPET_MAX_CHARS + 8);
        assert!(long_result.match_end > long_result.match_start);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_search_honors_checkpoint_cancellation() {
        let root = std::env::temp_dir().join(format!("carelo-cancel-search-{}", random_token(10)));
        let path = root.join("cancel.txt");

        fs::create_dir_all(&root).expect("create search root");
        fs::write(&path, "Needle").expect("write text file");

        let mut options = default_content_search_options();
        options.limit = 10;

        let error = search_local_content(
            root.to_str().unwrap(),
            "needle",
            options,
            |_| {},
            |_| {
                Err(FsError::new(
                    "operation_cancelled",
                    "The file operation was cancelled.",
                    None,
                ))
            },
        )
        .expect_err("search should be cancelled");

        assert_eq!(error.code, "operation_cancelled");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_excel_and_powerpoint_text_parts() {
        let spreadsheet = office_zip_bytes(&[(
            "xl/sharedStrings.xml",
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Budget forecast</t></si></sst>"#,
        )]);
        let presentation = office_zip_bytes(&[(
            "ppt/slides/slide1.xml",
            r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Launch plan</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
        )]);

        let spreadsheet_text =
            extract_office_document_text(&spreadsheet, "xlsx").expect("extract xlsx text");
        let presentation_text =
            extract_office_document_text(&presentation, "pptx").expect("extract pptx text");

        assert!(spreadsheet_text.contains("Budget forecast"));
        assert!(presentation_text.contains("Launch plan"));
    }

    #[test]
    fn extracts_pdf_text_from_pdf_bytes() {
        let bytes = simple_pdf_bytes("Carelo PDF Needle");
        let text = extract_pdf_search_text(&bytes).expect("extract pdf text");

        assert!(text.contains("Carelo PDF Needle"));
    }

    #[test]
    fn content_search_matches_pdf_documents() {
        let root = std::env::temp_dir().join(format!("carelo-pdf-search-{}", random_token(10)));
        let path = root.join("invoice.pdf");

        fs::create_dir_all(&root).expect("create search root");
        fs::write(&path, simple_pdf_bytes("Carelo PDF Invoice Needle")).expect("write pdf");

        let mut options = default_content_search_options();
        options.limit = 10;
        options.max_file_bytes = 1024 * 1024;

        let results = search_local_content_for_test(&root, "invoice needle", options)
            .expect("search pdf content");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "invoice.pdf");
        assert!(results[0].line_text.contains("Carelo PDF Invoice Needle"));

        let _ = fs::remove_dir_all(root);
    }
}
