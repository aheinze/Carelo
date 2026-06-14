use super::*;

const MAX_COMPARE_ENTRIES: usize = 20000;
// Filesystems store mtimes at different granularities (FAT is 2s); treat
// timestamps within this window as equal so they don't show as spurious diffs.
const MTIME_TOLERANCE_SECS: i64 = 2;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOptions {
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default = "default_compare_depth")]
    pub max_depth: usize,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            max_depth: default_compare_depth(),
        }
    }
}

fn default_compare_depth() -> usize {
    64
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSide {
    pub size: Option<u64>,
    pub modified_at: Option<u64>,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareEntry {
    pub relative_path: String,
    pub name: String,
    pub is_dir: bool,
    pub left: Option<CompareSide>,
    pub right: Option<CompareSide>,
    /// only_left | only_right | left_newer | right_newer | differs | type_conflict
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompareResult {
    pub left_root: String,
    pub right_root: String,
    pub entries: Vec<CompareEntry>,
    pub only_left: u64,
    pub only_right: u64,
    pub differing: u64,
    pub identical: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct ChildMeta {
    is_dir: bool,
    size: Option<u64>,
    modified_at: Option<u64>,
}

#[tauri::command]
pub async fn compare_directories(
    left: String,
    right: String,
    options: Option<CompareOptions>,
) -> Result<CompareResult, FsError> {
    let options = options.unwrap_or_default();
    run_local(move |_| compare_local_directories(&left, &right, &options)).await
}

fn compare_local_directories(
    left: &str,
    right: &str,
    options: &CompareOptions,
) -> FsResult<CompareResult> {
    if parse_remote_path(left).is_some()
        || parse_remote_path(right).is_some()
        || archive::is_archive_uri(left)
        || archive::is_archive_uri(right)
    {
        return Err(FsError::new(
            "compare_unsupported",
            "Folder comparison is available for local folders only.",
            None,
        ));
    }

    let left_root = expand_local_path(left)?;
    let right_root = expand_local_path(right)?;

    ensure_directory(&left_root)?;
    ensure_directory(&right_root)?;

    let mut ctx = CompareCtx {
        left_root: &left_root,
        right_root: &right_root,
        options,
        result: CompareResult {
            left_root: left_root.to_string_lossy().into_owned(),
            right_root: right_root.to_string_lossy().into_owned(),
            ..CompareResult::default()
        },
    };

    ctx.compare_dir("", 0)?;
    ctx.result
        .entries
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(ctx.result)
}

struct CompareCtx<'a> {
    left_root: &'a Path,
    right_root: &'a Path,
    options: &'a CompareOptions,
    result: CompareResult,
}

impl CompareCtx<'_> {
    fn compare_dir(&mut self, rel: &str, depth: usize) -> FsResult<()> {
        if self.result.truncated || depth > self.options.max_depth {
            return Ok(());
        }

        let left_children =
            read_children(&join_rel(self.left_root, rel), self.options.include_hidden);
        let right_children =
            read_children(&join_rel(self.right_root, rel), self.options.include_hidden);

        let mut names: Vec<&String> = left_children.keys().collect();
        for name in right_children.keys() {
            if !left_children.contains_key(name) {
                names.push(name);
            }
        }
        names.sort();

        for name in names {
            if self.result.truncated {
                return Ok(());
            }

            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let left = left_children.get(name);
            let right = right_children.get(name);

            match (left, right) {
                (Some(meta), None) => {
                    self.record("only_left", &child_rel, name, Some(meta), None);
                    self.result.only_left += 1;
                }
                (None, Some(meta)) => {
                    self.record("only_right", &child_rel, name, None, Some(meta));
                    self.result.only_right += 1;
                }
                (Some(left_meta), Some(right_meta)) => {
                    if left_meta.is_dir && right_meta.is_dir {
                        // Common directory — recurse, its contents are compared individually.
                        self.compare_dir(&child_rel, depth + 1)?;
                    } else if left_meta.is_dir != right_meta.is_dir {
                        self.record(
                            "type_conflict",
                            &child_rel,
                            name,
                            Some(left_meta),
                            Some(right_meta),
                        );
                        self.result.differing += 1;
                    } else {
                        match classify_files(left_meta, right_meta) {
                            Some(status) => {
                                self.record(
                                    status,
                                    &child_rel,
                                    name,
                                    Some(left_meta),
                                    Some(right_meta),
                                );
                                self.result.differing += 1;
                            }
                            None => self.result.identical += 1,
                        }
                    }
                }
                (None, None) => {}
            }
        }

        Ok(())
    }

    fn record(
        &mut self,
        status: &str,
        rel: &str,
        name: &str,
        left: Option<&ChildMeta>,
        right: Option<&ChildMeta>,
    ) {
        if self.result.entries.len() >= MAX_COMPARE_ENTRIES {
            self.result.truncated = true;
            return;
        }

        let is_dir = left
            .map(|m| m.is_dir)
            .or_else(|| right.map(|m| m.is_dir))
            .unwrap_or(false);
        self.result.entries.push(CompareEntry {
            relative_path: rel.to_string(),
            name: name.to_string(),
            is_dir,
            left: left.map(side_from_meta),
            right: right.map(side_from_meta),
            status: status.to_string(),
        });
    }
}

fn classify_files(left: &ChildMeta, right: &ChildMeta) -> Option<&'static str> {
    let same_size = left.size == right.size;
    let mtime_delta = match (left.modified_at, right.modified_at) {
        (Some(l), Some(r)) => Some(l as i64 - r as i64),
        _ => None,
    };
    let same_time = matches!(mtime_delta, Some(delta) if delta.abs() <= MTIME_TOLERANCE_SECS);

    if same_size && same_time {
        return None;
    }

    match mtime_delta {
        Some(delta) if delta > MTIME_TOLERANCE_SECS => Some("left_newer"),
        Some(delta) if delta < -MTIME_TOLERANCE_SECS => Some("right_newer"),
        _ => Some("differs"),
    }
}

fn side_from_meta(meta: &ChildMeta) -> CompareSide {
    CompareSide {
        size: meta.size,
        modified_at: meta.modified_at,
        is_dir: meta.is_dir,
    }
}

fn read_children(dir: &Path, include_hidden: bool) -> std::collections::HashMap<String, ChildMeta> {
    let mut map = std::collections::HashMap::new();
    let Ok(read) = fs::read_dir(dir) else {
        return map;
    };

    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();

        if !include_hidden && name.starts_with('.') {
            continue;
        }

        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        // Symlinks are treated as files so we never follow them while comparing.
        let is_dir = meta.is_dir() && !meta.file_type().is_symlink();
        let size = if is_dir { None } else { Some(meta.len()) };

        map.insert(
            name,
            ChildMeta {
                is_dir,
                size,
                modified_at: mtime_secs(&meta),
            },
        );
    }

    map
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

fn mtime_secs(meta: &fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn ensure_directory(path: &Path) -> FsResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| FsError::io("Unable to read folder for comparison", path, error))?;

    if !metadata.is_dir() {
        return Err(FsError::new(
            "compare_not_directory",
            "Folder comparison requires a folder on each side.",
            Some(path.to_string_lossy().into_owned()),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn set_mtime(path: &Path, secs: u64) {
        let file = fs::File::options()
            .write(true)
            .open(path)
            .expect("open for mtime");
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .expect("set mtime");
    }

    fn status_of<'a>(result: &'a CompareResult, rel: &str) -> Option<&'a str> {
        result
            .entries
            .iter()
            .find(|entry| entry.relative_path == rel)
            .map(|entry| entry.status.as_str())
    }

    #[test]
    fn classifies_directory_differences() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("carelo-compare-{}-{nonce}", std::process::id()));
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();

        // Identical: same content, same mtime.
        fs::write(left.join("same.txt"), "hello").unwrap();
        fs::write(right.join("same.txt"), "hello").unwrap();
        set_mtime(&left.join("same.txt"), 1_000_000);
        set_mtime(&right.join("same.txt"), 1_000_000);

        // Present on only one side.
        fs::write(left.join("left-only.txt"), "L").unwrap();
        fs::write(right.join("right-only.txt"), "R").unwrap();

        // Differs by size (same mtime).
        fs::write(left.join("diff.txt"), "aaaa").unwrap();
        fs::write(right.join("diff.txt"), "bb").unwrap();
        set_mtime(&left.join("diff.txt"), 1_000_000);
        set_mtime(&right.join("diff.txt"), 1_000_000);

        // Same size, left modified later → left_newer.
        fs::write(left.join("newer.txt"), "xy").unwrap();
        fs::write(right.join("newer.txt"), "xy").unwrap();
        set_mtime(&left.join("newer.txt"), 1_000_500);
        set_mtime(&right.join("newer.txt"), 1_000_000);

        // Common directory containing a left-only nested file.
        fs::create_dir_all(left.join("sub")).unwrap();
        fs::create_dir_all(right.join("sub")).unwrap();
        fs::write(left.join("sub/nested.txt"), "n").unwrap();

        let result = compare_local_directories(
            left.to_str().unwrap(),
            right.to_str().unwrap(),
            &CompareOptions::default(),
        )
        .expect("compare");

        assert_eq!(status_of(&result, "left-only.txt"), Some("only_left"));
        assert_eq!(status_of(&result, "right-only.txt"), Some("only_right"));
        assert_eq!(status_of(&result, "diff.txt"), Some("differs"));
        assert_eq!(status_of(&result, "newer.txt"), Some("left_newer"));
        // Nested file under a common directory is reported with its relative path.
        assert_eq!(status_of(&result, "sub/nested.txt"), Some("only_left"));
        // Identical file is counted, never listed.
        assert_eq!(status_of(&result, "same.txt"), None);

        assert_eq!(result.only_left, 2); // left-only.txt + sub/nested.txt
        assert_eq!(result.only_right, 1);
        assert_eq!(result.differing, 2); // diff.txt + newer.txt
        assert_eq!(result.identical, 1); // same.txt
        assert!(!result.truncated);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_files_excluded_by_default() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "carelo-compare-hidden-{}-{nonce}",
            std::process::id()
        ));
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join(".secret"), "x").unwrap();

        let hidden_off = compare_local_directories(
            left.to_str().unwrap(),
            right.to_str().unwrap(),
            &CompareOptions::default(),
        )
        .expect("compare");
        assert_eq!(hidden_off.entries.len(), 0);

        let hidden_on = compare_local_directories(
            left.to_str().unwrap(),
            right.to_str().unwrap(),
            &CompareOptions {
                include_hidden: true,
                max_depth: 64,
            },
        )
        .expect("compare");
        assert_eq!(status_of(&hidden_on, ".secret"), Some("only_left"));

        let _ = fs::remove_dir_all(&root);
    }
}
