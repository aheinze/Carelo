use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use zip::read::ZipFile;
use zip::result::ZipError;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::fs::models::{FsError, FsResult};

const PROGRESS_BYTE_STEP: u64 = 512 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ArchiveProgress {
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub processed_entries: u64,
    pub total_entries: u64,
    pub current_path: Option<String>,
    pub current_bytes: u64,
    pub current_total_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct ArchiveMeasure {
    bytes: u64,
    entries: u64,
}

#[derive(Debug, Clone, Default)]
struct ProgressState {
    processed_bytes: u64,
    total_bytes: u64,
    processed_entries: u64,
    total_entries: u64,
    last_emitted_bytes: u64,
    last_emitted_current_bytes: u64,
    last_emitted_entries: u64,
    current_bytes: u64,
    current_total_bytes: u64,
}

pub fn archive_items(paths: &[String], destination: &str, overwrite: bool) -> FsResult<()> {
    archive_items_with_progress(paths, destination, overwrite, |_| {}, || false)
}

pub fn archive_items_with_progress<F, C>(
    paths: &[String],
    destination: &str,
    overwrite: bool,
    mut on_progress: F,
    mut should_cancel: C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
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

    validate_archive_destination(&source_paths, &destination, overwrite)?;
    check_cancelled(&mut should_cancel, Some(&destination))?;

    let mut measure_visited_directories = HashSet::new();
    emit_progress(
        &mut ProgressState::default(),
        Some(&destination),
        true,
        &mut on_progress,
    );
    let measure = measure_paths(
        &source_paths,
        &mut measure_visited_directories,
        &mut should_cancel,
    )?;
    if measure.entries == 0 {
        return Err(FsError::new(
            "archive_no_supported_sources",
            "No archiveable files or folders were found.",
            None,
        ));
    }

    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };
    emit_progress(&mut progress, None, true, &mut on_progress);

    let archive_file = File::create(&destination)
        .map_err(|error| FsError::io("Unable to create zip archive", &destination, error))?;
    let mut zip = ZipWriter::new(archive_file);
    let mut visited_directories = HashSet::new();

    let result = (|| -> FsResult<()> {
        for source_path in &source_paths {
            let archive_name = source_path.file_name().ok_or_else(|| {
                FsError::new(
                    "invalid_archive_source",
                    "Unable to archive a path without a file name.",
                    Some(source_path.to_string_lossy().into_owned()),
                )
            })?;

            add_path_to_archive(
                &mut zip,
                source_path,
                Path::new(archive_name),
                &mut visited_directories,
                &mut progress,
                &mut on_progress,
                &mut should_cancel,
            )?;
        }

        zip.finish()
            .map(|_| ())
            .map_err(|error| zip_error("Unable to finish zip archive", &destination, error))?;
        progress.processed_bytes = progress.total_bytes;
        progress.processed_entries = progress.total_entries;
        emit_progress(&mut progress, None, true, &mut on_progress);
        Ok(())
    })();

    if matches!(&result, Err(error) if error.code == "operation_cancelled") {
        let _ = fs::remove_file(&destination);
    }

    result
}

pub fn unarchive_items(paths: &[String], destination_directory: &str) -> FsResult<Vec<String>> {
    unarchive_items_with_progress(paths, destination_directory, |_| {}, || false)
}

pub fn unarchive_items_with_progress<F, C>(
    paths: &[String],
    destination_directory: &str,
    mut on_progress: F,
    mut should_cancel: C,
) -> FsResult<Vec<String>>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    if paths.is_empty() {
        return Err(FsError::new(
            "unarchive_empty_selection",
            "Select at least one zip archive to extract.",
            None,
        ));
    }

    let destination_directory = expand_path(destination_directory)?;

    fs::create_dir_all(&destination_directory).map_err(|error| {
        FsError::io(
            "Unable to create extraction directory",
            &destination_directory,
            error,
        )
    })?;

    let archive_paths = paths
        .iter()
        .map(|path| expand_path(path))
        .collect::<FsResult<Vec<_>>>()?;
    emit_progress(
        &mut ProgressState::default(),
        Some(&destination_directory),
        true,
        &mut on_progress,
    );
    let measure = measure_archives(&archive_paths, &mut should_cancel)?;
    let mut progress = ProgressState {
        total_bytes: measure.bytes,
        total_entries: measure.entries,
        ..ProgressState::default()
    };
    emit_progress(&mut progress, None, true, &mut on_progress);

    let mut extracted_paths = Vec::new();

    for archive_path in archive_paths {
        validate_zip_source(&archive_path)?;

        let target_directory = unique_extraction_directory(&destination_directory, &archive_path)?;
        fs::create_dir(&target_directory).map_err(|error| {
            FsError::io(
                "Unable to create extraction folder",
                &target_directory,
                error,
            )
        })?;

        if let Err(error) = extract_archive_to_directory(
            &archive_path,
            &target_directory,
            &mut progress,
            &mut on_progress,
            &mut should_cancel,
        ) {
            let _ = fs::remove_dir_all(&target_directory);
            return Err(error);
        }

        extracted_paths.push(target_directory.to_string_lossy().into_owned());
    }

    progress.processed_bytes = progress.total_bytes;
    progress.processed_entries = progress.total_entries;
    emit_progress(&mut progress, None, true, &mut on_progress);
    Ok(extracted_paths)
}

fn add_path_to_archive<F, C>(
    zip: &mut ZipWriter<File>,
    path: &Path,
    archive_path: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(path))?;

    let Some(metadata) = archive_metadata(path)? else {
        return Ok(());
    };

    if metadata.is_dir() {
        let canonical = fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive directory", path, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "archive_cycle",
                "Refusing to archive a directory cycle.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let directory_name = zip_directory_name(archive_path)?;
        zip.add_directory(directory_name, zip_options(&metadata))
            .map_err(|error| zip_error("Unable to add directory to zip archive", path, error))?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(path), true, on_progress);

        for child in fs::read_dir(path)
            .map_err(|error| FsError::io("Unable to read archive directory", path, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read archive directory entry", path, error)
            })?;
            add_path_to_archive(
                zip,
                &child.path(),
                &archive_path.join(child.file_name()),
                visited_directories,
                progress,
                on_progress,
                should_cancel,
            )?;
        }

        return Ok(());
    }

    if metadata.is_file() {
        let archive_name = zip_entry_name(archive_path)?;
        let source = File::open(path)
            .map_err(|error| FsError::io("Unable to open archive source file", path, error))?;

        zip.start_file(archive_name, zip_options(&metadata))
            .map_err(|error| zip_error("Unable to add file to zip archive", path, error))?;
        copy_with_progress(
            source,
            zip,
            path,
            metadata.len(),
            progress,
            on_progress,
            should_cancel,
        )?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(path), true, on_progress);

        return Ok(());
    }

    Err(FsError::new(
        "unsupported_archive_source",
        "Only regular files and folders can be archived.",
        Some(path.to_string_lossy().into_owned()),
    ))
}

fn extract_archive_to_directory<F, C>(
    archive_path: &Path,
    target_directory: &Path,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    let file = File::open(archive_path)
        .map_err(|error| FsError::io("Unable to open zip archive", archive_path, error))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| zip_error("Unable to read zip archive", archive_path, error))?;

    for index in 0..archive.len() {
        check_cancelled(should_cancel, Some(archive_path))?;

        let entry = archive
            .by_index(index)
            .map_err(|error| zip_error("Unable to read zip archive entry", archive_path, error))?;

        if is_symlink_entry(&entry) {
            return Err(FsError::new(
                "unsupported_archive_entry",
                "Zip entries that create symbolic links are not supported.",
                Some(archive_path.to_string_lossy().into_owned()),
            ));
        }

        let enclosed_name = entry.enclosed_name().ok_or_else(|| {
            FsError::new(
                "unsafe_archive_entry",
                "The zip archive contains an unsafe path.",
                Some(entry.name().to_string()),
            )
        })?;
        let output_path = target_directory.join(enclosed_name);
        let mode = entry.unix_mode();

        if entry.name().ends_with('/') {
            fs::create_dir_all(&output_path).map_err(|error| {
                FsError::io("Unable to create extracted directory", &output_path, error)
            })?;
            progress.processed_entries += 1;
            emit_progress(progress, Some(&output_path), true, on_progress);
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                FsError::io("Unable to create extracted parent directory", parent, error)
            })?;
        }

        let current_total_bytes = entry.size();
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| FsError::io("Unable to create extracted file", &output_path, error))?;
        copy_with_progress(
            entry,
            &mut output,
            &output_path,
            current_total_bytes,
            progress,
            on_progress,
            should_cancel,
        )?;
        progress.processed_entries += 1;
        emit_progress(progress, Some(&output_path), true, on_progress);

        #[cfg(unix)]
        if let Some(mode) = mode {
            let permissions = fs::Permissions::from_mode(mode & 0o777);
            fs::set_permissions(&output_path, permissions).map_err(|error| {
                FsError::io(
                    "Unable to apply extracted file permissions",
                    &output_path,
                    error,
                )
            })?;
        }
    }

    Ok(())
}

fn measure_paths<C>(
    paths: &[PathBuf],
    visited_directories: &mut HashSet<PathBuf>,
    should_cancel: &mut C,
) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    let mut measure = ArchiveMeasure::default();

    for path in paths {
        measure.add(measure_path(path, visited_directories, should_cancel)?);
    }

    Ok(measure)
}

fn measure_path<C>(
    path: &Path,
    visited_directories: &mut HashSet<PathBuf>,
    should_cancel: &mut C,
) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    check_cancelled(should_cancel, Some(path))?;

    let Some(metadata) = archive_metadata(path)? else {
        return Ok(ArchiveMeasure::default());
    };

    if metadata.is_dir() {
        let canonical = fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive directory", path, error))?;

        if !visited_directories.insert(canonical) {
            return Err(FsError::new(
                "archive_cycle",
                "Refusing to archive a directory cycle.",
                Some(path.to_string_lossy().into_owned()),
            ));
        }

        let mut measure = ArchiveMeasure {
            bytes: 0,
            entries: 1,
        };

        for child in fs::read_dir(path)
            .map_err(|error| FsError::io("Unable to read archive directory", path, error))?
        {
            let child = child.map_err(|error| {
                FsError::io("Unable to read archive directory entry", path, error)
            })?;
            measure.add(measure_path(
                &child.path(),
                visited_directories,
                should_cancel,
            )?);
        }

        return Ok(measure);
    }

    if metadata.is_file() {
        return Ok(ArchiveMeasure {
            bytes: metadata.len(),
            entries: 1,
        });
    }

    Err(FsError::new(
        "unsupported_archive_source",
        "Only regular files and folders can be archived.",
        Some(path.to_string_lossy().into_owned()),
    ))
}

fn measure_archives<C>(paths: &[PathBuf], should_cancel: &mut C) -> FsResult<ArchiveMeasure>
where
    C: FnMut() -> bool,
{
    let mut measure = ArchiveMeasure::default();

    for path in paths {
        check_cancelled(should_cancel, Some(path))?;
        validate_zip_source(path)?;
        let file = File::open(path)
            .map_err(|error| FsError::io("Unable to open zip archive", path, error))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| zip_error("Unable to read zip archive", path, error))?;

        for index in 0..archive.len() {
            check_cancelled(should_cancel, Some(path))?;
            let entry = archive
                .by_index(index)
                .map_err(|error| zip_error("Unable to read zip archive entry", path, error))?;

            if is_symlink_entry(&entry) {
                return Err(FsError::new(
                    "unsupported_archive_entry",
                    "Zip entries that create symbolic links are not supported.",
                    Some(path.to_string_lossy().into_owned()),
                ));
            }

            if entry.enclosed_name().is_none() {
                return Err(FsError::new(
                    "unsafe_archive_entry",
                    "The zip archive contains an unsafe path.",
                    Some(entry.name().to_string()),
                ));
            }

            measure.entries += 1;

            if !entry.name().ends_with('/') {
                measure.bytes = measure.bytes.saturating_add(entry.size());
            }
        }
    }

    Ok(measure)
}

impl ArchiveMeasure {
    fn add(&mut self, other: ArchiveMeasure) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.entries = self.entries.saturating_add(other.entries);
    }
}

fn archive_metadata(path: &Path) -> FsResult<Option<fs::Metadata>> {
    let symlink_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(FsError::io(
                "Unable to read archive source metadata",
                path,
                error,
            ));
        }
    };

    if !symlink_metadata.file_type().is_symlink() {
        return Ok(Some(symlink_metadata));
    }

    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(FsError::io(
            "Unable to read archive source metadata",
            path,
            error,
        )),
    }
}

fn copy_with_progress<R, W, F, C>(
    mut reader: R,
    writer: &mut W,
    current_path: &Path,
    current_total_bytes: u64,
    progress: &mut ProgressState,
    on_progress: &mut F,
    should_cancel: &mut C,
) -> FsResult<()>
where
    R: Read,
    W: Write,
    F: FnMut(ArchiveProgress),
    C: FnMut() -> bool,
{
    let mut buffer = [0_u8; 64 * 1024];
    progress.current_bytes = 0;
    progress.current_total_bytes = current_total_bytes;
    emit_progress(progress, Some(current_path), true, on_progress);

    loop {
        check_cancelled(should_cancel, Some(current_path))?;
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|error| FsError::io("Unable to read archive data", current_path, error))?;

        if bytes_read == 0 {
            break;
        }

        writer
            .write_all(&buffer[..bytes_read])
            .map_err(|error| FsError::io("Unable to write archive data", current_path, error))?;
        progress.processed_bytes = progress.processed_bytes.saturating_add(bytes_read as u64);
        progress.current_bytes = progress.current_bytes.saturating_add(bytes_read as u64);
        emit_progress(progress, Some(current_path), false, on_progress);
    }

    progress.current_bytes = progress.current_total_bytes;
    emit_progress(progress, Some(current_path), true, on_progress);
    progress.current_bytes = 0;
    progress.current_total_bytes = 0;
    emit_progress(progress, Some(current_path), true, on_progress);
    Ok(())
}

fn check_cancelled<C>(should_cancel: &mut C, path: Option<&Path>) -> FsResult<()>
where
    C: FnMut() -> bool,
{
    if should_cancel() {
        return Err(FsError::new(
            "operation_cancelled",
            "The file operation was cancelled.",
            path.map(|path| path.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

fn emit_progress<F>(
    state: &mut ProgressState,
    current_path: Option<&Path>,
    force: bool,
    on_progress: &mut F,
) where
    F: FnMut(ArchiveProgress),
{
    let bytes_delta = state
        .processed_bytes
        .saturating_sub(state.last_emitted_bytes);
    let current_delta = state
        .current_bytes
        .saturating_sub(state.last_emitted_current_bytes);
    let entries_changed = state.processed_entries != state.last_emitted_entries;

    if !force
        && !entries_changed
        && bytes_delta < PROGRESS_BYTE_STEP
        && current_delta < PROGRESS_BYTE_STEP
    {
        return;
    }

    state.last_emitted_bytes = state.processed_bytes;
    state.last_emitted_current_bytes = state.current_bytes;
    state.last_emitted_entries = state.processed_entries;

    on_progress(ArchiveProgress {
        processed_bytes: state.processed_bytes.min(state.total_bytes),
        total_bytes: state.total_bytes,
        processed_entries: state.processed_entries.min(state.total_entries),
        total_entries: state.total_entries,
        current_path: current_path.map(|path| path.to_string_lossy().into_owned()),
        current_bytes: state.current_bytes.min(state.current_total_bytes),
        current_total_bytes: state.current_total_bytes,
    });
}

fn validate_archive_destination(
    source_paths: &[PathBuf],
    destination: &Path,
    overwrite: bool,
) -> FsResult<()> {
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

    let parent = destination.parent().ok_or_else(|| {
        FsError::new(
            "invalid_archive_destination",
            "Unable to resolve the archive destination folder.",
            Some(destination.to_string_lossy().into_owned()),
        )
    })?;

    fs::create_dir_all(parent).map_err(|error| {
        FsError::io("Unable to create archive destination folder", parent, error)
    })?;

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

        fs::remove_file(destination).map_err(|error| {
            FsError::io("Unable to replace existing archive", destination, error)
        })?;
    }

    let destination_parent = fs::canonicalize(parent).map_err(|error| {
        FsError::io(
            "Unable to resolve archive destination folder",
            parent,
            error,
        )
    })?;
    let destination_name = destination.file_name().ok_or_else(|| {
        FsError::new(
            "invalid_archive_destination",
            "Unable to resolve the archive destination name.",
            Some(destination.to_string_lossy().into_owned()),
        )
    })?;
    let destination_absolute = destination_parent.join(destination_name);

    for source_path in source_paths {
        let metadata = fs::symlink_metadata(source_path).map_err(|error| {
            FsError::io("Unable to read archive source metadata", source_path, error)
        })?;
        let source_absolute = archive_source_absolute(source_path, &metadata)?;

        if source_absolute == destination_absolute {
            return Err(FsError::new(
                "archive_destination_matches_source",
                "The archive destination cannot be one of the selected files.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }

        if metadata.is_dir() && destination_absolute.starts_with(&source_absolute) {
            return Err(FsError::new(
                "archive_destination_inside_source",
                "The archive destination cannot be inside one of the selected folders.",
                Some(destination.to_string_lossy().into_owned()),
            ));
        }
    }

    Ok(())
}

fn archive_source_absolute(path: &Path, metadata: &fs::Metadata) -> FsResult<PathBuf> {
    if !metadata.file_type().is_symlink() {
        return fs::canonicalize(path)
            .map_err(|error| FsError::io("Unable to resolve archive source", path, error));
    }

    let parent = path.parent().ok_or_else(|| {
        FsError::new(
            "invalid_archive_source",
            "Unable to resolve the selected item folder.",
            Some(path.to_string_lossy().into_owned()),
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        FsError::new(
            "invalid_archive_source",
            "Unable to archive a path without a file name.",
            Some(path.to_string_lossy().into_owned()),
        )
    })?;
    let parent_absolute = if parent.as_os_str().is_empty() {
        std::env::current_dir().map_err(|error| {
            FsError::new(
                "invalid_archive_source",
                format!("Unable to resolve the selected item folder: {error}"),
                Some(path.to_string_lossy().into_owned()),
            )
        })?
    } else {
        fs::canonicalize(parent).map_err(|error| {
            FsError::io("Unable to resolve archive source folder", parent, error)
        })?
    };

    Ok(parent_absolute.join(file_name))
}

fn validate_zip_source(path: &Path) -> FsResult<()> {
    let metadata = fs::metadata(path)
        .map_err(|error| FsError::io("Unable to read zip archive metadata", path, error))?;

    if !metadata.is_file() {
        return Err(FsError::new(
            "invalid_archive_source",
            "Only zip files can be extracted.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    if extension != "zip" {
        return Err(FsError::new(
            "invalid_archive_source",
            "Only .zip archives can be extracted.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
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

fn zip_options(metadata: &fs::Metadata) -> FileOptions {
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);

    #[cfg(unix)]
    {
        options.unix_permissions(metadata.permissions().mode())
    }

    #[cfg(not(unix))]
    {
        options
    }
}

fn is_symlink_entry(entry: &ZipFile<'_>) -> bool {
    entry
        .unix_mode()
        .map(|mode| mode & 0o170000 == 0o120000)
        .unwrap_or(false)
}

fn zip_entry_name(path: &Path) -> FsResult<String> {
    let mut parts = Vec::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {
                return Err(FsError::new(
                    "invalid_archive_entry_name",
                    "Unable to create a safe zip entry path.",
                    Some(path.to_string_lossy().into_owned()),
                ));
            }
        }
    }

    if parts.is_empty() {
        return Err(FsError::new(
            "invalid_archive_entry_name",
            "Unable to create a zip entry without a file name.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(parts.join("/"))
}

fn zip_directory_name(path: &Path) -> FsResult<String> {
    let mut name = zip_entry_name(path)?;

    if !name.ends_with('/') {
        name.push('/');
    }

    Ok(name)
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

fn zip_error(action: &str, path: &Path, error: ZipError) -> FsError {
    match error {
        ZipError::Io(error) => FsError::io(action, path, error),
        _ => FsError::new(
            "archive_error",
            format!("{action}: {error}"),
            Some(path.to_string_lossy().into_owned()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("carelo-archive-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn archives_and_extracts_selected_items() {
        let root = test_root("round-trip");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directories");
        fs::write(root.join("note.txt"), "hello").expect("write source file");
        fs::write(root.join("folder").join("nested.txt"), "nested").expect("write nested file");

        let archive_path = root.join("bundle.zip");
        archive_items(
            &[
                root.join("note.txt").to_string_lossy().into_owned(),
                root.join("folder").to_string_lossy().into_owned(),
            ],
            &archive_path.to_string_lossy(),
            false,
        )
        .expect("create archive");

        let output_root = root.join("out");
        let extracted = unarchive_items(
            &[archive_path.to_string_lossy().into_owned()],
            &output_root.to_string_lossy(),
        )
        .expect("extract archive");

        let extracted_root = PathBuf::from(&extracted[0]);
        assert_eq!(
            fs::read_to_string(extracted_root.join("note.txt")).expect("read extracted file"),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(extracted_root.join("folder").join("nested.txt"))
                .expect("read nested extracted file"),
            "nested"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn skips_broken_symlinks_inside_archived_folders() {
        let root = test_root("broken-symlink");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).expect("create test directories");
        fs::write(root.join("folder").join("note.txt"), "hello").expect("write regular file");
        std::os::unix::fs::symlink("missing.txt", root.join("folder").join("broken-link"))
            .expect("create broken symlink");

        let archive_path = root.join("links.zip");
        archive_items(
            &[root.join("folder").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
        )
        .expect("create archive with broken symlink");

        let archive_file = File::open(&archive_path).expect("open archive");
        let mut archive = ZipArchive::new(archive_file).expect("read archive");
        assert!(archive.by_name("folder/").is_ok());
        assert!(archive.by_name("folder/note.txt").is_ok());
        assert!(archive.by_name("folder/broken-link").is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unsafe_extraction_paths() {
        let root = test_root("unsafe-path");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");

        let archive_path = root.join("unsafe.zip");
        let archive_file = File::create(&archive_path).expect("create unsafe archive");
        let mut archive = ZipWriter::new(archive_file);
        archive
            .start_file("../escape.txt", FileOptions::default())
            .expect("add unsafe entry");
        archive.write_all(b"bad").expect("write unsafe entry");
        archive.finish().expect("finish unsafe archive");

        let error = unarchive_items(
            &[archive_path.to_string_lossy().into_owned()],
            &root.join("out").to_string_lossy(),
        )
        .expect_err("unsafe archive should be rejected");

        assert_eq!(error.code, "unsafe_archive_entry");
        assert!(!root.join("escape.txt").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancels_archive_before_creating_output() {
        let root = test_root("cancel");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        fs::write(root.join("note.txt"), "hello").expect("write source file");

        let archive_path = root.join("cancelled.zip");
        let error = archive_items_with_progress(
            &[root.join("note.txt").to_string_lossy().into_owned()],
            &archive_path.to_string_lossy(),
            false,
            |_| {},
            || true,
        )
        .expect_err("cancelled archive should fail");

        assert_eq!(error.code, "operation_cancelled");
        assert!(!archive_path.exists());

        let _ = fs::remove_dir_all(&root);
    }
}
