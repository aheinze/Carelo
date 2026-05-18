use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fs::models::{
    FileEntry, FileEntryKind, FileMetadata, FilePermissions, FsError, FsResult, PermissionSet,
};

const FIND_FIELD_COUNT: usize = 9;
const STAT_FIELD_COUNT: usize = 10;

pub fn list_directory(password: &str, path: &str) -> FsResult<Vec<FileEntry>> {
    let directory = expand_path(path)?;
    let stdout = run_sudo_command(
        password,
        "find",
        [
            OsString::from(directory.as_os_str()),
            OsString::from("-mindepth"),
            OsString::from("1"),
            OsString::from("-maxdepth"),
            OsString::from("1"),
            OsString::from("-printf"),
            OsString::from("%f\\0%p\\0%y\\0%Y\\0%s\\0%T@\\0%m\\0%u\\0%g\\0"),
        ],
        Some(&directory),
    )?;
    let mut entries = parse_find_entries(&stdout)?;

    entries.sort_by(compare_entries);
    Ok(entries)
}

pub fn get_file_metadata(password: &str, path: &str) -> FsResult<FileMetadata> {
    let path = expand_path(path)?;
    let symlink_stat = stat_path(password, &path, false)?;
    let is_symlink = symlink_stat.kind == FileEntryKind::Symlink;
    let stat = if is_symlink {
        stat_path(password, &path, true).unwrap_or_else(|_| symlink_stat.clone())
    } else {
        symlink_stat
    };
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();

    Ok(FileMetadata {
        path: path.to_string_lossy().into_owned(),
        kind: stat.kind,
        size: if stat.kind == FileEntryKind::File {
            Some(stat.size)
        } else {
            None
        },
        modified_at: stat.modified_at,
        created_at: stat.created_at,
        accessed_at: stat.accessed_at,
        is_hidden: name.starts_with('.'),
        is_symlink,
        is_readonly: is_readonly_mode(stat.mode),
        permissions: Some(permissions_for_mode(
            stat.mode,
            stat.owner_name,
            stat.group_name,
            stat.uid,
            stat.gid,
        )),
    })
}

pub fn create_folder(password: &str, path: &str) -> FsResult<()> {
    let path = expand_path(path)?;
    run_sudo_command(
        password,
        "mkdir",
        [OsString::from(path.as_os_str())],
        Some(&path),
    )
    .map(|_| ())
}

pub fn rename_item(password: &str, from: &str, to: &str) -> FsResult<()> {
    move_item(password, from, to, true)
}

pub fn delete_item(password: &str, path: &str) -> FsResult<()> {
    let path = expand_path(path)?;

    if path.as_os_str().is_empty() || path == Path::new("/") {
        return Err(FsError::new(
            "unsafe_delete_target",
            "Refusing to delete the root directory.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    run_sudo_command(
        password,
        "rm",
        [
            OsString::from("-rf"),
            OsString::from("--"),
            OsString::from(path.as_os_str()),
        ],
        Some(&path),
    )
    .map(|_| ())
}

pub fn copy_item(password: &str, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
    let from = expand_path(from)?;
    let to = expand_path(to)?;
    let destination_exists = sudo_path_exists(password, &to)?;

    if !overwrite && destination_exists {
        return Err(destination_exists_error(&to));
    }

    if overwrite
        && destination_exists
        && (sudo_path_is_directory(password, &from)? || sudo_path_is_directory(password, &to)?)
    {
        return Err(destination_type_error(&to));
    }

    run_sudo_command(
        password,
        "cp",
        [
            OsString::from("-a"),
            OsString::from("-T"),
            OsString::from("--"),
            OsString::from(from.as_os_str()),
            OsString::from(to.as_os_str()),
        ],
        Some(&from),
    )
    .map(|_| ())
}

pub fn move_item(password: &str, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
    let from = expand_path(from)?;
    let to = expand_path(to)?;
    let destination_exists = sudo_path_exists(password, &to)?;

    if !overwrite && destination_exists {
        return Err(destination_exists_error(&to));
    }

    if overwrite
        && destination_exists
        && (sudo_path_is_directory(password, &from)? || sudo_path_is_directory(password, &to)?)
    {
        return Err(destination_type_error(&to));
    }

    match run_sudo_command(
        password,
        "mv",
        [
            OsString::from("-f"),
            OsString::from("-T"),
            OsString::from("--"),
            OsString::from(from.as_os_str()),
            OsString::from(to.as_os_str()),
        ],
        Some(&from),
    ) {
        Ok(_) => Ok(()),
        Err(error)
            if error.code == "sudo_failed"
                && error.message.contains("cannot stat")
                && error.message.contains("No such file or directory")
                && to.exists() =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn destination_exists_error(path: &Path) -> FsError {
    FsError::new(
        "destination_exists",
        "An item already exists at the destination.",
        Some(path.to_string_lossy().into_owned()),
    )
}

fn destination_type_error(path: &Path) -> FsError {
    FsError::new(
        "destination_type_conflict",
        "The existing destination has an incompatible type.",
        Some(path.to_string_lossy().into_owned()),
    )
}

fn sudo_path_exists(password: &str, path: &Path) -> FsResult<bool> {
    if sudo_test_path(password, path, "-e")? {
        return Ok(true);
    }

    sudo_test_path(password, path, "-L")
}

fn sudo_path_is_directory(password: &str, path: &Path) -> FsResult<bool> {
    sudo_test_path(password, path, "-d")
}

fn sudo_test_path(password: &str, path: &Path, flag: &str) -> FsResult<bool> {
    match run_sudo_command(
        password,
        "test",
        [OsString::from(flag), OsString::from(path.as_os_str())],
        Some(path),
    ) {
        Ok(_) => Ok(true),
        Err(error) if error.code == "sudo_failed" && error.message.starts_with("test failed") => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

pub fn archive_items(
    password: &str,
    paths: &[String],
    destination: &str,
    overwrite: bool,
) -> FsResult<()> {
    if paths.is_empty() {
        return Err(FsError::new(
            "archive_empty_selection",
            "Select at least one item to archive.",
            None,
        ));
    }

    let source_paths = paths
        .iter()
        .map(|path| expand_path(path))
        .collect::<FsResult<Vec<_>>>()?;
    let destination = expand_path(destination)?;
    let extension = destination
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    if extension != "zip" {
        return Err(FsError::new(
            "invalid_archive_destination",
            "Zip archive names must end in .zip.",
            Some(destination.to_string_lossy().into_owned()),
        ));
    }

    let destination_parent = destination.parent().ok_or_else(|| {
        FsError::new(
            "invalid_archive_destination",
            "Unable to resolve the archive destination folder.",
            Some(destination.to_string_lossy().into_owned()),
        )
    })?;
    let source_parent = common_parent(&source_paths)?;

    if destination.exists() {
        if destination.is_dir() {
            return Err(FsError::new(
                "archive_destination_is_directory",
                "A folder already exists with that archive name.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }

        if !overwrite {
            return Err(FsError::new(
                "archive_destination_exists",
                "A file already exists with that archive name.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }

        run_sudo_command(
            password,
            "rm",
            [
                OsString::from("-f"),
                OsString::from("--"),
                OsString::from(destination.as_os_str()),
            ],
            Some(&destination),
        )?;
    }

    run_sudo_command(
        password,
        "mkdir",
        [
            OsString::from("-p"),
            OsString::from("--"),
            OsString::from(destination_parent.as_os_str()),
        ],
        Some(destination_parent),
    )?;

    let mut args = vec![
        OsString::from("-r"),
        OsString::from("-q"),
        OsString::from(destination.as_os_str()),
        OsString::from("--"),
    ];

    for source_path in &source_paths {
        let file_name = source_path.file_name().ok_or_else(|| {
            FsError::new(
                "invalid_archive_source",
                "Unable to archive a path without a file name.",
                Some(source_path.to_string_lossy().into_owned()),
            )
        })?;
        args.push(OsString::from(file_name));
    }

    run_sudo_command_in_dir(password, "zip", args, Some(&destination), &source_parent)
        .map(|_| ())
        .map_err(|error| map_missing_tool(error, "zip"))
        .and_then(|_| validate_zip_created(&destination))
}

pub fn unarchive_items(
    password: &str,
    paths: &[String],
    destination_directory: &str,
) -> FsResult<Vec<String>> {
    if paths.is_empty() {
        return Err(FsError::new(
            "unarchive_empty_selection",
            "Select at least one zip archive to extract.",
            None,
        ));
    }

    let destination_directory = expand_path(destination_directory)?;

    run_sudo_command(
        password,
        "mkdir",
        [
            OsString::from("-p"),
            OsString::from("--"),
            OsString::from(destination_directory.as_os_str()),
        ],
        Some(&destination_directory),
    )?;

    let mut extracted_paths = Vec::new();

    for path in paths {
        let archive_path = expand_path(path)?;
        let extension = archive_path
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .to_ascii_lowercase();

        if extension != "zip" {
            return Err(FsError::new(
                "invalid_archive_source",
                "Only .zip archives can be extracted.",
                Some(archive_path.to_string_lossy().into_owned()),
            ));
        }

        let target_directory = unique_extraction_directory(&destination_directory, &archive_path)?;
        run_sudo_command(
            password,
            "mkdir",
            [
                OsString::from("--"),
                OsString::from(target_directory.as_os_str()),
            ],
            Some(&target_directory),
        )?;

        let result = run_sudo_command(
            password,
            "unzip",
            [
                OsString::from("-q"),
                OsString::from(archive_path.as_os_str()),
                OsString::from("-d"),
                OsString::from(target_directory.as_os_str()),
            ],
            Some(&archive_path),
        )
        .map(|_| ())
        .map_err(|error| map_missing_tool(error, "unzip"));

        if let Err(error) = result {
            let _ = run_sudo_command(
                password,
                "rm",
                [
                    OsString::from("-rf"),
                    OsString::from("--"),
                    OsString::from(target_directory.as_os_str()),
                ],
                Some(&target_directory),
            );
            return Err(error);
        }

        extracted_paths.push(target_directory.to_string_lossy().into_owned());
    }

    Ok(extracted_paths)
}

fn common_parent(paths: &[PathBuf]) -> FsResult<PathBuf> {
    let first = paths.first().ok_or_else(|| {
        FsError::new(
            "archive_empty_selection",
            "Select at least one item to archive.",
            None,
        )
    })?;
    let parent = first.parent().ok_or_else(|| {
        FsError::new(
            "invalid_archive_source",
            "Unable to resolve the selected item folder.",
            Some(first.to_string_lossy().into_owned()),
        )
    })?;

    for path in paths.iter().skip(1) {
        if path.parent() != Some(parent) {
            return Err(FsError::new(
                "archive_sources_mixed_folders",
                "Selected items must be in the same folder.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }
    }

    Ok(parent.to_path_buf())
}

fn unique_extraction_directory(parent: &Path, archive_path: &Path) -> FsResult<PathBuf> {
    let base_name = archive_path
        .file_stem()
        .and_then(OsStr::to_str)
        .map(safe_file_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Archive".to_string());

    for index in 1..1000 {
        let candidate_name = if index == 1 {
            base_name.clone()
        } else {
            format!("{base_name} {index}")
        };
        let candidate = parent.join(candidate_name);

        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(FsError::new(
        "extract_destination_unavailable",
        "Unable to choose a unique extraction folder.",
        Some(parent.to_string_lossy().into_owned()),
    ))
}

fn safe_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' | '\\' | '\0' => '_',
            _ => character,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

fn validate_zip_created(destination: &Path) -> FsResult<()> {
    if destination.exists() {
        Ok(())
    } else {
        Err(FsError::new(
            "archive_failed",
            "The zip archive was not created.",
            Some(destination.to_string_lossy().into_owned()),
        ))
    }
}

fn map_missing_tool(error: FsError, tool: &str) -> FsError {
    if error.code == "sudo_failed"
        && (error.message.contains("No such file or directory")
            || error.message.contains("command not found"))
    {
        return FsError::new(
            "archive_tool_missing",
            format!("The elevated archive operation requires the `{tool}` command."),
            error.path,
        );
    }

    error
}

fn expand_path(path: &str) -> FsResult<PathBuf> {
    let trimmed = path.trim();

    if trimmed.is_empty() || trimmed == "~" {
        return home_dir();
    }

    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }

    Ok(PathBuf::from(trimmed))
}

fn home_dir() -> FsResult<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }

    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }

    std::env::current_dir().map_err(|error| {
        FsError::new(
            "home_not_found",
            format!("Unable to resolve a home directory: {error}"),
            None,
        )
    })
}

fn run_sudo_command<I, S>(
    password: &str,
    program: &str,
    args: I,
    path: Option<&Path>,
) -> FsResult<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_sudo_command_with_current_dir(password, program, args, path, None)
}

fn run_sudo_command_in_dir<I, S>(
    password: &str,
    program: &str,
    args: I,
    path: Option<&Path>,
    current_dir: &Path,
) -> FsResult<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_sudo_command_with_current_dir(password, program, args, path, Some(current_dir))
}

fn run_sudo_command_with_current_dir<I, S>(
    password: &str,
    program: &str,
    args: I,
    path: Option<&Path>,
    current_dir: Option<&Path>,
) -> FsResult<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if password.is_empty() {
        return Err(FsError::new(
            "sudo_password_required",
            "A sudo password is required.",
            path.map(|path| path.to_string_lossy().into_owned()),
        ));
    }

    let mut command = Command::new("sudo");
    command
        .arg("-S")
        .arg("-p")
        .arg("")
        .arg("--")
        .arg(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }

    for arg in args {
        command.arg(arg);
    }

    let mut child = command.spawn().map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            "sudo_unavailable"
        } else {
            "sudo_failed"
        };

        FsError::new(
            code,
            format!("Unable to start sudo: {error}"),
            path.map(|path| path.to_string_lossy().into_owned()),
        )
    })?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(password.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|error| {
                FsError::new(
                    "sudo_failed",
                    format!("Unable to pass sudo password: {error}"),
                    path.map(|path| path.to_string_lossy().into_owned()),
                )
            })?;
    }

    drop(child.stdin.take());

    let output = child.wait_with_output().map_err(|error| {
        FsError::new(
            "sudo_failed",
            format!("Unable to wait for sudo command: {error}"),
            path.map(|path| path.to_string_lossy().into_owned()),
        )
    })?;

    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = classify_sudo_error(&stderr);
    let message = match code {
        "sudo_auth_failed" => "The sudo password was not accepted.".to_string(),
        "sudo_forbidden" => "The current user is not allowed to run sudo.".to_string(),
        _ => {
            let detail = stderr.trim();

            if detail.is_empty() {
                format!("{program} failed while running with sudo.")
            } else {
                format!("{program} failed while running with sudo: {detail}")
            }
        }
    };

    Err(FsError::new(
        code,
        message,
        path.map(|path| path.to_string_lossy().into_owned()),
    ))
}

fn classify_sudo_error(stderr: &str) -> &'static str {
    let lower = stderr.to_lowercase();

    if lower.contains("not in the sudoers") || lower.contains("may not run sudo") {
        return "sudo_forbidden";
    }

    if lower.contains("no new privileges") {
        return "sudo_unavailable";
    }

    if lower.contains("incorrect password")
        || lower.contains("try again")
        || lower.contains("authentication failure")
        || lower.contains("a password is required")
        || lower.contains("no password was provided")
        || lower.contains("conversation failed")
        || lower.contains("sorry, try again")
    {
        return "sudo_auth_failed";
    }

    "sudo_failed"
}

fn parse_find_entries(stdout: &[u8]) -> FsResult<Vec<FileEntry>> {
    let fields = nul_fields(stdout);

    if fields.is_empty() {
        return Ok(Vec::new());
    }

    if fields.len() % FIND_FIELD_COUNT != 0 {
        return Err(FsError::new(
            "sudo_parse_error",
            "Unable to parse elevated directory listing.",
            None,
        ));
    }

    fields
        .chunks_exact(FIND_FIELD_COUNT)
        .map(|chunk| {
            let name = chunk[0].clone();
            let path = chunk[1].clone();
            let file_type = chunk[2].as_str();
            let target_type = chunk[3].as_str();
            let kind = kind_from_find_type(file_type, target_type);
            let mode = parse_octal_mode(&chunk[6])?;

            Ok(FileEntry {
                name: name.clone(),
                path,
                kind,
                size: if kind == FileEntryKind::File {
                    Some(parse_u64(&chunk[4])?)
                } else {
                    None
                },
                modified_at: parse_timestamp(&chunk[5]),
                is_hidden: name.starts_with('.'),
                is_symlink: file_type == "l",
                is_readonly: is_readonly_mode(mode),
                tag_color: None,
            })
        })
        .collect()
}

#[derive(Clone)]
struct SudoStat {
    kind: FileEntryKind,
    size: u64,
    modified_at: Option<u64>,
    created_at: Option<u64>,
    accessed_at: Option<u64>,
    mode: u32,
    owner_name: Option<String>,
    group_name: Option<String>,
    uid: Option<u32>,
    gid: Option<u32>,
}

fn stat_path(password: &str, path: &Path, follow: bool) -> FsResult<SudoStat> {
    let mut args = Vec::new();

    if follow {
        args.push(OsString::from("-L"));
    }

    args.push(OsString::from("-c"));
    args.push(OsString::from(
        "%F\\0%s\\0%Y\\0%W\\0%X\\0%a\\0%U\\0%G\\0%u\\0%g\\0",
    ));
    args.push(OsString::from("--"));
    args.push(OsString::from(path.as_os_str()));

    let stdout = run_sudo_command(password, "stat", args, Some(path))?;
    parse_stat(&stdout)
}

fn parse_stat(stdout: &[u8]) -> FsResult<SudoStat> {
    let fields = nul_fields(stdout);

    if fields.len() != STAT_FIELD_COUNT {
        return Err(FsError::new(
            "sudo_parse_error",
            "Unable to parse elevated file metadata.",
            None,
        ));
    }

    let mode = parse_octal_mode(&fields[5])?;

    Ok(SudoStat {
        kind: kind_from_stat_type(&fields[0]),
        size: parse_u64(&fields[1])?,
        modified_at: parse_u64(&fields[2]).ok(),
        created_at: parse_birth_time(&fields[3]),
        accessed_at: parse_u64(&fields[4]).ok(),
        mode,
        owner_name: non_empty_string(&fields[6]),
        group_name: non_empty_string(&fields[7]),
        uid: parse_u32(&fields[8]).ok(),
        gid: parse_u32(&fields[9]).ok(),
    })
}

fn nul_fields(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect()
}

fn parse_u64(value: &str) -> FsResult<u64> {
    value.trim().parse::<u64>().map_err(|error| {
        FsError::new(
            "sudo_parse_error",
            format!("Unable to parse numeric sudo output: {error}"),
            None,
        )
    })
}

fn parse_u32(value: &str) -> FsResult<u32> {
    value.trim().parse::<u32>().map_err(|error| {
        FsError::new(
            "sudo_parse_error",
            format!("Unable to parse numeric sudo output: {error}"),
            None,
        )
    })
}

fn parse_octal_mode(value: &str) -> FsResult<u32> {
    u32::from_str_radix(value.trim(), 8).map_err(|error| {
        FsError::new(
            "sudo_parse_error",
            format!("Unable to parse sudo permissions: {error}"),
            None,
        )
    })
}

fn parse_timestamp(value: &str) -> Option<u64> {
    value
        .trim()
        .split_once('.')
        .map(|(seconds, _)| seconds)
        .unwrap_or_else(|| value.trim())
        .parse::<u64>()
        .ok()
}

fn parse_birth_time(value: &str) -> Option<u64> {
    match parse_timestamp(value) {
        Some(0) | None => None,
        Some(timestamp) => Some(timestamp),
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();

    if trimmed.is_empty() || trimmed == "UNKNOWN" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn kind_from_find_type(file_type: &str, target_type: &str) -> FileEntryKind {
    let effective_type = if file_type == "l" {
        match target_type {
            "d" | "f" => target_type,
            _ => file_type,
        }
    } else {
        file_type
    };

    match effective_type {
        "d" => FileEntryKind::Directory,
        "f" => FileEntryKind::File,
        "l" => FileEntryKind::Symlink,
        _ => FileEntryKind::Other,
    }
}

fn kind_from_stat_type(file_type: &str) -> FileEntryKind {
    let file_type = file_type.to_lowercase();

    if file_type.contains("directory") {
        FileEntryKind::Directory
    } else if file_type.contains("regular") {
        FileEntryKind::File
    } else if file_type.contains("symbolic link") {
        FileEntryKind::Symlink
    } else {
        FileEntryKind::Other
    }
}

fn is_readonly_mode(mode: u32) -> bool {
    mode & 0o222 == 0
}

fn permissions_for_mode(
    mode: u32,
    owner_name: Option<String>,
    group_name: Option<String>,
    uid: Option<u32>,
    gid: Option<u32>,
) -> FilePermissions {
    FilePermissions {
        owner: permission_set(mode, 0o400, 0o200, 0o100),
        group: permission_set(mode, 0o040, 0o020, 0o010),
        others: permission_set(mode, 0o004, 0o002, 0o001),
        symbolic: symbolic_mode(mode),
        octal: format!("{:03o}", mode & 0o777),
        mode,
        owner_name,
        group_name,
        uid,
        gid,
    }
}

fn permission_set(mode: u32, read: u32, write: u32, execute: u32) -> PermissionSet {
    PermissionSet {
        read: mode & read != 0,
        write: mode & write != 0,
        execute: mode & execute != 0,
    }
}

fn symbolic_mode(mode: u32) -> String {
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];

    bits.into_iter()
        .map(|(bit, character)| if mode & bit != 0 { character } else { '-' })
        .collect()
}

fn compare_entries(a: &FileEntry, b: &FileEntry) -> std::cmp::Ordering {
    a.kind
        .sort_rank()
        .cmp(&b.kind.sort_rank())
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}
