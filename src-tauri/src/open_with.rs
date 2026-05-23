use crate::fs::models::{FsError, FsResult};
use crate::store::OpenWithDefaultEntry;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTypeInfo {
    pub key: String,
    pub label: String,
    pub mime_type: String,
    pub extension: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithApp {
    pub id: String,
    pub name: String,
    pub description: String,
    pub desktop_file: String,
    pub is_system_default: bool,
    pub is_remembered_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithContext {
    pub file_type: FileTypeInfo,
    pub apps: Vec<OpenWithApp>,
    pub remembered_app_id: Option<String>,
    pub system_default_app_id: Option<String>,
}

#[derive(Debug, Clone)]
struct DesktopApp {
    id: String,
    name: String,
    exec: String,
    desktop_file: PathBuf,
    mime_types: Vec<String>,
    no_display: bool,
}

pub fn open_with_context(
    path: &Path,
    remembered: Option<OpenWithDefaultEntry>,
) -> FsResult<OpenWithContext> {
    let file_type = file_type_for_path(path);
    Ok(open_with_context_for_file_type(file_type, remembered))
}

pub fn open_with_context_for_file_type(
    file_type: FileTypeInfo,
    remembered: Option<OpenWithDefaultEntry>,
) -> OpenWithContext {
    let remembered_app_id = remembered.map(|entry| entry.app_id);
    let system_default_app_id = system_default_app_id(&file_type.mime_type);
    let apps = open_with_apps_for(
        &file_type,
        remembered_app_id.as_deref(),
        system_default_app_id.as_deref(),
    );

    OpenWithContext {
        file_type,
        apps,
        remembered_app_id,
        system_default_app_id,
    }
}

pub fn open_with_default(path: &Path, remembered: Option<OpenWithDefaultEntry>) -> FsResult<()> {
    if let Some(remembered) = remembered {
        if open_with_app_id(path, &remembered.app_id).is_ok() {
            return Ok(());
        }
    }

    open_with_system_default(path)
}

pub fn open_with_app_id(path: &Path, app_id: &str) -> FsResult<()> {
    let app_id = app_id.trim();

    if app_id.is_empty() {
        return Err(FsError::new(
            "open_with_missing_app",
            "Choose an app to open this file.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let Some(app) = desktop_apps()
        .into_iter()
        .find(|candidate| candidate.id == app_id)
    else {
        return Err(FsError::new(
            "open_with_app_not_found",
            "The selected app is no longer available.",
            Some(path.to_string_lossy().into_owned()),
        ));
    };

    open_with_desktop_app(path, &app)
}

pub fn file_type_for_path(path: &Path) -> FileTypeInfo {
    let extension = extension_for_path(path);
    let mime_type = query_mime_type(path)
        .or_else(|| extension.as_deref().and_then(mime_type_for_extension))
        .unwrap_or_else(|| "application/octet-stream".to_string());
    file_type_from_parts(extension, mime_type)
}

pub fn file_type_for_virtual_path(path: &str) -> FileTypeInfo {
    let extension = extension_for_path(Path::new(path));
    let mime_type = extension
        .as_deref()
        .and_then(mime_type_for_extension)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    file_type_from_parts(extension, mime_type)
}

fn file_type_from_parts(extension: Option<String>, mime_type: String) -> FileTypeInfo {
    let key = extension
        .as_ref()
        .map(|extension| format!("ext:{extension}"))
        .unwrap_or_else(|| format!("mime:{mime_type}"));
    let label = extension
        .as_ref()
        .map(|extension| format!(".{} files", extension))
        .unwrap_or_else(|| mime_type.clone());

    FileTypeInfo {
        key,
        label,
        mime_type,
        extension,
    }
}

fn extension_for_path(path: &Path) -> Option<String> {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn mime_type_for_extension(extension: &str) -> Option<String> {
    let mime_type = match extension {
        "txt" | "text" | "log" | "md" | "markdown" | "csv" | "tsv" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "json" | "map" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rs" | "go" | "py" | "rb" | "php" | "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp"
        | "ts" | "tsx" | "jsx" | "vue" | "svelte" | "sql" | "sh" | "zsh" | "bash" | "toml"
        | "yaml" | "yml" => "text/plain",
        _ => return None,
    };

    Some(mime_type.to_string())
}

fn open_with_system_default(path: &Path) -> FsResult<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|error| {
        FsError::new(
            "open_failed",
            format!("Unable to open item with the default app: {error}"),
            Some(path.to_string_lossy().into_owned()),
        )
    })
}

fn open_with_desktop_app(path: &Path, app: &DesktopApp) -> FsResult<()> {
    let tokens = tokenize_exec(&app.exec);
    let args = expand_exec_tokens(&tokens, path);
    let Some((program, rest)) = args.split_first() else {
        return Err(FsError::new(
            "open_with_invalid_app",
            "The selected app does not provide a valid launch command.",
            Some(app.desktop_file.to_string_lossy().into_owned()),
        ));
    };

    Command::new(program)
        .args(rest)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            FsError::new(
                "open_with_failed",
                format!("Unable to open with {}: {error}", app.name),
                Some(path.to_string_lossy().into_owned()),
            )
        })
}

fn open_with_apps_for(
    file_type: &FileTypeInfo,
    remembered_app_id: Option<&str>,
    system_default_app_id: Option<&str>,
) -> Vec<OpenWithApp> {
    let all_apps = desktop_apps();
    let by_id = all_apps
        .iter()
        .map(|app| (app.id.clone(), app))
        .collect::<HashMap<_, _>>();
    let mut candidates = all_apps
        .iter()
        .filter(|app| handles_mime(app, &file_type.mime_type))
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        candidates = all_apps.iter().filter(|app| !app.no_display).collect();
    }

    if let Some(app_id) = remembered_app_id.and_then(|app_id| by_id.get(app_id).copied()) {
        candidates.push(app_id);
    }

    if let Some(app_id) = system_default_app_id.and_then(|app_id| by_id.get(app_id).copied()) {
        candidates.push(app_id);
    }

    let mut seen = HashSet::new();
    let mut apps = candidates
        .into_iter()
        .filter(|app| seen.insert(app.id.clone()))
        .map(|app| {
            let is_system_default = system_default_app_id == Some(app.id.as_str());
            let is_remembered_default = remembered_app_id == Some(app.id.as_str());

            OpenWithApp {
                id: app.id.clone(),
                name: app.name.clone(),
                description: if is_remembered_default {
                    "Remembered default".to_string()
                } else if is_system_default {
                    "System default".to_string()
                } else if handles_mime(app, &file_type.mime_type) {
                    format!("Supports {}", file_type.label)
                } else {
                    "Application".to_string()
                },
                desktop_file: app.desktop_file.to_string_lossy().into_owned(),
                is_system_default,
                is_remembered_default,
            }
        })
        .collect::<Vec<_>>();

    apps.sort_by(|left, right| {
        app_rank(left)
            .cmp(&app_rank(right))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    apps
}

fn app_rank(app: &OpenWithApp) -> u8 {
    if app.is_remembered_default {
        return 0;
    }

    if app.is_system_default {
        return 1;
    }

    2
}

fn query_mime_type(path: &Path) -> Option<String> {
    let output = Command::new("xdg-mime")
        .arg("query")
        .arg("filetype")
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn system_default_app_id(mime_type: &str) -> Option<String> {
    let output = Command::new("xdg-mime")
        .arg("query")
        .arg("default")
        .arg(mime_type)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn handles_mime(app: &DesktopApp, mime_type: &str) -> bool {
    app.mime_types
        .iter()
        .any(|candidate| candidate == mime_type || candidate == "*/*")
}

fn desktop_apps() -> Vec<DesktopApp> {
    let mut files = Vec::new();

    for directory in desktop_application_dirs() {
        collect_desktop_files(&directory, &mut files);
    }

    let mut seen = HashSet::new();
    let mut apps = files
        .into_iter()
        .filter_map(|path| parse_desktop_app(&path))
        .filter(|app| seen.insert(app.id.clone()))
        .collect::<Vec<_>>();

    apps.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    apps
}

fn desktop_application_dirs() -> Vec<PathBuf> {
    let mut directories = Vec::new();

    if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
        directories.push(PathBuf::from(value).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share/applications"));
    }

    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());

    for directory in data_dirs.split(':').filter(|part| !part.trim().is_empty()) {
        directories.push(PathBuf::from(directory).join("applications"));
    }

    if let Some(home) = std::env::var_os("HOME") {
        directories
            .push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }
    directories.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    let mut seen = HashSet::new();
    directories
        .into_iter()
        .filter(|directory| seen.insert(directory.clone()))
        .collect()
}

fn collect_desktop_files(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.extension().and_then(OsStr::to_str) == Some("desktop") {
            output.push(path);
        }
    }
}

fn parse_desktop_app(path: &Path) -> Option<DesktopApp> {
    let contents = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut fields = HashMap::new();

    for raw_line in contents.lines() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_entry {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        if key.contains('[') {
            continue;
        }

        fields.insert(key.to_string(), value.trim().to_string());
    }

    if fields
        .get("Type")
        .map(String::as_str)
        .unwrap_or("Application")
        != "Application"
    {
        return None;
    }

    if desktop_bool(fields.get("Hidden")) {
        return None;
    }

    let exec = fields.get("Exec")?.trim().to_string();

    if exec.is_empty() {
        return None;
    }

    let name = fields
        .get("Name")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| path.file_stem().and_then(OsStr::to_str).map(str::to_string))?;
    let id = path.file_name().and_then(OsStr::to_str)?.to_string();
    let mime_types = fields
        .get("MimeType")
        .map(|value| {
            value
                .split(';')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(DesktopApp {
        id,
        name,
        exec,
        desktop_file: path.to_path_buf(),
        mime_types,
        no_display: desktop_bool(fields.get("NoDisplay")),
    })
}

fn desktop_bool(value: Option<&String>) -> bool {
    value
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn tokenize_exec(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = exec.chars().peekable();
    let mut quote = None;

    while let Some(character) = chars.next() {
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else if character == '\\' {
                if let Some(next) = chars.next() {
                    token.push(next);
                }
            } else {
                token.push(character);
            }
            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),
            '\\' => {
                if let Some(next) = chars.next() {
                    token.push(next);
                }
            }
            character if character.is_whitespace() => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            _ => token.push(character),
        }
    }

    if !token.is_empty() {
        tokens.push(token);
    }

    tokens
}

fn expand_exec_tokens(tokens: &[String], path: &Path) -> Vec<String> {
    let path_arg = path.to_string_lossy().into_owned();
    let uri_arg = file_uri(path).unwrap_or_else(|| path_arg.clone());
    let mut inserted_file = false;
    let mut result = Vec::new();

    for token in tokens {
        if token == "%i" || token == "%c" || token == "%k" {
            continue;
        }

        let mut value = token.clone();

        if value.contains("%f") || value.contains("%F") {
            value = value.replace("%f", &path_arg).replace("%F", &path_arg);
            inserted_file = true;
        }

        if value.contains("%u") || value.contains("%U") {
            value = value.replace("%u", &uri_arg).replace("%U", &uri_arg);
            inserted_file = true;
        }

        value = strip_field_codes(&value);

        if !value.is_empty() {
            result.push(value);
        }
    }

    if !inserted_file {
        result.push(path_arg);
    }

    result
}

fn strip_field_codes(value: &str) -> String {
    let mut result = String::new();
    let mut chars = value.chars().peekable();

    while let Some(character) = chars.next() {
        if character != '%' {
            result.push(character);
            continue;
        }

        match chars.next() {
            Some('%') => result.push('%'),
            Some(_) => {}
            None => result.push('%'),
        }
    }

    result
}

fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_desktop_exec_file_field_codes() {
        let tokens = tokenize_exec("demo-app --new-window %f %i %c %k");
        let args = expand_exec_tokens(&tokens, Path::new("/tmp/file with spaces.txt"));

        assert_eq!(
            args,
            vec![
                "demo-app".to_string(),
                "--new-window".to_string(),
                "/tmp/file with spaces.txt".to_string()
            ]
        );
    }

    #[test]
    fn appends_path_when_desktop_exec_has_no_file_field_code() {
        let tokens = tokenize_exec("demo-app --flag");
        let args = expand_exec_tokens(&tokens, Path::new("/tmp/image.png"));

        assert_eq!(
            args,
            vec![
                "demo-app".to_string(),
                "--flag".to_string(),
                "/tmp/image.png".to_string()
            ]
        );
    }

    #[test]
    fn infers_virtual_file_type_from_extension() {
        let file_type = file_type_for_virtual_path("folder/movie.mp4");

        assert_eq!(file_type.key, "ext:mp4");
        assert_eq!(file_type.mime_type, "video/mp4");
        assert_eq!(file_type.label, ".mp4 files");
    }
}
