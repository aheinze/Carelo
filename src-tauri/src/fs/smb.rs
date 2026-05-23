use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use url::Url;

use crate::fs::models::{FsError, FsResult};
use crate::fs::remote::RemoteVolumeConfig;

const SMB_SCHEMES: [&str; 2] = ["smb", "cifs"];
#[cfg(target_os = "linux")]
static OWNED_SMB_MOUNTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct SmbShare {
    server: String,
    port: Option<u16>,
    share: String,
    root: String,
    username: Option<String>,
    password: Option<String>,
    domain: Option<String>,
}

pub fn is_smb_scheme(scheme: &str) -> bool {
    SMB_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
}

pub fn smb_fs_operator_options(config: &RemoteVolumeConfig) -> FsResult<Vec<(String, String)>> {
    let root = ensure_smb_mount(config)?;
    Ok(vec![(
        "root".to_string(),
        root.to_string_lossy().into_owned(),
    )])
}

pub fn unmount_smb(config: &RemoteVolumeConfig) -> FsResult<bool> {
    let share = parse_smb_share(config)?;
    unmount_smb_for_platform(config, &share)
}

fn ensure_smb_mount(config: &RemoteVolumeConfig) -> FsResult<PathBuf> {
    let share = parse_smb_share(config)?;
    ensure_smb_mount_for_platform(config, &share)
}

#[cfg(target_os = "linux")]
fn ensure_smb_mount_for_platform(
    config: &RemoteVolumeConfig,
    share: &SmbShare,
) -> FsResult<PathBuf> {
    ensure_linux_gvfs_mount(config, share)
}

#[cfg(target_os = "macos")]
fn ensure_smb_mount_for_platform(
    config: &RemoteVolumeConfig,
    share: &SmbShare,
) -> FsResult<PathBuf> {
    ensure_macos_smb_mount(config, share)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn ensure_smb_mount_for_platform(
    config: &RemoteVolumeConfig,
    _share: &SmbShare,
) -> FsResult<PathBuf> {
    Err(FsError::new(
        "smb_unsupported_platform",
        "SMB/CIFS remote volumes are supported on Linux and macOS only.",
        Some(remote_config_path(config)),
    ))
}

#[cfg(target_os = "linux")]
fn unmount_smb_for_platform(config: &RemoteVolumeConfig, share: &SmbShare) -> FsResult<bool> {
    if !is_owned_smb_mount(&config.id) {
        return Ok(false);
    }

    if find_linux_gvfs_mount(share).is_none() {
        forget_owned_smb_mount(&config.id);
        return Ok(false);
    }

    let uri = smb_uri(share, "smb", false, true)?;
    let output = Command::new("gio")
        .arg("mount")
        .arg("-u")
        .arg(&uri)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            FsError::new(
                "smb_unmount_spawn_failed",
                format!("Unable to start gio to unmount the SMB share: {error}"),
                Some(remote_config_path(config)),
            )
        })?;

    for _ in 0..20 {
        if find_linux_gvfs_mount(share).is_none() {
            forget_owned_smb_mount(&config.id);
            return Ok(true);
        }

        thread::sleep(Duration::from_millis(150));
    }

    if output.status.success() && find_linux_gvfs_mount(share).is_none() {
        forget_owned_smb_mount(&config.id);
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    Err(FsError::new(
        "smb_unmount_failed",
        if detail.is_empty() {
            "Unable to unmount SMB share.".to_string()
        } else {
            format!("Unable to unmount SMB share: {detail}")
        },
        Some(remote_config_path(config)),
    ))
}

#[cfg(target_os = "macos")]
fn unmount_smb_for_platform(config: &RemoteVolumeConfig, _share: &SmbShare) -> FsResult<bool> {
    let mount_point = macos_mount_point(config)?;

    if !is_mounted_at(&mount_point)? {
        return Ok(false);
    }

    let output = Command::new("umount")
        .arg(&mount_point)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            FsError::new(
                "smb_unmount_spawn_failed",
                format!("Unable to start umount for the SMB share: {error}"),
                Some(remote_config_path(config)),
            )
        })?;

    if output.status.success() || !is_mounted_at(&mount_point)? {
        let _ = std::fs::remove_dir(&mount_point);
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    Err(FsError::new(
        "smb_unmount_failed",
        if detail.is_empty() {
            "Unable to unmount SMB share.".to_string()
        } else {
            format!("Unable to unmount SMB share: {detail}")
        },
        Some(remote_config_path(config)),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unmount_smb_for_platform(_config: &RemoteVolumeConfig, _share: &SmbShare) -> FsResult<bool> {
    Ok(false)
}

#[cfg(target_os = "linux")]
fn ensure_linux_gvfs_mount(config: &RemoteVolumeConfig, share: &SmbShare) -> FsResult<PathBuf> {
    if let Some(path) = find_linux_gvfs_mount(share) {
        return Ok(join_smb_root(path, &share.root));
    }

    let uri = smb_uri(share, "smb", true, true)?;
    let mut command = Command::new("gio");
    command
        .arg("mount")
        .arg(&uri)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output().map_err(|error| {
        FsError::new(
            "smb_mount_spawn_failed",
            format!("Unable to start gio to mount the SMB share: {error}"),
            Some(remote_config_path(config)),
        )
    })?;

    for _ in 0..20 {
        if let Some(path) = find_linux_gvfs_mount(share) {
            mark_owned_smb_mount(&config.id);
            return Ok(join_smb_root(path, &share.root));
        }

        thread::sleep(Duration::from_millis(150));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    let message = if !output.status.success() && !detail.is_empty() {
        format!("Unable to mount SMB share through gio: {detail}")
    } else if !gvfs_root().is_dir() {
        "SMB share mounted through GIO, but the GVFS FUSE mount is not available. Install/enable gvfs-fuse so Carelo can access the mounted share as a local path.".to_string()
    } else {
        "SMB share mounted through GIO, but Carelo could not resolve its GVFS mount path."
            .to_string()
    };

    Err(FsError::new(
        "smb_mount_path_not_found",
        message,
        Some(remote_config_path(config)),
    ))
}

#[cfg(target_os = "linux")]
fn find_linux_gvfs_mount(share: &SmbShare) -> Option<PathBuf> {
    let gvfs_root = gvfs_root();
    let entries = std::fs::read_dir(gvfs_root).ok()?;
    let server = share.server.to_ascii_lowercase();
    let share_name = share.share.to_ascii_lowercase();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(fields) = name.strip_prefix("smb-share:") else {
            continue;
        };
        let fields = parse_gvfs_fields(fields);
        let mounted_server = fields
            .get("server")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        let mounted_share = fields
            .get("share")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();

        if mounted_server == server && mounted_share == share_name {
            return Some(path);
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn parse_gvfs_fields(value: &str) -> HashMap<String, String> {
    value
        .split(',')
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.to_string(), percent_decode_lossy(value)))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn gvfs_root() -> PathBuf {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("gvfs");
    }

    PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() })).join("gvfs")
}

#[cfg(target_os = "linux")]
fn owned_smb_mounts() -> &'static Mutex<HashSet<String>> {
    OWNED_SMB_MOUNTS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(target_os = "linux")]
fn mark_owned_smb_mount(id: &str) {
    if let Ok(mut mounts) = owned_smb_mounts().lock() {
        mounts.insert(id.to_string());
    }
}

#[cfg(target_os = "linux")]
fn forget_owned_smb_mount(id: &str) {
    if let Ok(mut mounts) = owned_smb_mounts().lock() {
        mounts.remove(id);
    }
}

#[cfg(target_os = "linux")]
fn is_owned_smb_mount(id: &str) -> bool {
    owned_smb_mounts()
        .lock()
        .map(|mounts| mounts.contains(id))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn ensure_macos_smb_mount(config: &RemoteVolumeConfig, share: &SmbShare) -> FsResult<PathBuf> {
    let mount_point = macos_mount_point(config)?;
    std::fs::create_dir_all(&mount_point).map_err(|error| {
        FsError::io("Unable to create SMB mount directory", &mount_point, error)
    })?;

    if is_mounted_at(&mount_point)? {
        return Ok(join_smb_root(mount_point, &share.root));
    }

    let uri = smb_uri(share, "", true, true)?;
    let output = Command::new("mount_smbfs")
        .arg(&uri)
        .arg(&mount_point)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            FsError::new(
                "smb_mount_spawn_failed",
                format!("Unable to start mount_smbfs to mount the SMB share: {error}"),
                Some(remote_config_path(config)),
            )
        })?;

    if output.status.success() || is_mounted_at(&mount_point)? {
        return Ok(join_smb_root(mount_point, &share.root));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    Err(FsError::new(
        "smb_mount_failed",
        if detail.is_empty() {
            "Unable to mount SMB share.".to_string()
        } else {
            format!("Unable to mount SMB share: {detail}")
        },
        Some(remote_config_path(config)),
    ))
}

#[cfg(target_os = "macos")]
fn macos_mount_point(config: &RemoteVolumeConfig) -> FsResult<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        FsError::new(
            "home_directory_unavailable",
            "Unable to locate the home directory for SMB mounts.",
            Some(remote_config_path(config)),
        )
    })?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Caches")
        .join("Carelo")
        .join("smb-mounts")
        .join(sanitize_mount_name(&config.id)))
}

#[cfg(target_os = "macos")]
fn is_mounted_at(path: &std::path::Path) -> FsResult<bool> {
    let output = Command::new("mount")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            FsError::new(
                "mount_table_read_failed",
                format!("Unable to inspect mounted volumes: {error}"),
                Some(path.to_string_lossy().into_owned()),
            )
        })?;

    let target = path.to_string_lossy();
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains(&format!(" on {target} "))))
}

#[cfg(target_os = "macos")]
fn sanitize_mount_name(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();

    if cleaned.is_empty() {
        "smb-share".to_string()
    } else {
        cleaned
    }
}

fn parse_smb_share(config: &RemoteVolumeConfig) -> FsResult<SmbShare> {
    if !is_smb_scheme(&config.scheme) {
        return Err(FsError::new(
            "invalid_smb_config",
            "Remote volume is not an SMB/CIFS volume.",
            Some(remote_config_path(config)),
        ));
    }

    let endpoint = config
        .options
        .get("endpoint")
        .or_else(|| config.options.get("url"))
        .or_else(|| config.options.get("share"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            FsError::new(
                "invalid_smb_config",
                "SMB/CIFS volumes require a share URL such as smb://server/share.",
                Some(remote_config_path(config)),
            )
        })?;
    let url = parse_smb_endpoint(endpoint, config)?;
    let server = url.host_str().map(str::to_string).ok_or_else(|| {
        FsError::new(
            "invalid_smb_config",
            "SMB/CIFS share URL must include a server name.",
            Some(endpoint.to_string()),
        )
    })?;
    let mut segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(percent_decode_lossy)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if segments.is_empty() {
        return Err(FsError::new(
            "invalid_smb_config",
            "SMB/CIFS share URL must include a share name, for example smb://server/share.",
            Some(endpoint.to_string()),
        ));
    }

    let share = segments.remove(0);
    let endpoint_root = normalize_remoteish_path(&segments.join("/"));
    let config_root = config
        .root
        .as_deref()
        .map(normalize_remoteish_path)
        .unwrap_or_default();
    let root = join_remoteish_paths(&endpoint_root, &config_root);
    let username = option_value(&config.options, "username")
        .or_else(|| option_value(&config.options, "user"))
        .or_else(|| {
            let username = percent_decode_lossy(url.username()).trim().to_string();
            (!username.is_empty()).then_some(username)
        });
    let password = option_value(&config.options, "password")
        .or_else(|| url.password().map(percent_decode_lossy));
    let domain = option_value(&config.options, "domain")
        .or_else(|| option_value(&config.options, "workgroup"));

    Ok(SmbShare {
        server,
        port: url.port(),
        share,
        root,
        username,
        password,
        domain,
    })
}

fn parse_smb_endpoint(endpoint: &str, config: &RemoteVolumeConfig) -> FsResult<Url> {
    let normalized = if endpoint.starts_with("//") {
        format!("smb:{endpoint}")
    } else if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("smb://{endpoint}")
    };
    let normalized = normalized.replace(' ', "%20");
    let url = Url::parse(&normalized).map_err(|error| {
        FsError::new(
            "invalid_smb_config",
            format!("Invalid SMB/CIFS share URL: {error}"),
            Some(endpoint.to_string()),
        )
    })?;

    if !is_smb_scheme(url.scheme()) {
        return Err(FsError::new(
            "invalid_smb_config",
            "SMB/CIFS share URL must start with smb:// or cifs://.",
            Some(endpoint.to_string()),
        ));
    }

    if !is_smb_scheme(&config.scheme) {
        return Err(FsError::new(
            "invalid_smb_config",
            "Remote volume is not configured as SMB/CIFS.",
            Some(remote_config_path(config)),
        ));
    }

    Ok(url)
}

fn option_value(options: &HashMap<String, String>, key: &str) -> Option<String> {
    options
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn smb_uri(
    share: &SmbShare,
    scheme: &str,
    include_password: bool,
    include_port: bool,
) -> FsResult<String> {
    let authority = smb_authority(share, include_password, include_port);
    let path = uri_encode_component(&share.share);

    if scheme.is_empty() {
        Ok(format!("//{authority}/{path}"))
    } else {
        Ok(format!("{scheme}://{authority}/{path}"))
    }
}

fn smb_authority(share: &SmbShare, include_password: bool, include_port: bool) -> String {
    let mut authority = String::new();

    if let Some(username) = share.username.as_deref().filter(|value| !value.is_empty()) {
        let user = if let Some(domain) = share.domain.as_deref().filter(|value| !value.is_empty()) {
            format!("{domain};{username}")
        } else {
            username.to_string()
        };

        authority.push_str(&uri_encode_userinfo(&user));

        if include_password {
            if let Some(password) = share.password.as_deref() {
                authority.push(':');
                authority.push_str(&uri_encode_userinfo(password));
            }
        }

        authority.push('@');
    }

    authority.push_str(&host_for_uri(&share.server));

    if include_port {
        if let Some(port) = share.port {
            authority.push(':');
            authority.push_str(&port.to_string());
        }
    }

    authority
}

fn host_for_uri(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') && !host.ends_with(']') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn join_smb_root(mount_root: PathBuf, root: &str) -> PathBuf {
    if root.is_empty() {
        mount_root
    } else {
        root.split('/')
            .fold(mount_root, |path, segment| path.join(segment))
    }
}

fn remote_config_path(config: &RemoteVolumeConfig) -> String {
    format!("remote://{}/", config.id)
}

fn normalize_remoteish_path(path: &str) -> String {
    path.trim()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn join_remoteish_paths(left: &str, right: &str) -> String {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => String::new(),
        (true, false) => right.to_string(),
        (false, true) => left.to_string(),
        (false, false) => format!("{left}/{right}"),
    }
}

fn uri_encode_component(value: &str) -> String {
    percent_encode(value, false)
}

fn uri_encode_userinfo(value: &str) -> String {
    percent_encode(value, true)
}

fn percent_encode(value: &str, encode_slash: bool) -> String {
    let mut result = String::new();

    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (!encode_slash && byte == b'/');

        if keep {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }

    result
}

fn percent_decode_lossy(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smb_config(endpoint: &str, root: Option<&str>) -> RemoteVolumeConfig {
        RemoteVolumeConfig {
            id: "share".to_string(),
            name: "Share".to_string(),
            scheme: "smb".to_string(),
            root: root.map(str::to_string),
            options: HashMap::from([
                ("endpoint".to_string(), endpoint.to_string()),
                ("username".to_string(), "artur".to_string()),
                ("password".to_string(), "secret value".to_string()),
                ("domain".to_string(), "WORKGROUP".to_string()),
            ]),
        }
    }

    #[test]
    fn parses_smb_endpoint_and_roots() {
        let share = parse_smb_share(&smb_config(
            "smb://fileserver/Projects/Design%20Docs",
            Some("/Carelo/"),
        ))
        .expect("SMB endpoint should parse");

        assert_eq!(share.server, "fileserver");
        assert_eq!(share.share, "Projects");
        assert_eq!(share.root, "Design Docs/Carelo");
        assert_eq!(share.username.as_deref(), Some("artur"));
        assert_eq!(share.password.as_deref(), Some("secret value"));
        assert_eq!(share.domain.as_deref(), Some("WORKGROUP"));
    }

    #[test]
    fn builds_mount_uris_with_encoded_credentials() {
        let share = parse_smb_share(&smb_config("cifs://nas.local/Team Share", None))
            .expect("SMB endpoint should parse");

        assert_eq!(
            smb_uri(&share, "smb", true, true).expect("URI should build"),
            "smb://WORKGROUP%3Bartur:secret%20value@nas.local/Team%20Share"
        );
        assert_eq!(
            smb_uri(&share, "", false, false).expect("mount_smbfs URI should build"),
            "//WORKGROUP%3Bartur@nas.local/Team%20Share"
        );
    }

    #[test]
    fn accepts_unc_style_endpoints() {
        let share =
            parse_smb_share(&smb_config("//nas.local/archive", None)).expect("UNC should parse");

        assert_eq!(share.server, "nas.local");
        assert_eq!(share.share, "archive");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn decodes_gvfs_mount_fields() {
        let fields = parse_gvfs_fields("server=nas.local,share=Team%20Share,user=artur");

        assert_eq!(fields.get("server").map(String::as_str), Some("nas.local"));
        assert_eq!(fields.get("share").map(String::as_str), Some("Team Share"));
        assert_eq!(fields.get("user").map(String::as_str), Some("artur"));
    }
}
