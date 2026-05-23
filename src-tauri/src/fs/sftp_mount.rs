use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use url::Url;

use crate::fs::models::{FsError, FsResult};
use crate::fs::remote::RemoteVolumeConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SftpShare {
    host: String,
    port: Option<u16>,
    root: String,
    username: Option<String>,
    password: String,
}

pub fn is_password_sftp_config(config: &RemoteVolumeConfig) -> bool {
    config.scheme.eq_ignore_ascii_case("sftp")
        && config
            .options
            .get("password")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
}

pub fn sftp_password_fs_operator_options(
    config: &RemoteVolumeConfig,
) -> FsResult<Vec<(String, String)>> {
    let root = ensure_sftp_mount(config)?;
    Ok(vec![(
        "root".to_string(),
        root.to_string_lossy().into_owned(),
    )])
}

fn ensure_sftp_mount(config: &RemoteVolumeConfig) -> FsResult<PathBuf> {
    let share = parse_sftp_share(config)?;
    ensure_sftp_mount_for_platform(config, &share)
}

#[cfg(target_os = "linux")]
fn ensure_sftp_mount_for_platform(
    config: &RemoteVolumeConfig,
    share: &SftpShare,
) -> FsResult<PathBuf> {
    ensure_linux_gvfs_sftp_mount(config, share)
}

#[cfg(not(target_os = "linux"))]
fn ensure_sftp_mount_for_platform(
    config: &RemoteVolumeConfig,
    _share: &SftpShare,
) -> FsResult<PathBuf> {
    Err(FsError::new(
        "sftp_password_unsupported_platform",
        "SFTP password authentication is currently supported on Linux through GVFS only. Use SSH keys or an SSH agent on this platform.",
        Some(remote_config_path(config)),
    ))
}

#[cfg(target_os = "linux")]
fn ensure_linux_gvfs_sftp_mount(
    config: &RemoteVolumeConfig,
    share: &SftpShare,
) -> FsResult<PathBuf> {
    if let Some(path) = find_linux_gvfs_sftp_mount(share) {
        return Ok(join_sftp_root(path, &share.root));
    }

    let uri = sftp_mount_uri(share, true)?;
    let output = Command::new("gio")
        .arg("mount")
        .arg(&uri)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            FsError::new(
                "sftp_mount_spawn_failed",
                format!("Unable to start gio to mount the SFTP server: {error}"),
                Some(remote_config_path(config)),
            )
        })?;

    for _ in 0..20 {
        if let Some(path) = find_linux_gvfs_sftp_mount(share) {
            return Ok(join_sftp_root(path, &share.root));
        }

        thread::sleep(Duration::from_millis(150));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    let message = if !output.status.success() && !detail.is_empty() {
        format!("Unable to mount SFTP server through gio: {detail}")
    } else if !gvfs_root().is_dir() {
        "SFTP server mounted through GIO, but the GVFS FUSE mount is not available. Install/enable gvfs-fuse so Carelo can access the mounted server as a local path.".to_string()
    } else {
        "SFTP server mounted through GIO, but Carelo could not resolve its GVFS mount path."
            .to_string()
    };

    Err(FsError::new(
        "sftp_mount_path_not_found",
        message,
        Some(remote_config_path(config)),
    ))
}

#[cfg(target_os = "linux")]
fn find_linux_gvfs_sftp_mount(share: &SftpShare) -> Option<PathBuf> {
    let entries = std::fs::read_dir(gvfs_root()).ok()?;
    let host = share.host.to_ascii_lowercase();
    let username = share
        .username
        .as_ref()
        .map(|value| value.to_ascii_lowercase());
    let port = share.port.map(|value| value.to_string());

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(fields) = name.strip_prefix("sftp:") else {
            continue;
        };
        let fields = parse_gvfs_fields(fields);
        let mounted_host = fields
            .get("host")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();

        if mounted_host != host {
            continue;
        }

        if let Some(username) = username.as_deref() {
            let mounted_user = fields
                .get("user")
                .map(|value| value.to_ascii_lowercase())
                .unwrap_or_default();

            if mounted_user != username {
                continue;
            }
        }

        if let Some(port) = port.as_deref() {
            if fields.get("port").map(String::as_str) != Some(port) {
                continue;
            }
        }

        return Some(path);
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

fn parse_sftp_share(config: &RemoteVolumeConfig) -> FsResult<SftpShare> {
    if !config.scheme.eq_ignore_ascii_case("sftp") {
        return Err(FsError::new(
            "invalid_sftp_config",
            "Remote volume is not an SFTP volume.",
            Some(remote_config_path(config)),
        ));
    }

    let endpoint = config
        .options
        .get("endpoint")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            FsError::new(
                "invalid_sftp_config",
                "SFTP volumes require a server endpoint.",
                Some(remote_config_path(config)),
            )
        })?;
    let password = config
        .options
        .get("password")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            FsError::new(
                "invalid_sftp_config",
                "SFTP password authentication requires a password.",
                Some(remote_config_path(config)),
            )
        })?;
    let url = parse_sftp_endpoint(endpoint)?;
    let host = url.host_str().map(str::to_string).ok_or_else(|| {
        FsError::new(
            "invalid_sftp_config",
            "SFTP endpoint must include a host.",
            Some(endpoint.to_string()),
        )
    })?;
    let endpoint_root = normalize_remoteish_path(
        &url.path_segments()
            .map(|segments| {
                segments
                    .filter(|segment| !segment.is_empty())
                    .map(percent_decode_lossy)
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default(),
    );
    let config_root = config
        .root
        .as_deref()
        .map(normalize_remoteish_path)
        .unwrap_or_default();
    let root = join_remoteish_paths(&endpoint_root, &config_root);
    let username = config
        .options
        .get("user")
        .or_else(|| config.options.get("username"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let username = percent_decode_lossy(url.username()).trim().to_string();
            (!username.is_empty()).then_some(username)
        });
    let username = username.ok_or_else(|| {
        FsError::new(
            "invalid_sftp_config",
            "SFTP password authentication requires a username.",
            Some(remote_config_path(config)),
        )
    })?;

    Ok(SftpShare {
        host,
        port: url.port(),
        root,
        username: Some(username),
        password,
    })
}

fn parse_sftp_endpoint(endpoint: &str) -> FsResult<Url> {
    let normalized = if endpoint.starts_with("ssh://") || endpoint.starts_with("sftp://") {
        endpoint.to_string()
    } else {
        format!("sftp://{endpoint}")
    };
    let normalized = normalized.replace(' ', "%20");
    let url = Url::parse(&normalized).map_err(|error| {
        FsError::new(
            "invalid_sftp_config",
            format!("Invalid SFTP endpoint: {error}"),
            Some(endpoint.to_string()),
        )
    })?;

    if !matches!(url.scheme(), "sftp" | "ssh") {
        return Err(FsError::new(
            "invalid_sftp_config",
            "SFTP endpoint must start with sftp:// or ssh://.",
            Some(endpoint.to_string()),
        ));
    }

    Ok(url)
}

#[cfg(test)]
fn sftp_uri(share: &SftpShare, include_password: bool) -> FsResult<String> {
    let authority = sftp_authority(share, include_password);
    let path = if share.root.is_empty() {
        String::new()
    } else {
        format!("/{}", percent_encode(&share.root, false))
    };

    Ok(format!("sftp://{authority}{path}"))
}

fn sftp_mount_uri(share: &SftpShare, include_password: bool) -> FsResult<String> {
    Ok(format!(
        "sftp://{}",
        sftp_authority(share, include_password)
    ))
}

fn sftp_authority(share: &SftpShare, include_password: bool) -> String {
    let mut authority = String::new();

    if let Some(username) = share.username.as_deref().filter(|value| !value.is_empty()) {
        authority.push_str(&uri_encode_userinfo(username));

        if include_password {
            authority.push(':');
            authority.push_str(&uri_encode_userinfo(&share.password));
        }

        authority.push('@');
    }

    authority.push_str(&host_for_uri(&share.host));

    if let Some(port) = share.port {
        authority.push(':');
        authority.push_str(&port.to_string());
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

fn join_sftp_root(mount_root: PathBuf, root: &str) -> PathBuf {
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

    fn sftp_config(endpoint: &str, root: Option<&str>) -> RemoteVolumeConfig {
        RemoteVolumeConfig {
            id: "server".to_string(),
            name: "Server".to_string(),
            scheme: "sftp".to_string(),
            root: root.map(str::to_string),
            options: HashMap::from([
                ("endpoint".to_string(), endpoint.to_string()),
                ("user".to_string(), "artur".to_string()),
                ("password".to_string(), "secret value".to_string()),
            ]),
        }
    }

    #[test]
    fn detects_password_sftp_configs() {
        assert!(is_password_sftp_config(&sftp_config("example.com", None)));

        let mut key_config = sftp_config("example.com", None);
        key_config.options.remove("password");

        assert!(!is_password_sftp_config(&key_config));
    }

    #[test]
    fn parses_sftp_endpoint_and_roots() {
        let share = parse_sftp_share(&sftp_config(
            "ssh://files.example.com:2222/home/artur",
            Some("/projects/"),
        ))
        .expect("SFTP endpoint should parse");

        assert_eq!(share.host, "files.example.com");
        assert_eq!(share.port, Some(2222));
        assert_eq!(share.root, "home/artur/projects");
        assert_eq!(share.username.as_deref(), Some("artur"));
        assert_eq!(share.password, "secret value");
    }

    #[test]
    fn rejects_password_without_username() {
        let mut config = sftp_config("files.example.com:2222", None);
        config.options.remove("user");

        let error = parse_sftp_share(&config).expect_err("username should be required");

        assert_eq!(error.code, "invalid_sftp_config");
        assert!(error.message.contains("requires a username"));
    }

    #[test]
    fn builds_encoded_sftp_uri() {
        let share = parse_sftp_share(&sftp_config("files.example.com:2222", Some("Team Docs")))
            .expect("SFTP endpoint should parse");

        assert_eq!(
            sftp_uri(&share, true).expect("URI should build"),
            "sftp://artur:secret%20value@files.example.com:2222/Team%20Docs"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn decodes_gvfs_mount_fields() {
        let fields = parse_gvfs_fields("host=files.example.com,user=artur,port=2222");

        assert_eq!(
            fields.get("host").map(String::as_str),
            Some("files.example.com")
        );
        assert_eq!(fields.get("user").map(String::as_str), Some("artur"));
        assert_eq!(fields.get("port").map(String::as_str), Some("2222"));
    }
}
