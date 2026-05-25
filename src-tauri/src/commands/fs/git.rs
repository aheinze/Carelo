use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileInfo {
    pub root: String,
    pub repository: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub status: String,
    pub status_code: Option<String>,
    pub changed_entries: usize,
}

#[tauri::command]
pub async fn get_git_file_info(path: String) -> Result<Option<GitFileInfo>, FsError> {
    if archive::is_archive_uri(&path) || parse_remote_path(&path).is_some() {
        return Ok(None);
    }

    let error_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || git_file_info(&path))
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Git metadata lookup failed: {error}"),
                Some(error_path),
            )
        })?
}

fn git_file_info(path: &str) -> FsResult<Option<GitFileInfo>> {
    let local_path = expand_local_path(path)?;
    let is_directory = fs::metadata(&local_path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    let work_directory = if is_directory {
        local_path.as_path()
    } else {
        local_path.parent().unwrap_or_else(|| Path::new("."))
    };

    let Some(root) = git_output(work_directory, &["rev-parse", "--show-toplevel"]) else {
        return Ok(None);
    };

    let pathspec = selected_pathspec(&local_path, is_directory);

    let branch = git_branch(work_directory);
    let commit = git_output(work_directory, &["rev-parse", "--short", "HEAD"]);
    let status_output = git_output(
        work_directory,
        &["status", "--porcelain=v1", "-uno", "--", &pathspec],
    )
    .unwrap_or_default();
    let mut status_lines = status_output.lines().collect::<Vec<_>>();

    if status_lines.is_empty() && !is_directory {
        if let Some(untracked) = git_output(
            work_directory,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "--",
                &pathspec,
            ],
        ) {
            if !untracked.trim().is_empty() {
                status_lines.push("??");
            }
        }
    }

    let status_code = status_lines
        .first()
        .map(|line| line.chars().take(2).collect::<String>());
    let status = git_status_label(status_lines.as_slice());
    let root_path = PathBuf::from(root.trim());
    let repository = root_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| root.trim().to_string());

    Ok(Some(GitFileInfo {
        root: root.trim().to_string(),
        repository,
        branch,
        commit,
        status,
        status_code,
        changed_entries: status_lines.len(),
    }))
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn git_branch(cwd: &Path) -> Option<String> {
    let branch = git_output(cwd, &["branch", "--show-current"]);

    if branch.is_some() {
        return branch;
    }

    git_output(cwd, &["rev-parse", "--short", "HEAD"]).map(|commit| format!("Detached {commit}"))
}

fn selected_pathspec(path: &Path, is_directory: bool) -> String {
    if is_directory {
        return ".".to_string();
    }

    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

fn git_status_label(lines: &[&str]) -> String {
    if lines.is_empty() {
        return "Clean".to_string();
    }

    if lines.len() > 1 {
        return format!("Contains {} changes", lines.len());
    }

    let code = lines[0].chars().take(2).collect::<String>();

    if code == "??" {
        return "Untracked".to_string();
    }

    if code == "!!" {
        return "Ignored".to_string();
    }

    if code.contains('U') {
        return "Conflict".to_string();
    }

    let mut labels = Vec::new();

    for marker in code.chars().filter(|marker| !marker.is_whitespace()) {
        let label = match marker {
            'M' => "Modified",
            'A' => "Added",
            'D' => "Deleted",
            'R' => "Renamed",
            'C' => "Copied",
            'T' => "Type changed",
            _ => continue,
        };

        if !labels.contains(&label) {
            labels.push(label);
        }
    }

    if labels.is_empty() {
        "Changed".to_string()
    } else {
        labels.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::{git_status_label, selected_pathspec};
    use std::path::Path;

    #[test]
    fn labels_common_git_status_codes() {
        assert_eq!(git_status_label(&[]), "Clean");
        assert_eq!(git_status_label(&[" M src/main.rs"]), "Modified");
        assert_eq!(git_status_label(&["A  src/main.rs"]), "Added");
        assert_eq!(git_status_label(&["?? scratch.txt"]), "Untracked");
        assert_eq!(git_status_label(&["UU src/main.rs"]), "Conflict");
        assert_eq!(
            git_status_label(&[" M src/main.rs", "A  src/lib.rs"]),
            "Contains 2 changes",
        );
    }

    #[test]
    fn pathspec_is_relative_to_git_command_directory() {
        assert_eq!(
            selected_pathspec(Path::new("/repo/src/components/PreviewPanel.vue"), false),
            "PreviewPanel.vue",
        );
        assert_eq!(
            selected_pathspec(Path::new("/repo/src/components"), true),
            ".",
        );
    }
}
