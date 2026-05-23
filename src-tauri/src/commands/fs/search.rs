use super::*;

#[tauri::command]
pub async fn search_files(
    root: String,
    query: String,
    options: Option<FileSearchOptions>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<FileSearchResult>, FsError> {
    if let Some(remote_root) = parse_remote_path(&root) {
        return search_remote_files(
            &remotes,
            remote_root,
            &query,
            options.unwrap_or_else(default_search_options),
        )
        .await;
    }

    run_local(move |_| {
        search_local_files(
            &root,
            &query,
            options.unwrap_or_else(default_search_options),
        )
    })
    .await
}

#[tauri::command]
pub async fn search_content(
    root: String,
    query: String,
    options: Option<ContentSearchOptions>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<ContentSearchResult>, FsError> {
    if let Some(remote_root) = parse_remote_path(&root) {
        return search_remote_content(
            &remotes,
            remote_root,
            &query,
            options.unwrap_or_else(default_content_search_options),
        )
        .await;
    }

    run_local(move |_| {
        search_local_content(
            &root,
            &query,
            options.unwrap_or_else(default_content_search_options),
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

fn search_local_files(
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

    let limit = options.limit.clamp(1, 500);
    let query = query.trim();
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

    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();

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

        let candidate = path
            .strip_prefix(&root_path)
            .unwrap_or(path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let score = if query.is_empty() {
            0
        } else if let Some(score) = pattern.score(
            Utf32Str::new(candidate.as_str(), &mut haystack_buf),
            &mut matcher,
        ) {
            score
        } else {
            continue;
        };
        let name = path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy()
            .into_owned();
        let parent_path = path
            .parent()
            .unwrap_or(&root_path)
            .to_string_lossy()
            .into_owned();

        results.push(FileSearchResult {
            name,
            path: path.to_string_lossy().into_owned(),
            parent_path,
            kind: kind.to_string(),
            score: i64::from(score),
            size: metadata.is_file().then_some(metadata.len()),
            modified_at: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs()),
        });
    }

    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    results.truncate(limit);
    Ok(results)
}

async fn search_remote_files(
    remotes: &RemoteVolumeState,
    root: RemotePath,
    query: &str,
    options: FileSearchOptions,
) -> FsResult<Vec<FileSearchResult>> {
    let limit = options.limit.clamp(1, 500);
    let query = query.trim();
    let root_uri = crate::fs::remote::format_remote_uri(&root.volume_id, &root.path);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut results = Vec::new();
    let mut haystack_buf = Vec::new();
    let mut stack = vec![(root, 0_usize)];

    while let Some((directory, depth)) = stack.pop() {
        if results.len() >= limit {
            break;
        }

        let entries = list_remote_directory(remotes, directory.clone()).await?;

        for entry in entries {
            if results.len() >= limit {
                break;
            }

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

            let candidate = remote_search_candidate(&root_uri, &entry.path);
            let score = if query.is_empty() {
                0
            } else if let Some(score) = pattern.score(
                Utf32Str::new(candidate.as_str(), &mut haystack_buf),
                &mut matcher,
            ) {
                score
            } else {
                if is_directory && should_descend_remote(depth, options.max_depth) {
                    if let Some(remote_path) = parse_remote_path(&entry.path) {
                        stack.push((remote_path, depth + 1));
                    }
                }
                continue;
            };

            results.push(FileSearchResult {
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
                score: i64::from(score),
                size: entry.size,
                modified_at: entry.modified_at,
            });

            if is_directory && should_descend_remote(depth, options.max_depth) {
                if let Some(remote_path) = parse_remote_path(&entry.path) {
                    stack.push((remote_path, depth + 1));
                }
            }
        }
    }

    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    results.truncate(limit);
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

fn line_text_with_limit(line: &str) -> String {
    const MAX_CHARS: usize = 500;

    if line.chars().count() <= MAX_CHARS {
        return line.to_string();
    }

    line.chars().take(MAX_CHARS).collect::<String>()
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

fn search_local_content(
    root: &str,
    query: &str,
    options: ContentSearchOptions,
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

    for entry in builder.build() {
        if results.len() >= limit {
            break;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        if !metadata.is_file() || metadata.len() > max_file_bytes {
            continue;
        }

        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };

        let Some(content) = searchable_content_for_path(path, &bytes) else {
            continue;
        };

        push_content_file_match(
            &mut results,
            limit,
            &root_path,
            path,
            content.as_ref(),
            query,
            &options,
            matcher.as_ref(),
        );
    }

    Ok(results)
}

async fn search_remote_content(
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

    while let Some((directory, depth)) = stack.pop() {
        if results.len() >= limit {
            break;
        }

        let entries = list_remote_directory(remotes, directory).await?;

        for entry in entries {
            if results.len() >= limit {
                break;
            }

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

            push_remote_content_file_match(
                &mut results,
                limit,
                &entry,
                content.as_ref(),
                query,
                &options,
                matcher.as_ref(),
            );
        }
    }

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

fn push_content_file_match(
    results: &mut Vec<ContentSearchResult>,
    limit: usize,
    root_path: &Path,
    path: &Path,
    content: &str,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
) {
    if results.len() >= limit {
        return;
    }

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
    let mut first_result = None;
    let mut match_count = 0;

    for (line_index, line) in content.lines().enumerate() {
        let Some((match_start, match_end)) = find_content_line_match(line, query, options, matcher)
        else {
            continue;
        };

        match_count += 1;

        if first_result.is_none() {
            first_result = Some(ContentSearchResult {
                name: name.clone(),
                path: path_string.clone(),
                parent_path: parent_path.clone(),
                line_number: line_index + 1,
                line_text: line_text_with_limit(line),
                match_start,
                match_end,
                match_count: 1,
            });
        }
    }

    if let Some(mut result) = first_result {
        result.match_count = match_count;
        results.push(result);
    }
}

fn push_remote_content_file_match(
    results: &mut Vec<ContentSearchResult>,
    limit: usize,
    entry: &FileEntry,
    content: &str,
    query: &str,
    options: &ContentSearchOptions,
    matcher: Option<&regex::Regex>,
) {
    if results.len() >= limit {
        return;
    }

    let mut first_result = None;
    let mut match_count = 0;

    for (line_index, line) in content.lines().enumerate() {
        let Some((match_start, match_end)) = find_content_line_match(line, query, options, matcher)
        else {
            continue;
        };

        match_count += 1;

        if first_result.is_none() {
            first_result = Some(ContentSearchResult {
                name: entry.name.clone(),
                path: entry.path.clone(),
                parent_path: parent_path_for_remote_uri(&entry.path),
                line_number: line_index + 1,
                line_text: line_text_with_limit(line),
                match_start,
                match_end,
                match_count: 1,
            });
        }
    }

    if let Some(mut result) = first_result {
        result.match_count = match_count;
        results.push(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let results =
            search_local_content(root.to_str().unwrap(), "office document", options).unwrap();

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

        let results = search_local_content(root.to_str().unwrap(), "needle", options)
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

        let results = search_local_content(root.to_str().unwrap(), "invoice needle", options)
            .expect("search pdf content");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "invoice.pdf");
        assert!(results[0].line_text.contains("Carelo PDF Invoice Needle"));

        let _ = fs::remove_dir_all(root);
    }
}
