//! Storage-class detection that decides how aggressively a file operation may
//! run in parallel.
//!
//! Parallel I/O speeds up SSD/NVMe and (especially) remote transfers, but it
//! *hurts* spinning disks because concurrent requests cause seek thrashing. So
//! the safe default is to stay serial whenever we can't prove the target is
//! solid-state. Detection is Linux-focused (reads `/proc/self/mountinfo` plus
//! sysfs); anything we can't classify falls back to `Unknown`, which keeps
//! concurrency conservative.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    /// Spinning disk — parallel access causes seek thrashing, so stay serial.
    Rotational,
    /// SSD/NVMe — benefits from a small amount of parallelism.
    Solid,
    /// Network/remote volume — latency-bound, so high concurrency hides RTT.
    Remote,
    /// Couldn't be determined — use a conservative amount of parallelism.
    Unknown,
}

impl StorageClass {
    /// Default number of concurrent workers for this storage class, before any
    /// user override is applied.
    pub fn default_concurrency(self) -> usize {
        match self {
            StorageClass::Rotational => 1,
            StorageClass::Solid => 4,
            StorageClass::Remote => 8,
            StorageClass::Unknown => 2,
        }
    }
}

/// Resolve the effective worker count for a storage class given an optional
/// user override (a cap; `Some(1)` means "off"/serial, `None` means "auto").
pub fn resolve_concurrency(class: StorageClass, max_override: Option<usize>) -> usize {
    let base = class.default_concurrency();

    match max_override {
        Some(max) => base.min(max).max(1),
        None => base.max(1),
    }
}

/// Classify the storage backing `path`. The path need not exist yet (a copy
/// destination, say); its nearest existing ancestor is used instead.
pub fn classify_path(path: &str) -> StorageClass {
    if path.starts_with("remote://") {
        return StorageClass::Remote;
    }

    #[cfg(target_os = "linux")]
    {
        linux::classify(std::path::Path::new(path))
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Without a portable rotational query we stay conservative. Modern Macs
        // are all-SSD, but we don't special-case that here.
        let _ = path;
        StorageClass::Unknown
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::StorageClass;
    use std::path::{Path, PathBuf};

    pub(super) fn classify(path: &Path) -> StorageClass {
        let target = resolve_existing(path);

        match backing_source(&target)
            .as_deref()
            .and_then(device_is_rotational)
        {
            Some(true) => StorageClass::Rotational,
            Some(false) => StorageClass::Solid,
            None => StorageClass::Unknown,
        }
    }

    /// Walk up until a component exists, then canonicalize it. This lets us
    /// classify a not-yet-created destination by the directory it will live in,
    /// and resolves symlinked mount points to their real location.
    fn resolve_existing(path: &Path) -> PathBuf {
        let mut current = path;

        loop {
            if let Ok(canonical) = std::fs::canonicalize(current) {
                return canonical;
            }

            match current.parent() {
                Some(parent) => current = parent,
                None => return path.to_path_buf(),
            }
        }
    }

    fn backing_source(target: &Path) -> Option<String> {
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        select_mount_source(&mountinfo, target)
    }

    /// Find the mount source (e.g. `/dev/nvme0n1p3`) of the mount point that is
    /// the longest path-prefix of `target`. Pure so it can be unit-tested with
    /// synthetic `mountinfo` content. Works for btrfs/zfs/overlay whose `st_dev`
    /// is an anonymous device that doesn't appear under `/sys`.
    fn select_mount_source(mountinfo: &str, target: &Path) -> Option<String> {
        let target = target.to_string_lossy();
        let mut best: Option<(usize, String)> = None;

        for line in mountinfo.lines() {
            let Some((mount_point, source)) = parse_mountinfo_line(line) else {
                continue;
            };

            if path_has_prefix(&target, &mount_point) {
                let length = mount_point.len();

                if best
                    .as_ref()
                    .map_or(true, |(best_len, _)| length > *best_len)
                {
                    best = Some((length, source));
                }
            }
        }

        best.map(|(_, source)| source)
    }

    /// `/proc/self/mountinfo` line layout (see proc(5)):
    /// `id parent major:minor root MOUNT_POINT options [optional...] - fstype SOURCE superopts`
    fn parse_mountinfo_line(line: &str) -> Option<(String, String)> {
        let (before, after) = line.split_once(" - ")?;
        let mount_point = before.split_whitespace().nth(4)?;
        let source = after.split_whitespace().nth(1)?;
        Some((unescape_octal(mount_point), unescape_octal(source)))
    }

    /// mountinfo escapes space/tab/newline/backslash as octal (e.g. `\040`).
    fn unescape_octal(value: &str) -> String {
        if !value.contains('\\') {
            return value.to_string();
        }

        let mut out = String::with_capacity(value.len());
        let bytes = value.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 3 < bytes.len() {
                let octal = &value[i + 1..i + 4];
                if let Ok(code) = u8::from_str_radix(octal, 8) {
                    out.push(code as char);
                    i += 4;
                    continue;
                }
            }

            out.push(bytes[i] as char);
            i += 1;
        }

        out
    }

    fn path_has_prefix(path: &str, prefix: &str) -> bool {
        if prefix == "/" {
            return path.starts_with('/');
        }

        path == prefix || path.starts_with(&format!("{prefix}/"))
    }

    /// Resolve a mount source device to its rotational flag. Returns `None` for
    /// non-block sources (tmpfs, nfs, overlay) or when sysfs can't be read.
    fn device_is_rotational(source: &str) -> Option<bool> {
        if !source.starts_with("/dev/") {
            return None;
        }

        let real = std::fs::canonicalize(source).ok()?;
        let name = real.file_name()?.to_str()?;

        // Whole disks and dm/loop devices expose `queue/` directly.
        if let Some(rotational) =
            read_rotational_flag(&format!("/sys/block/{name}/queue/rotational"))
        {
            return Some(rotational);
        }

        // Partitions don't; resolve via /sys/class/block and read the parent
        // disk's flag instead.
        let resolved = std::fs::canonicalize(format!("/sys/class/block/{name}")).ok()?;
        let parent = resolved.parent()?;
        read_rotational_flag(&parent.join("queue/rotational").to_string_lossy())
    }

    fn read_rotational_flag(path: &str) -> Option<bool> {
        match std::fs::read_to_string(path).ok()?.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::Path;

        const SAMPLE: &str = "\
22 28 0:21 / /proc rw,nosuid - proc proc rw
24 28 0:6 / /dev rw,nosuid - devtmpfs devtmpfs rw
26 28 0:23 / /tmp rw,nosuid - tmpfs tmpfs rw
28 1 254:3 / / rw,relatime - btrfs /dev/nvme0n1p3 rw,subvol=/root
30 28 254:3 /home /home rw,relatime - btrfs /dev/nvme0n1p3 rw,subvol=/home
40 28 8:1 / /mnt/backup rw,relatime - ext4 /dev/sda1 rw";

        #[test]
        fn picks_longest_matching_mount_source() {
            // /home wins over / for a path inside it.
            assert_eq!(
                select_mount_source(SAMPLE, Path::new("/home/artur/file.txt")).as_deref(),
                Some("/dev/nvme0n1p3")
            );
            assert_eq!(
                select_mount_source(SAMPLE, Path::new("/mnt/backup/data")).as_deref(),
                Some("/dev/sda1")
            );
            // A path with no dedicated mount falls back to the root device.
            assert_eq!(
                select_mount_source(SAMPLE, Path::new("/var/log/app")).as_deref(),
                Some("/dev/nvme0n1p3")
            );
            assert_eq!(
                select_mount_source(SAMPLE, Path::new("/tmp/scratch")).as_deref(),
                Some("tmpfs")
            );
        }

        #[test]
        fn prefix_matching_respects_path_boundaries() {
            // "/home" must not match "/home-backup".
            assert_eq!(
                select_mount_source(SAMPLE, Path::new("/home-backup/x")).as_deref(),
                Some("/dev/nvme0n1p3"),
                "should fall back to root, not the /home mount"
            );
        }

        #[test]
        fn unescapes_mount_points_with_spaces() {
            let line = "30 28 254:3 /home /mnt/my\\040drive rw,relatime - ext4 /dev/sdb1 rw";
            assert_eq!(
                select_mount_source(line, Path::new("/mnt/my drive/file")).as_deref(),
                Some("/dev/sdb1")
            );
        }

        #[test]
        fn non_block_sources_are_unclassified() {
            assert_eq!(device_is_rotational("tmpfs"), None);
            assert_eq!(device_is_rotational("nfs.example:/export"), None);
            assert_eq!(device_is_rotational("overlay"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_classify_as_remote() {
        assert_eq!(classify_path("remote://volume/dir"), StorageClass::Remote);
    }

    #[test]
    fn concurrency_defaults_are_sane() {
        assert_eq!(StorageClass::Rotational.default_concurrency(), 1);
        assert!(StorageClass::Solid.default_concurrency() > 1);
        assert!(
            StorageClass::Remote.default_concurrency() >= StorageClass::Solid.default_concurrency()
        );
        assert!(StorageClass::Unknown.default_concurrency() >= 1);
    }
}
