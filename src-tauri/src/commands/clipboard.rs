use crate::fs::models::FsError;
use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use tauri::AppHandle;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemFileClipboardInput {
    pub mode: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemFileClipboard {
    pub mode: String,
    pub paths: Vec<String>,
}

#[tauri::command]
pub async fn write_system_file_clipboard(
    app: AppHandle,
    payload: SystemFileClipboardInput,
) -> Result<bool, FsError> {
    let mode = normalize_clipboard_mode(&payload.mode)?;
    let paths = normalize_local_paths(payload.paths)?;

    if paths.is_empty() {
        return Ok(false);
    }

    run_file_clipboard_on_main_thread(app, move || platform::write_file_clipboard(&mode, &paths))
        .await
}

#[tauri::command]
pub async fn read_system_file_clipboard(
    app: AppHandle,
) -> Result<Option<SystemFileClipboard>, FsError> {
    run_file_clipboard_on_main_thread(app, platform::read_file_clipboard).await
}

async fn run_file_clipboard_on_main_thread<T, F>(app: AppHandle, task: F) -> Result<T, FsError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, FsError> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();

    app.run_on_main_thread(move || {
        let _ = sender.send(task());
    })
    .map_err(|error| {
        FsError::new(
            "clipboard_unavailable",
            format!("Unable to access the system clipboard: {error}"),
            None,
        )
    })?;

    receiver.recv().map_err(|error| {
        FsError::new(
            "clipboard_unavailable",
            format!("Unable to receive the system clipboard result: {error}"),
            None,
        )
    })?
}

fn normalize_clipboard_mode(mode: &str) -> Result<String, FsError> {
    match mode {
        "copy" | "move" => Ok(mode.to_string()),
        "cut" => Ok("move".to_string()),
        value => Err(FsError::new(
            "invalid_clipboard_mode",
            format!("Unsupported file clipboard mode: {value}"),
            None,
        )),
    }
}

fn normalize_local_paths(paths: Vec<String>) -> Result<Vec<String>, FsError> {
    let mut normalized = Vec::new();

    for path in paths {
        let value = path.trim();

        if value.is_empty() {
            continue;
        }

        if value.starts_with("remote://") || value.contains("!/") {
            return Err(FsError::new(
                "unsupported_clipboard_path",
                "Only local filesystem paths can be written to the system file clipboard.",
                Some(value.to_string()),
            ));
        }

        normalized.push(value.to_string());
    }

    Ok(normalized)
}

fn path_from_file_uri(uri: &str) -> Option<String> {
    if !uri.starts_with("file://") {
        return None;
    }

    url::Url::parse(uri)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

fn parse_uri_list(text: &str) -> Vec<String> {
    unique_paths(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(path_from_file_uri),
    )
}

fn parse_gnome_file_clipboard(text: &str) -> Option<SystemFileClipboard> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if lines.len() < 2 {
        return None;
    }

    let mode = match lines[0] {
        "cut" => "move",
        "copy" => "copy",
        _ => return None,
    };
    let paths = unique_paths(
        lines
            .iter()
            .skip(1)
            .filter_map(|line| path_from_file_uri(line)),
    );

    if paths.is_empty() {
        None
    } else {
        Some(SystemFileClipboard {
            mode: mode.to_string(),
            paths,
        })
    }
}

fn unique_paths(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique = Vec::new();

    for path in paths {
        if !unique.iter().any(|candidate| candidate == &path) {
            unique.push(path);
        }
    }

    unique
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use gtk::{Clipboard, SelectionData, TargetEntry, TargetFlags};
    use std::path::Path;

    const TARGET_URI_LIST: &str = "text/uri-list";
    const TARGET_GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";
    const TARGET_NAUTILUS_CLIPBOARD: &str = "x-special/nautilus-clipboard";
    const TARGET_KDE_CUT_SELECTION: &str = "application/x-kde-cutselection";
    const TARGET_TEXT_PLAIN: &str = "text/plain";

    const INFO_URI_LIST: u32 = 1;
    const INFO_GNOME_COPIED_FILES: u32 = 2;
    const INFO_NAUTILUS_CLIPBOARD: u32 = 3;
    const INFO_KDE_CUT_SELECTION: u32 = 4;
    const INFO_TEXT_PLAIN: u32 = 5;

    #[derive(Clone)]
    struct ClipboardPayload {
        uri_list: String,
        gnome_payload: String,
        plain_text: String,
    }

    pub fn write_file_clipboard(mode: &str, paths: &[String]) -> Result<bool, FsError> {
        ensure_gtk()?;

        let payload = ClipboardPayload {
            uri_list: uri_list_for_paths(paths)?,
            gnome_payload: gnome_payload_for_paths(mode, paths)?,
            plain_text: paths.join("\n"),
        };
        let mut targets = vec![
            TargetEntry::new(TARGET_URI_LIST, TargetFlags::empty(), INFO_URI_LIST),
            TargetEntry::new(
                TARGET_GNOME_COPIED_FILES,
                TargetFlags::empty(),
                INFO_GNOME_COPIED_FILES,
            ),
            TargetEntry::new(
                TARGET_NAUTILUS_CLIPBOARD,
                TargetFlags::empty(),
                INFO_NAUTILUS_CLIPBOARD,
            ),
            TargetEntry::new(TARGET_TEXT_PLAIN, TargetFlags::empty(), INFO_TEXT_PLAIN),
        ];

        if mode == "move" {
            targets.push(TargetEntry::new(
                TARGET_KDE_CUT_SELECTION,
                TargetFlags::empty(),
                INFO_KDE_CUT_SELECTION,
            ));
        }

        let clipboard = Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        let success = clipboard.set_with_data(&targets, move |_, selection_data, info| {
            set_selection_data(selection_data, info, &payload);
        });

        if success {
            clipboard.store();
        }

        Ok(success)
    }

    pub fn read_file_clipboard() -> Result<Option<SystemFileClipboard>, FsError> {
        ensure_gtk()?;

        let clipboard = Clipboard::get(&gdk::SELECTION_CLIPBOARD);

        if let Some(data) =
            clipboard.wait_for_contents(&gdk::Atom::intern(TARGET_GNOME_COPIED_FILES))
        {
            if let Some(payload) = parse_gnome_file_clipboard(&selection_text(&data)) {
                return Ok(Some(payload));
            }
        }

        if let Some(data) =
            clipboard.wait_for_contents(&gdk::Atom::intern(TARGET_NAUTILUS_CLIPBOARD))
        {
            if let Some(payload) = parse_gnome_file_clipboard(&selection_text(&data)) {
                return Ok(Some(payload));
            }
        }

        let Some(data) = clipboard.wait_for_contents(&gdk::Atom::intern(TARGET_URI_LIST)) else {
            return Ok(None);
        };
        let paths = parse_uri_list(&selection_text(&data));

        if paths.is_empty() {
            return Ok(None);
        }

        Ok(Some(SystemFileClipboard {
            mode: kde_clipboard_mode(&clipboard),
            paths,
        }))
    }

    fn ensure_gtk() -> Result<(), FsError> {
        if gtk::is_initialized_main_thread() {
            return Ok(());
        }

        gtk::init().map_err(|error| {
            FsError::new(
                "clipboard_unavailable",
                format!("Unable to initialize GTK clipboard access: {error}"),
                None,
            )
        })
    }

    fn set_selection_data(selection_data: &SelectionData, info: u32, payload: &ClipboardPayload) {
        match info {
            INFO_URI_LIST => {
                set_bytes(selection_data, TARGET_URI_LIST, payload.uri_list.as_bytes())
            }
            INFO_GNOME_COPIED_FILES => set_bytes(
                selection_data,
                TARGET_GNOME_COPIED_FILES,
                payload.gnome_payload.as_bytes(),
            ),
            INFO_NAUTILUS_CLIPBOARD => set_bytes(
                selection_data,
                TARGET_NAUTILUS_CLIPBOARD,
                payload.gnome_payload.as_bytes(),
            ),
            INFO_KDE_CUT_SELECTION => set_bytes(selection_data, TARGET_KDE_CUT_SELECTION, b"1"),
            INFO_TEXT_PLAIN => {
                let _ = selection_data.set_text(&payload.plain_text);
            }
            _ => {}
        }
    }

    fn set_bytes(selection_data: &SelectionData, target: &str, bytes: &[u8]) {
        selection_data.set(&gdk::Atom::intern(target), 8, bytes);
    }

    fn uri_list_for_paths(paths: &[String]) -> Result<String, FsError> {
        paths
            .iter()
            .map(|path| file_uri_for_path(path))
            .collect::<Result<Vec<_>, _>>()
            .map(|uris| uris.join("\n"))
    }

    fn gnome_payload_for_paths(mode: &str, paths: &[String]) -> Result<String, FsError> {
        let action = if mode == "move" { "cut" } else { "copy" };
        let mut lines = vec![action.to_string()];
        lines.extend(
            paths
                .iter()
                .map(|path| file_uri_for_path(path))
                .collect::<Result<Vec<_>, _>>()?,
        );

        Ok(lines.join("\n"))
    }

    fn file_uri_for_path(path: &str) -> Result<String, FsError> {
        url::Url::from_file_path(Path::new(path))
            .map(|url| url.to_string())
            .map_err(|_| {
                FsError::new(
                    "invalid_path",
                    "Unable to convert path to a file URI.",
                    Some(path.to_string()),
                )
            })
    }

    fn selection_text(data: &SelectionData) -> String {
        String::from_utf8_lossy(&data.data())
            .trim_end_matches('\0')
            .to_string()
    }

    fn kde_clipboard_mode(clipboard: &Clipboard) -> String {
        let Some(data) = clipboard.wait_for_contents(&gdk::Atom::intern(TARGET_KDE_CUT_SELECTION))
        else {
            return "copy".to_string();
        };
        let value = selection_text(&data);

        if value.trim() == "1" || value.trim().eq_ignore_ascii_case("true") {
            "move".to_string()
        } else {
            "copy".to_string()
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    pub fn write_file_clipboard(_mode: &str, _paths: &[String]) -> Result<bool, FsError> {
        Ok(false)
    }

    pub fn read_file_clipboard() -> Result<Option<SystemFileClipboard>, FsError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gnome_file_clipboard_payload() {
        let parsed = parse_gnome_file_clipboard("cut\nfile:///tmp/a.txt\nfile:///tmp/b%20c.txt\n")
            .expect("clipboard payload");

        assert_eq!(parsed.mode, "move");
        assert_eq!(parsed.paths, vec!["/tmp/a.txt", "/tmp/b c.txt"]);
    }

    #[test]
    fn parses_uri_list_ignoring_comments_and_duplicates() {
        let paths =
            parse_uri_list("# comment\nfile:///tmp/a.txt\nfile:///tmp/a.txt\nfile:///tmp/b.txt\n");

        assert_eq!(paths, vec!["/tmp/a.txt", "/tmp/b.txt"]);
    }
}
