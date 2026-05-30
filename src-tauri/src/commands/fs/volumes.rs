use super::*;

#[tauri::command]
pub async fn list_volumes(
    remotes: tauri::State<'_, RemoteVolumeState>,
) -> Result<Vec<VolumeEntry>, FsError> {
    let mut volumes = tauri::async_runtime::spawn_blocking(list_system_volumes)
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Volume scan failed: {error}"),
                None,
            )
        })??;

    volumes.extend(remotes.volume_entries()?);
    sort_volumes(&mut volumes);
    Ok(volumes)
}

#[tauri::command]
pub async fn mount_volume(device_path: String) -> Result<VolumeEntry, FsError> {
    let error_path = device_path.clone();

    tauri::async_runtime::spawn_blocking(move || mount_system_volume(&device_path))
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Volume mount failed: {error}"),
                Some(error_path),
            )
        })?
}

#[tauri::command]
pub async fn unlock_volume(device_path: String, password: String) -> Result<VolumeEntry, FsError> {
    let error_path = device_path.clone();

    tauri::async_runtime::spawn_blocking(move || unlock_system_volume(&device_path, &password))
        .await
        .map_err(|error| {
            FsError::new(
                "task_join_error",
                format!("Volume unlock failed: {error}"),
                Some(error_path),
            )
        })?
}

fn list_system_volumes() -> FsResult<Vec<VolumeEntry>> {
    #[cfg(target_os = "macos")]
    {
        return list_macos_volumes();
    }

    #[cfg(target_os = "linux")]
    {
        return list_linux_volumes();
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(Vec::new())
    }
}

fn mount_system_volume(device_path: &str) -> FsResult<VolumeEntry> {
    #[cfg(target_os = "linux")]
    {
        return mount_linux_volume(device_path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(FsError::new(
            "mount_volume_unsupported",
            "Mounting volumes from Carelo is not supported on this platform yet.",
            Some(device_path.to_string()),
        ))
    }
}

fn unlock_system_volume(device_path: &str, password: &str) -> FsResult<VolumeEntry> {
    #[cfg(target_os = "linux")]
    {
        return unlock_linux_volume(device_path, password);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = password;

        Err(FsError::new(
            "unlock_volume_unsupported",
            "Unlocking encrypted volumes from Carelo is not supported on this platform yet.",
            Some(device_path.to_string()),
        ))
    }
}

#[cfg(target_os = "linux")]
fn mounted_linux_volume_for_device(device_path: &str) -> FsResult<Option<VolumeEntry>> {
    Ok(list_linux_volumes()?.into_iter().find(|volume| {
        volume
            .device_path
            .as_deref()
            .is_some_and(|mounted_device_path| {
                linux_device_paths_match(mounted_device_path, device_path)
            })
            && volume.is_mounted
            && !volume.path.is_empty()
    }))
}

#[cfg(target_os = "linux")]
fn mount_linux_volume(device_path: &str) -> FsResult<VolumeEntry> {
    let device_path = device_path.trim();

    if !device_path.starts_with("/dev/") {
        return Err(FsError::new(
            "invalid_volume_device",
            "Volume device path must start with /dev/.",
            Some(device_path.to_string()),
        ));
    }

    if linux_block_device_is_encrypted(device_path) {
        if let Some(unlocked_device_path) = unlocked_linux_device_for_device(device_path) {
            return mount_linux_volume(&unlocked_device_path);
        }
    }

    if let Some(volume) = mounted_linux_volume_for_device(device_path)? {
        return Ok(volume);
    }

    let output = Command::new("udisksctl")
        .args(["mount", "-b", device_path])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            FsError::new(
                "mount_volume_spawn_failed",
                format!("Unable to start udisksctl to mount the volume: {error}"),
                Some(device_path.to_string()),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(FsError::new(
            "mount_volume_failed",
            if detail.is_empty() {
                "Unable to mount the volume.".to_string()
            } else {
                format!("Unable to mount the volume: {detail}")
            },
            Some(device_path.to_string()),
        ));
    }

    for _ in 0..20 {
        if let Some(volume) = mounted_linux_volume_for_device(device_path)? {
            return Ok(volume);
        }

        thread::sleep(Duration::from_millis(150));
    }

    Err(FsError::new(
        "mount_volume_path_not_found",
        "The volume mounted, but Carelo could not find its mount path yet.",
        Some(device_path.to_string()),
    ))
}

#[cfg(target_os = "linux")]
fn unlock_linux_volume(device_path: &str, password: &str) -> FsResult<VolumeEntry> {
    let device_path = device_path.trim();

    if !device_path.starts_with("/dev/") {
        return Err(FsError::new(
            "invalid_volume_device",
            "Volume device path must start with /dev/.",
            Some(device_path.to_string()),
        ));
    }

    if password.is_empty() {
        return Err(FsError::new(
            "volume_unlock_password_required",
            "A password is required to unlock this encrypted volume.",
            Some(device_path.to_string()),
        ));
    }

    if let Some(unlocked_device_path) = unlocked_linux_device_for_device(device_path) {
        return mount_linux_volume(&unlocked_device_path);
    }

    let key_file = write_udisks_unlock_key_file(password, device_path)?;
    let output_result = Command::new("udisksctl")
        .args(["unlock", "-b", device_path, "--key-file"])
        .arg(&key_file)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            FsError::new(
                "unlock_volume_spawn_failed",
                format!("Unable to start udisksctl to unlock the volume: {error}"),
                Some(device_path.to_string()),
            )
        });
    let _ = std::fs::remove_file(&key_file);
    let output = output_result?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(unlock_volume_failed_error(device_path, &detail));
    }

    if let Some(unlocked_device_path) = unlocked_device_from_udisks_output(&output.stdout) {
        return mount_linux_volume(&unlocked_device_path);
    }

    for _ in 0..20 {
        if let Some(unlocked_device_path) = unlocked_linux_device_for_device(device_path) {
            return mount_linux_volume(&unlocked_device_path);
        }

        thread::sleep(Duration::from_millis(150));
    }

    Err(FsError::new(
        "unlock_volume_path_not_found",
        "The volume unlocked, but Carelo could not find its cleartext device yet.",
        Some(device_path.to_string()),
    ))
}

#[cfg(target_os = "linux")]
fn write_udisks_unlock_key_file(password: &str, device_path: &str) -> FsResult<PathBuf> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    for _ in 0..16 {
        let file_name = format!(
            "carelo-udisks-key-{}-{}",
            std::process::id(),
            Alphanumeric.sample_string(&mut rand::rng(), 18)
        );
        let path = std::env::temp_dir().join(file_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);

        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(FsError::new(
                    "unlock_key_file_failed",
                    format!("Unable to prepare the encrypted volume password: {error}"),
                    Some(device_path.to_string()),
                ));
            }
        };

        if let Err(error) = file
            .write_all(password.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(FsError::new(
                "unlock_key_file_failed",
                format!("Unable to prepare the encrypted volume password: {error}"),
                Some(device_path.to_string()),
            ));
        }

        return Ok(path);
    }

    Err(FsError::new(
        "unlock_key_file_failed",
        "Unable to prepare the encrypted volume password.",
        Some(device_path.to_string()),
    ))
}

#[cfg(target_os = "linux")]
fn unlock_volume_failed_error(device_path: &str, detail: &str) -> FsError {
    let lower = detail.to_lowercase();
    let code = if lower.contains("incorrect passphrase")
        || lower.contains("no key available")
        || lower.contains("not a valid passphrase")
        || lower.contains("failed to activate device")
        || lower.contains("authentication failed")
    {
        "volume_unlock_auth_failed"
    } else {
        "unlock_volume_failed"
    };

    FsError::new(
        code,
        if code == "volume_unlock_auth_failed" {
            "The encrypted volume password was not accepted.".to_string()
        } else if detail.is_empty() {
            "Unable to unlock the encrypted volume.".to_string()
        } else {
            format!("Unable to unlock the encrypted volume: {detail}")
        },
        Some(device_path.to_string()),
    )
}

#[cfg(target_os = "linux")]
fn unlocked_device_from_udisks_output(stdout: &[u8]) -> Option<String> {
    let output = String::from_utf8_lossy(stdout);
    let (_, rest) = output.split_once(" as ")?;
    let device_path = rest
        .trim()
        .trim_end_matches('.')
        .split_whitespace()
        .next()?
        .trim_end_matches('.');

    device_path
        .starts_with("/dev/")
        .then(|| device_path.to_string())
}

#[cfg(target_os = "macos")]
fn list_macos_volumes() -> FsResult<Vec<VolumeEntry>> {
    let volumes_dir = Path::new("/Volumes");
    let entries = match std::fs::read_dir(volumes_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FsError::io("Read mounted volumes", volumes_dir, error)),
    };

    let root = std::fs::canonicalize("/").ok();
    let mut volumes = Vec::new();

    for entry in entries {
        let entry =
            entry.map_err(|error| FsError::io("Read mounted volume", volumes_dir, error))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        if let (Some(root), Ok(canonical_path)) = (root.as_ref(), std::fs::canonicalize(&path)) {
            if &canonical_path == root {
                continue;
            }
        }

        let name = entry.file_name().to_string_lossy().into_owned();

        if name.is_empty() {
            continue;
        }

        volumes.push(VolumeEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            device_path: None,
            detail: capacity_detail(&path),
            is_removable: true,
            is_mounted: true,
            is_encrypted: false,
            needs_unlock: false,
            capabilities: None,
            health: None,
        });
    }

    sort_volumes(&mut volumes);
    Ok(volumes)
}

#[cfg(target_os = "linux")]
fn list_linux_volumes() -> FsResult<Vec<VolumeEntry>> {
    let mountinfo_path = Path::new("/proc/self/mountinfo");
    let mountinfo = match std::fs::read_to_string(mountinfo_path) {
        Ok(mountinfo) => mountinfo,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FsError::io("Read mount table", mountinfo_path, error)),
    };

    let mount_points_by_device = linux_mount_points_by_device(&mountinfo);
    let mut seen_paths = HashSet::new();
    let mut seen_devices = HashSet::new();
    let mut volumes = Vec::new();

    for line in mountinfo.lines() {
        if let Some(volume) = volume_from_mountinfo_line(line) {
            if let Some(device_path) = volume.device_path.as_ref() {
                seen_devices.insert(device_path.clone());
            }

            if seen_paths.insert(volume.path.clone()) {
                volumes.push(volume);
            }
        }
    }

    for volume in attached_linux_block_volumes(&mount_points_by_device)? {
        let Some(device_path) = volume.device_path.as_ref() else {
            continue;
        };

        if seen_devices.insert(device_path.clone()) {
            if volume.path.is_empty() || seen_paths.insert(volume.path.clone()) {
                volumes.push(volume);
            }
        }
    }

    sort_volumes(&mut volumes);
    Ok(volumes)
}

#[cfg(target_os = "linux")]
fn volume_from_mountinfo_line(line: &str) -> Option<VolumeEntry> {
    let fields: Vec<&str> = line.split(' ').collect();
    let separator_index = fields.iter().position(|field| *field == "-")?;
    let mount_point = decode_mountinfo_escape(fields.get(4)?);
    let fs_type = fields.get(separator_index + 1)?;
    let source = decode_mountinfo_escape(fields.get(separator_index + 2).copied().unwrap_or(""));

    if is_pseudo_filesystem(fs_type) {
        return None;
    }

    let path = PathBuf::from(&mount_point);

    if (!is_user_visible_mount(&path) && !is_removable_device_source(&source)) || !path.is_dir() {
        return None;
    }

    Some(VolumeEntry {
        name: mounted_volume_name(&path, &source),
        path: mount_point,
        device_path: source.starts_with("/dev/").then_some(source),
        detail: capacity_detail(&path),
        is_removable: true,
        is_mounted: true,
        is_encrypted: false,
        needs_unlock: false,
        capabilities: None,
        health: None,
    })
}

#[cfg(target_os = "linux")]
fn linux_mount_points_by_device(mountinfo: &str) -> HashMap<String, String> {
    let mut mount_points = HashMap::new();

    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(separator_index) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        let Some(mount_point) = fields.get(4) else {
            continue;
        };
        let source =
            decode_mountinfo_escape(fields.get(separator_index + 2).copied().unwrap_or(""));

        if !source.starts_with("/dev/") {
            continue;
        }

        let device_path = strip_mount_source_suffix(&source);
        let mount_point = decode_mountinfo_escape(mount_point);
        mount_points.insert(device_path.clone(), mount_point.clone());
        mount_points.insert(canonical_linux_device_path(&device_path), mount_point);
    }

    mount_points
}

#[cfg(target_os = "linux")]
fn linux_device_paths_match(left: &str, right: &str) -> bool {
    let left = strip_mount_source_suffix(left);
    let right = strip_mount_source_suffix(right);

    left == right || canonical_linux_device_path(&left) == canonical_linux_device_path(&right)
}

#[cfg(target_os = "linux")]
fn canonical_linux_device_path(device_path: &str) -> String {
    std::fs::canonicalize(device_path)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| device_path.to_string())
}

#[cfg(target_os = "linux")]
fn strip_mount_source_suffix(source: &str) -> String {
    source
        .split_once('[')
        .map(|(device, _)| device.to_string())
        .unwrap_or_else(|| source.to_string())
}

#[cfg(target_os = "linux")]
fn attached_linux_block_volumes(
    mount_points_by_device: &HashMap<String, String>,
) -> FsResult<Vec<VolumeEntry>> {
    let block_dir = Path::new("/sys/class/block");
    let entries = match std::fs::read_dir(block_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FsError::io("Read block devices", block_dir, error)),
    };

    let mut volumes = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| FsError::io("Read block device", block_dir, error))?;
        let device_name = entry.file_name().to_string_lossy().into_owned();

        if should_skip_block_device_name(&device_name) || !is_linux_usb_block_device(&device_name) {
            continue;
        }

        let device_path = format!("/dev/{device_name}");
        let properties = read_udev_properties(&device_name);
        let filesystem_type = properties
            .get("ID_FS_TYPE")
            .filter(|value| !value.is_empty());
        let is_encrypted = linux_udev_properties_are_encrypted(&properties);
        let unlocked_device_path = is_encrypted
            .then(|| unlocked_linux_device_for_device(&device_path))
            .flatten();
        let mount_device_path = unlocked_device_path.as_deref().unwrap_or(&device_path);

        if filesystem_type.is_none() && block_device_has_children(&entry.path()) {
            continue;
        }

        if filesystem_type.is_none() && !mount_points_by_device.contains_key(mount_device_path) {
            continue;
        }

        let mount_path = mount_points_by_device
            .get(mount_device_path)
            .filter(|path| Path::new(path.as_str()).is_dir())
            .cloned();
        let size = block_device_size(&device_name);
        let needs_unlock = is_encrypted && unlocked_device_path.is_none();
        let detail = if needs_unlock {
            size.map(|bytes| format!("{} • Encrypted • Locked", format_bytes(bytes)))
                .or_else(|| Some("Encrypted • Locked".to_string()))
        } else if is_encrypted && mount_path.is_none() {
            size.map(|bytes| format!("{} • Encrypted • Unlocked", format_bytes(bytes)))
                .or_else(|| Some("Encrypted • Unlocked".to_string()))
        } else if let Some(path) = mount_path.as_ref() {
            capacity_detail(Path::new(path))
                .or_else(|| size.map(|bytes| format!("{} mounted", format_bytes(bytes))))
        } else {
            size.map(|bytes| format!("{} • Not mounted", format_bytes(bytes)))
                .or_else(|| Some("Not mounted".to_string()))
        };

        volumes.push(VolumeEntry {
            name: attached_volume_name(&device_name, &properties),
            path: mount_path.clone().unwrap_or_default(),
            device_path: Some(device_path),
            detail,
            is_removable: true,
            is_mounted: mount_path.is_some(),
            is_encrypted,
            needs_unlock,
            capabilities: None,
            health: None,
        });
    }

    Ok(volumes)
}

#[cfg(target_os = "linux")]
fn should_skip_block_device_name(device_name: &str) -> bool {
    device_name.starts_with("loop")
        || device_name.starts_with("ram")
        || device_name.starts_with("zram")
        || device_name.starts_with("dm-")
}

#[cfg(target_os = "linux")]
fn block_device_has_children(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .map(|name| entry.path().join("partition").exists() || name.starts_with("dm-"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn linux_block_device_is_encrypted(device_path: &str) -> bool {
    device_name_from_device_path(device_path)
        .map(|device_name| read_udev_properties(&device_name))
        .map(|properties| linux_udev_properties_are_encrypted(&properties))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn linux_udev_properties_are_encrypted(properties: &HashMap<String, String>) -> bool {
    properties
        .get("ID_FS_USAGE")
        .map(|value| value.eq_ignore_ascii_case("crypto"))
        .unwrap_or(false)
        || properties
            .get("ID_FS_TYPE")
            .map(|value| {
                let value = value.to_lowercase();
                value.contains("crypto") || value.contains("luks")
            })
            .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn unlocked_linux_device_for_device(device_path: &str) -> Option<String> {
    let device_name = device_name_from_device_path(device_path)?;
    let holders_path = Path::new("/sys/class/block")
        .join(device_name)
        .join("holders");
    let entries = std::fs::read_dir(holders_path).ok()?;

    entries.filter_map(Result::ok).find_map(|entry| {
        let holder_name = entry.file_name().to_string_lossy().into_owned();

        (!holder_name.is_empty()).then(|| format!("/dev/{holder_name}"))
    })
}

#[cfg(target_os = "linux")]
fn device_name_from_device_path(device_path: &str) -> Option<String> {
    Path::new(device_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(target_os = "linux")]
fn is_linux_usb_block_device(device_name: &str) -> bool {
    let properties = read_udev_properties(device_name);

    if properties
        .get("ID_BUS")
        .or_else(|| properties.get("ID_USB_TYPE"))
        .map(|value| value == "usb" || value == "disk")
        .unwrap_or(false)
    {
        return true;
    }

    std::fs::canonicalize(Path::new("/sys/class/block").join(device_name))
        .map(|path| path.to_string_lossy().contains("/usb"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn read_udev_properties(device_name: &str) -> HashMap<String, String> {
    let dev_path = Path::new("/sys/class/block").join(device_name).join("dev");
    let Ok(dev_id) = std::fs::read_to_string(dev_path) else {
        return HashMap::new();
    };
    let data_path = Path::new("/run/udev/data").join(format!("b{}", dev_id.trim()));
    let Ok(data) = std::fs::read_to_string(data_path) else {
        return HashMap::new();
    };

    data.lines()
        .filter_map(|line| line.strip_prefix("E:"))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), decode_udev_value(value)))
        .collect()
}

#[cfg(target_os = "linux")]
fn decode_udev_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1] == b'x'
            && bytes[index + 2].is_ascii_hexdigit()
            && bytes[index + 3].is_ascii_hexdigit()
        {
            if let Some(byte) = hex_byte(bytes[index + 2], bytes[index + 3]) {
                decoded.push(byte);
                index += 4;
                continue;
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(target_os = "linux")]
fn hex_byte(left: u8, right: u8) -> Option<u8> {
    Some(hex_nibble(left)? * 16 + hex_nibble(right)?)
}

#[cfg(target_os = "linux")]
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn block_device_size(device_name: &str) -> Option<u64> {
    let size_path = Path::new("/sys/class/block").join(device_name).join("size");
    let sectors = std::fs::read_to_string(size_path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;

    Some(sectors.saturating_mul(512))
}

#[cfg(target_os = "linux")]
fn attached_volume_name(device_name: &str, properties: &HashMap<String, String>) -> String {
    properties
        .get("ID_FS_LABEL")
        .or_else(|| properties.get("ID_PART_ENTRY_NAME"))
        .or_else(|| properties.get("ID_MODEL"))
        .map(|name| readable_device_name(name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| device_name.to_string())
}

#[cfg(target_os = "linux")]
fn readable_device_name(name: &str) -> String {
    name.trim().replace('_', " ")
}

#[cfg(target_os = "linux")]
fn is_user_visible_mount(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_owned))
        .filter(|part| !part.is_empty() && part != "/")
        .collect();

    match parts.as_slice() {
        [run, media, _user, _volume, ..] if run == "run" && media == "media" => true,
        [media, _volume, ..] if media == "media" => true,
        [mnt, _volume, ..] if mnt == "mnt" => true,
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn is_pseudo_filesystem(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "autofs"
            | "bpf"
            | "cgroup"
            | "cgroup2"
            | "configfs"
            | "debugfs"
            | "devpts"
            | "devtmpfs"
            | "efivarfs"
            | "fusectl"
            | "mqueue"
            | "overlay"
            | "proc"
            | "pstore"
            | "securityfs"
            | "squashfs"
            | "sysfs"
            | "tmpfs"
            | "tracefs"
    )
}

#[cfg(target_os = "linux")]
fn is_removable_device_source(source: &str) -> bool {
    if !source.starts_with("/dev/") {
        return false;
    }

    let source_path = std::fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source));
    let Some(device_name) = source_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    is_linux_usb_block_device(device_name)
        || block_device_candidates(device_name)
            .iter()
            .any(|candidate| is_removable_block_device(candidate))
}

#[cfg(target_os = "linux")]
fn block_device_candidates(device_name: &str) -> Vec<String> {
    let mut candidates = vec![device_name.to_string()];
    let base = device_name.trim_end_matches(|character: char| character.is_ascii_digit());

    if base != device_name && !base.is_empty() {
        candidates.push(base.trim_end_matches('p').to_string());
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

#[cfg(target_os = "linux")]
fn is_removable_block_device(device_name: &str) -> bool {
    if device_name.is_empty() {
        return false;
    }

    let removable_path = Path::new("/sys/class/block")
        .join(device_name)
        .join("removable");

    std::fs::read_to_string(removable_path)
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn mounted_volume_name(path: &Path, source: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| source.to_string())
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_escape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            if let Some(byte) = octal_byte(&bytes[index + 1..index + 4]) {
                decoded.push(byte);
                index += 4;
                continue;
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(target_os = "linux")]
fn octal_byte(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 3 || !bytes.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
        return None;
    }

    let value = u16::from(bytes[0] - b'0') * 64
        + u16::from(bytes[1] - b'0') * 8
        + u16::from(bytes[2] - b'0');

    u8::try_from(value).ok()
}

fn sort_volumes(volumes: &mut [VolumeEntry]) {
    volumes.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
}

#[cfg(unix)]
fn capacity_detail(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };

    if result != 0 {
        return None;
    }

    let stats = unsafe { stats.assume_init() };
    let block_size = if stats.f_frsize > 0 {
        stats.f_frsize
    } else {
        stats.f_bsize
    } as u64;
    let available = (stats.f_bavail as u64).saturating_mul(block_size);

    Some(format!("{} available", format_bytes(available)))
}

#[cfg(not(unix))]
fn capacity_detail(_path: &Path) -> Option<String> {
    None
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} bytes");
    }

    let mut value = bytes as f64;
    let mut unit_index = 0;

    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parses_udisks_unlock_cleartext_device_path() {
        assert_eq!(
            unlocked_device_from_udisks_output(b"Unlocked /dev/sdb1 as /dev/dm-3.\n"),
            Some("/dev/dm-3".to_string())
        );
    }

    #[test]
    fn classifies_rejected_luks_passwords() {
        let error = unlock_volume_failed_error(
            "/dev/sdb1",
            "Error unlocking /dev/sdb1: Failed to activate device: Incorrect passphrase.",
        );

        assert_eq!(error.code, "volume_unlock_auth_failed");
        assert_eq!(
            error.message,
            "The encrypted volume password was not accepted."
        );
    }

    #[test]
    fn detects_encrypted_udev_volume_properties() {
        let properties = HashMap::from([
            ("ID_FS_TYPE".to_string(), "crypto_LUKS".to_string()),
            ("ID_FS_USAGE".to_string(), "crypto".to_string()),
        ]);

        assert!(linux_udev_properties_are_encrypted(&properties));
    }
}
