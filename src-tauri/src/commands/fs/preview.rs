use super::*;

#[derive(Clone)]
struct MediaStreamServer {
    port: u16,
    token: String,
}

#[derive(Clone)]
struct MediaStreamEntry {
    path: PathBuf,
    remote_id: Option<String>,
    cleanup_root: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct MediaStreamState {
    server: Arc<Mutex<Option<MediaStreamServer>>>,
    entries: Arc<Mutex<HashMap<String, MediaStreamEntry>>>,
    entry_order: Arc<Mutex<VecDeque<String>>>,
}

impl MediaStreamState {
    fn stream_url_for(&self, path: PathBuf) -> FsResult<String> {
        self.stream_url_for_entry(MediaStreamEntry {
            path,
            remote_id: None,
            cleanup_root: None,
        })
    }

    fn stream_url_for_remote(&self, remote_id: String, path: PathBuf) -> FsResult<String> {
        let cleanup_root = path.parent().map(Path::to_path_buf);

        self.stream_url_for_entry(MediaStreamEntry {
            path,
            remote_id: Some(remote_id),
            cleanup_root,
        })
    }

    fn stream_url_for_entry(&self, entry: MediaStreamEntry) -> FsResult<String> {
        let metadata = fs::metadata(&entry.path)
            .map_err(|error| FsError::io("Unable to read media metadata", &entry.path, error))?;

        if !metadata.is_file() {
            return Err(FsError::new(
                "media_stream_not_file",
                "Media preview is available for files only.",
                Some(entry.path.to_string_lossy().into_owned()),
            ));
        }

        let server = self.ensure_server()?;
        let id = random_token(32);

        self.entries
            .lock()
            .map_err(|_| media_stream_error("Unable to register media stream."))?
            .insert(id.clone(), entry.clone());
        self.prune_entries(&id)?;

        Ok(format!(
            "http://127.0.0.1:{}/media/{}/{}/preview{}",
            server.port,
            server.token,
            id,
            media_url_extension(&entry.path),
        ))
    }

    fn ensure_server(&self) -> FsResult<MediaStreamServer> {
        let mut server = self
            .server
            .lock()
            .map_err(|_| media_stream_error("Unable to start media stream server."))?;

        if let Some(server) = server.clone() {
            return Ok(server);
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
            FsError::new(
                "media_stream_server_unavailable",
                format!("Unable to start media stream server: {error}"),
                None,
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                FsError::new(
                    "media_stream_server_unavailable",
                    format!("Unable to read media stream server address: {error}"),
                    None,
                )
            })?
            .port();
        let token = random_token(32);
        let entries = self.entries.clone();
        let server_token = token.clone();

        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let entries = entries.clone();
                let token = server_token.clone();

                thread::spawn(move || {
                    handle_media_stream_request(stream, entries, token);
                });
            }
        });

        let created = MediaStreamServer { port, token };
        *server = Some(created.clone());
        Ok(created)
    }

    fn prune_entries(&self, id: &str) -> FsResult<()> {
        let mut order = self
            .entry_order
            .lock()
            .map_err(|_| media_stream_error("Unable to prune media stream entries."))?;

        order.push_back(id.to_string());

        while order.len() > MEDIA_STREAM_MAX_ENTRIES {
            if let Some(expired_id) = order.pop_front() {
                if let Ok(mut entries) = self.entries.lock() {
                    if let Some(entry) = entries.remove(&expired_id) {
                        cleanup_media_stream_entry(entry);
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn release_remote_entries(&self, remote_ids: &HashSet<String>) -> FsResult<usize> {
        if remote_ids.is_empty() {
            return Ok(0);
        }

        let (expired_ids, removed) = {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| media_stream_error("Unable to release media stream entries."))?;
            let expired_ids = entries
                .iter()
                .filter_map(|(id, entry)| {
                    entry
                        .remote_id
                        .as_ref()
                        .filter(|remote_id| remote_ids.contains(*remote_id))
                        .map(|_| id.clone())
                })
                .collect::<Vec<_>>();
            let removed = expired_ids
                .iter()
                .filter_map(|id| entries.remove(id))
                .collect::<Vec<_>>();

            (expired_ids, removed)
        };

        if !expired_ids.is_empty() {
            let mut order = self
                .entry_order
                .lock()
                .map_err(|_| media_stream_error("Unable to release media stream entries."))?;
            let expired = expired_ids.into_iter().collect::<HashSet<_>>();
            order.retain(|id| !expired.contains(id));
        }

        let removed_count = removed.len();

        for entry in removed {
            cleanup_media_stream_entry(entry);
        }

        Ok(removed_count)
    }
}

#[tauri::command]
pub async fn read_text_preview(
    path: String,
    max_bytes: Option<usize>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<TextPreview, FsError> {
    if let Some(remote_path) = parse_remote_path(&path) {
        let byte_limit = max_bytes.unwrap_or(96 * 1024).clamp(4 * 1024, 512 * 1024);
        let preview = read_remote_file_prefix(&remotes, remote_path, byte_limit as u64).await?;

        if is_probably_binary(&preview.bytes) {
            return Err(FsError::new(
                "preview_binary_file",
                "This file appears to be binary.",
                Some(path),
            ));
        }

        return Ok(TextPreview {
            text: String::from_utf8_lossy(&preview.bytes).into_owned(),
            truncated: preview.truncated,
            bytes_read: preview.bytes.len(),
        });
    }

    run_local(move |_| read_local_text_preview(&path, max_bytes.unwrap_or(96 * 1024))).await
}

#[tauri::command]
pub async fn read_media_preview(
    path: String,
    max_bytes: Option<u64>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<tauri::ipc::Response, FsError> {
    if let Some(remote_path) = parse_remote_path(&path) {
        let byte_limit = max_bytes
            .unwrap_or(MEDIA_PREVIEW_MAX_BYTES)
            .clamp(1024 * 1024, MEDIA_PREVIEW_MAX_BYTES);
        let preview = read_remote_file_prefix(&remotes, remote_path, byte_limit).await?;

        if preview.total_bytes > byte_limit {
            return Err(FsError::new(
                "preview_file_too_large",
                "This media file is too large for inline preview.",
                Some(path),
            ));
        }

        return Ok(tauri::ipc::Response::new(preview.bytes));
    }

    let bytes = run_local(move |_| {
        read_local_media_preview(&path, max_bytes.unwrap_or(MEDIA_PREVIEW_MAX_BYTES))
    })
    .await?;

    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn create_media_stream_url(
    path: String,
    media_state: tauri::State<'_, MediaStreamState>,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<String, FsError> {
    if let Some(remote_path) = parse_remote_path(&path) {
        let remote_id = remote_path.volume_id.clone();
        let materialized_path = materialize_remote_file(&remotes, remote_path).await?;
        return media_state.stream_url_for_remote(remote_id, materialized_path);
    }

    let path = expand_local_path(&path)?;
    media_state.stream_url_for(path)
}

#[tauri::command]
pub async fn compare_file_checksums(
    left_path: String,
    right_path: String,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<FileChecksumComparison, FsError> {
    if archive::is_archive_uri(&left_path) || archive::is_archive_uri(&right_path) {
        return Err(FsError::new(
            "checksum_unsupported",
            "Checksum comparison is available for local and remote files only.",
            None,
        ));
    }

    let resolved_left = if let Some(remote_path) = parse_remote_path(&left_path) {
        materialize_remote_file(&remotes, remote_path)
            .await?
            .to_string_lossy()
            .into_owned()
    } else {
        left_path.clone()
    };
    let resolved_right = if let Some(remote_path) = parse_remote_path(&right_path) {
        materialize_remote_file(&remotes, remote_path)
            .await?
            .to_string_lossy()
            .into_owned()
    } else {
        right_path.clone()
    };

    let mut comparison =
        run_local(move |_| compare_local_file_checksums(&resolved_left, &resolved_right)).await?;
    comparison.left_path = left_path;
    comparison.right_path = right_path;
    Ok(comparison)
}

#[tauri::command]
pub async fn compute_file_checksum(
    path: String,
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<FileChecksum, FsError> {
    if archive::is_archive_uri(&path) {
        return Err(FsError::new(
            "checksum_unsupported",
            "Checksum computation is available for local and remote files only.",
            None,
        ));
    }

    let resolved = if let Some(remote_path) = parse_remote_path(&path) {
        materialize_remote_file(&remotes, remote_path)
            .await?
            .to_string_lossy()
            .into_owned()
    } else {
        path.clone()
    };

    let (hash, bytes) = run_local(move |_| file_sha256(&expand_local_path(&resolved)?)).await?;

    Ok(FileChecksum {
        algorithm: "SHA-256".to_string(),
        path,
        hash,
        bytes,
    })
}

fn read_local_text_preview(path: &str, max_bytes: usize) -> FsResult<TextPreview> {
    let path = expand_local_search_root(path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| FsError::io("Unable to read text preview metadata", &path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "preview_not_file",
            "Text preview is available for files only.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let byte_limit = max_bytes.clamp(4 * 1024, 512 * 1024);
    let bytes = fs::read(&path)
        .map_err(|error| FsError::io("Unable to read text preview", &path, error))?;
    let truncated = bytes.len() > byte_limit;
    let bytes = &bytes[..bytes.len().min(byte_limit)];

    if is_probably_binary(bytes) {
        return Err(FsError::new(
            "preview_binary_file",
            "This file appears to be binary.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(TextPreview {
        text: String::from_utf8_lossy(bytes).into_owned(),
        truncated,
        bytes_read: bytes.len(),
    })
}

fn compare_local_file_checksums(
    left_path: &str,
    right_path: &str,
) -> FsResult<FileChecksumComparison> {
    let left = expand_local_path(left_path)?;
    let right = expand_local_path(right_path)?;
    let (left_hash, left_bytes) = file_sha256(&left)?;
    let (right_hash, right_bytes) = file_sha256(&right)?;

    Ok(FileChecksumComparison {
        algorithm: "SHA-256".to_string(),
        left_path: left.to_string_lossy().into_owned(),
        right_path: right.to_string_lossy().into_owned(),
        equal: left_hash == right_hash,
        left_hash,
        right_hash,
        left_bytes,
        right_bytes,
    })
}

fn file_sha256(path: &Path) -> FsResult<(String, u64)> {
    let metadata = fs::metadata(path)
        .map_err(|error| FsError::io("Unable to read checksum metadata", path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "checksum_not_file",
            "Checksum comparison is available for files only.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let mut file = fs::File::open(path)
        .map_err(|error| FsError::io("Unable to read file checksum", path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes_read = 0_u64;

    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| FsError::io("Unable to read file checksum", path, error))?;

        if count == 0 {
            break;
        }

        hasher.update(&buffer[..count]);
        bytes_read = bytes_read.saturating_add(count as u64);
    }

    Ok((hex_string(&hasher.finalize()), bytes_read))
}

fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }

    output
}

fn read_local_media_preview(path: &str, max_bytes: u64) -> FsResult<Vec<u8>> {
    let path = expand_local_path(path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| FsError::io("Unable to read media preview metadata", &path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "preview_not_file",
            "Media preview is available for files only.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let byte_limit = max_bytes.clamp(1024 * 1024, MEDIA_PREVIEW_MAX_BYTES);

    if metadata.len() > byte_limit {
        return Err(FsError::new(
            "preview_file_too_large",
            "This media file is too large for inline preview.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    fs::read(&path).map_err(|error| FsError::io("Unable to read media preview", &path, error))
}

struct MediaStreamRequest {
    method: String,
    path: String,
    range: Option<String>,
}

fn handle_media_stream_request(
    mut stream: TcpStream,
    entries: Arc<Mutex<HashMap<String, MediaStreamEntry>>>,
    token: String,
) {
    let request = match read_media_stream_request(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_media_stream_error(&mut stream, "400 Bad Request", "Bad request");
            return;
        }
    };

    if request.method == "OPTIONS" {
        let _ = write_media_stream_options_response(&mut stream);
        return;
    }

    if request.method != "GET" && request.method != "HEAD" {
        let _ =
            write_media_stream_error(&mut stream, "405 Method Not Allowed", "Method not allowed");
        return;
    }

    let prefix = format!("/media/{token}/");
    let Some(rest) = request.path.strip_prefix(&prefix) else {
        let _ = write_media_stream_error(&mut stream, "404 Not Found", "Not found");
        return;
    };
    let id = rest.split('/').next().unwrap_or_default();

    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        let _ = write_media_stream_error(&mut stream, "404 Not Found", "Not found");
        return;
    }

    let entry = match entries
        .lock()
        .ok()
        .and_then(|entries| entries.get(id).cloned())
    {
        Some(entry) => entry,
        None => {
            let _ = write_media_stream_error(&mut stream, "404 Not Found", "Not found");
            return;
        }
    };
    let path = entry.path;

    if let Err(error) = write_media_stream_file(&mut stream, &request, &path) {
        eprintln!("Unable to stream media preview {}: {error}", path.display());
    }
}

fn read_media_stream_request(stream: &mut TcpStream) -> std::io::Result<MediaStreamRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];

    loop {
        let bytes_read = stream.read(&mut chunk)?;

        if bytes_read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);

        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 64 * 1024 {
            break;
        }
    }

    let request = String::from_utf8_lossy(&buffer);
    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let target = request_parts.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default().to_string();
    let mut range = None;

    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };

        if name.eq_ignore_ascii_case("range") {
            range = Some(value.trim().to_string());
        }
    }

    if method.is_empty() || path.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid media stream request",
        ));
    }

    Ok(MediaStreamRequest {
        method,
        path,
        range,
    })
}

fn write_media_stream_file(
    stream: &mut TcpStream,
    request: &MediaStreamRequest,
    path: &Path,
) -> std::io::Result<()> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let content_type = media_content_type(path);

    if len == 0 {
        write_media_stream_headers(stream, "200 OK", content_type, 0, None)?;
        return Ok(());
    }

    let range = request
        .range
        .as_deref()
        .and_then(|range| parse_media_range(range, len));

    let (status, start, end, content_range) = if let Some((start, end)) = range {
        (
            "206 Partial Content",
            start,
            end,
            Some(format!("bytes {start}-{end}/{len}")),
        )
    } else if request.range.is_some() {
        write_media_stream_range_error(stream, len)?;
        return Ok(());
    } else {
        ("200 OK", 0, len - 1, None)
    };
    let content_length = end - start + 1;

    write_media_stream_headers(
        stream,
        status,
        content_type,
        content_length,
        content_range.as_deref(),
    )?;

    if request.method == "HEAD" {
        return Ok(());
    }

    file.seek(SeekFrom::Start(start))?;
    stream_file_range(stream, &mut file, content_length)
}

fn parse_media_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let (unit, spec) = header.trim().split_once('=')?;

    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }

    let first_range = spec.split(',').next()?.trim();
    let (start, end) = first_range.split_once('-')?;

    if start.is_empty() {
        let suffix_length = end.parse::<u64>().ok()?;

        if suffix_length == 0 {
            return None;
        }

        let start = len.saturating_sub(suffix_length);
        return Some((start, len - 1));
    }

    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().ok()?.min(len - 1)
    };

    if start >= len || end < start {
        return None;
    }

    Some((start, end))
}

fn stream_file_range(
    stream: &mut TcpStream,
    file: &mut fs::File,
    mut remaining: u64,
) -> std::io::Result<()> {
    let mut buffer = [0_u8; 64 * 1024];

    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let bytes_read = file.read(&mut buffer[..limit])?;

        if bytes_read == 0 {
            break;
        }

        if let Err(error) = stream.write_all(&buffer[..bytes_read]) {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }

            return Err(error);
        }

        remaining -= bytes_read as u64;
    }

    Ok(())
}

fn write_media_stream_headers(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    content_length: u64,
    content_range: Option<&str>,
) -> std::io::Result<()> {
    stream.write_all(
        media_stream_header_response(status, content_type, content_length, content_range)
            .as_bytes(),
    )
}

fn media_stream_header_response(
    status: &str,
    content_type: &str,
    content_length: u64,
    content_range: Option<&str>,
) -> String {
    let content_range_header = content_range
        .map(|value| format!("Content-Range: {value}\r\n"))
        .unwrap_or_default();

    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {content_length}\r\n\
         Accept-Ranges: bytes\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Expose-Headers: Accept-Ranges, Content-Length, Content-Range\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         {content_range_header}\r\n",
    )
}

fn write_media_stream_range_error(stream: &mut TcpStream, len: u64) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 416 Range Not Satisfiable\r\n\
         Content-Length: 0\r\n\
         Content-Range: bytes */{len}\r\n\
         Accept-Ranges: bytes\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
    );

    stream.write_all(response.as_bytes())
}

fn write_media_stream_options_response(stream: &mut TcpStream) -> std::io::Result<()> {
    let response = concat!(
        "HTTP/1.1 204 No Content\r\n",
        "Content-Length: 0\r\n",
        "Access-Control-Allow-Origin: *\r\n",
        "Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n",
        "Access-Control-Allow-Headers: Range\r\n",
        "Access-Control-Max-Age: 86400\r\n",
        "Connection: close\r\n\r\n",
    );

    stream.write_all(response.as_bytes())
}

fn write_media_stream_error(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n\
         {body}",
        body.len(),
    );

    stream.write_all(response.as_bytes())
}

fn media_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("ogv") => "video/ogg",
        Some("mpeg" | "mpg") => "video/mpeg",
        Some("3gp") => "video/3gpp",
        Some("3g2") => "video/3gpp2",
        Some("avi") => "video/x-msvideo",
        Some("mkv") => "video/x-matroska",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "alac") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("oga" | "ogg" | "opus") => "audio/ogg",
        Some("weba") => "audio/webm",
        Some("aif" | "aiff") => "audio/aiff",
        Some("wma") => "audio/x-ms-wma",
        _ => "application/octet-stream",
    }
}

fn media_url_extension(path: &Path) -> String {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.to_ascii_lowercase())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 12
                && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
        })
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default()
}

fn cleanup_media_stream_entry(entry: MediaStreamEntry) {
    let Some(cleanup_root) = entry.cleanup_root else {
        return;
    };

    if !is_remote_media_cleanup_root(&cleanup_root) {
        return;
    }

    let _ = fs::remove_dir_all(cleanup_root);
}

fn is_remote_media_cleanup_root(path: &Path) -> bool {
    path.starts_with(remote_media_temp_base())
}

fn remote_media_temp_base() -> PathBuf {
    std::env::temp_dir().join("carelo-remote-open")
}

fn media_stream_error(message: &str) -> FsError {
    FsError::new("media_stream_error", message, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_media_ranges() {
        assert_eq!(parse_media_range("bytes=0-1023", 10_000), Some((0, 1023)));
        assert_eq!(parse_media_range("Bytes=500-", 10_000), Some((500, 9999)));
        assert_eq!(
            parse_media_range("bytes=500-20", 10_000),
            None,
            "end before start is not satisfiable"
        );
        assert_eq!(
            parse_media_range("items=0-10", 10_000),
            None,
            "only byte ranges are supported"
        );
    }

    #[test]
    fn parses_suffix_media_ranges() {
        assert_eq!(parse_media_range("bytes=-500", 10_000), Some((9500, 9999)));
        assert_eq!(parse_media_range("bytes=-15000", 10_000), Some((0, 9999)));
        assert_eq!(parse_media_range("bytes=-0", 10_000), None);
    }

    #[test]
    fn sanitizes_media_url_extension() {
        assert_eq!(
            media_url_extension(Path::new("/tmp/example.MP4")),
            ".mp4".to_string()
        );
        assert_eq!(
            media_url_extension(Path::new("/tmp/example.bad-ext")),
            String::new()
        );
    }

    #[test]
    fn media_stream_headers_are_valid_http_headers() {
        let headers = media_stream_header_response(
            "206 Partial Content",
            "video/mp4",
            1024,
            Some("bytes 0-1023/2048"),
        );

        assert!(headers.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(headers.contains("\r\nContent-Type: video/mp4\r\n"));
        assert!(headers.contains("\r\nContent-Range: bytes 0-1023/2048\r\n"));
        assert!(!headers.contains("\r\n Content-Type"));
        assert!(headers.ends_with("\r\n\r\n"));
    }

    #[test]
    fn releasing_remote_media_entries_removes_cached_files() {
        let root = remote_media_temp_base().join(format!("test-{}", random_token(10)));
        let file = root.join("movie.mp4");
        fs::create_dir_all(&root).expect("create remote media cache root");
        fs::write(&file, b"media").expect("write cached media");

        let state = MediaStreamState::default();
        state.entries.lock().expect("entries lock").insert(
            "entry".to_string(),
            MediaStreamEntry {
                path: file,
                remote_id: Some("server".to_string()),
                cleanup_root: Some(root.clone()),
            },
        );
        state
            .entry_order
            .lock()
            .expect("order lock")
            .push_back("entry".to_string());

        let remote_ids = HashSet::from(["server".to_string()]);

        assert_eq!(
            state
                .release_remote_entries(&remote_ids)
                .expect("release entries"),
            1
        );
        assert!(!root.exists());
        assert!(state.entries.lock().expect("entries lock").is_empty());
        assert!(state.entry_order.lock().expect("order lock").is_empty());
    }

    #[test]
    fn media_stream_server_serves_byte_ranges() {
        let path = std::env::temp_dir().join(format!("carelo-media-{}.mp4", random_token(10)));
        fs::write(&path, b"0123456789abcdef").expect("write test media file");

        let state = MediaStreamState::default();
        let url = match state.stream_url_for(path.clone()) {
            Ok(url) => url,
            Err(error)
                if error.code == "media_stream_server_unavailable"
                    && error.message.contains("Operation not permitted") =>
            {
                let _ = fs::remove_file(path);
                return;
            }
            Err(error) => panic!("create stream URL: {error:?}"),
        };
        let parsed = url::Url::parse(&url).expect("parse stream URL");
        let port = parsed.port().expect("stream URL includes port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to stream server");
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=4-7\r\nConnection: close\r\n\r\n",
            parsed.path(),
        );

        stream
            .write_all(request.as_bytes())
            .expect("write stream request");

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("read stream response");
        let response = String::from_utf8_lossy(&response);

        assert!(response.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(response.contains("\r\nContent-Type: video/mp4\r\n"));
        assert!(response.contains("\r\nContent-Range: bytes 4-7/16\r\n"));
        assert!(response.ends_with("4567"));

        let _ = fs::remove_file(path);
    }
}
