use crate::fs::models::FsError;
use serde::Serialize;

#[derive(Default)]
pub struct TerminalState {
    #[cfg(unix)]
    inner: unix::TerminalRegistry,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub session_id: u64,
    pub shell: String,
    pub cwd: String,
}

#[tauri::command]
pub async fn terminal_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, TerminalState>,
    cwd: Option<String>,
) -> Result<TerminalSessionInfo, FsError> {
    #[cfg(unix)]
    {
        return unix::start(app, &state.inner, cwd);
    }

    #[cfg(not(unix))]
    {
        let _ = app;
        let _ = state;
        let _ = cwd;

        Err(FsError::new(
            "terminal_unsupported",
            "Embedded terminal sessions are currently supported on macOS and Linux.",
            None,
        ))
    }
}

#[tauri::command]
pub async fn terminal_write(
    state: tauri::State<'_, TerminalState>,
    session_id: u64,
    data: String,
) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        return unix::write(&state.inner, session_id, data);
    }

    #[cfg(not(unix))]
    {
        let _ = state;
        let _ = session_id;
        let _ = data;

        Err(FsError::new(
            "terminal_unsupported",
            "Embedded terminal sessions are currently supported on macOS and Linux.",
            None,
        ))
    }
}

#[tauri::command]
pub async fn terminal_resize(
    state: tauri::State<'_, TerminalState>,
    session_id: u64,
    rows: u16,
    cols: u16,
) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        return unix::resize(&state.inner, session_id, rows, cols);
    }

    #[cfg(not(unix))]
    {
        let _ = state;
        let _ = session_id;
        let _ = rows;
        let _ = cols;

        Err(FsError::new(
            "terminal_unsupported",
            "Embedded terminal sessions are currently supported on macOS and Linux.",
            None,
        ))
    }
}

#[tauri::command]
pub async fn terminal_close(
    state: tauri::State<'_, TerminalState>,
    session_id: u64,
) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        return unix::close(&state.inner, session_id);
    }

    #[cfg(not(unix))]
    {
        let _ = state;
        let _ = session_id;

        Err(FsError::new(
            "terminal_unsupported",
            "Embedded terminal sessions are currently supported on macOS and Linux.",
            None,
        ))
    }
}

/// Current working directory of a session's shell, or `None` if it can't be
/// resolved (closed session, or unsupported platform).
#[tauri::command]
pub async fn terminal_cwd(
    state: tauri::State<'_, TerminalState>,
    session_id: u64,
) -> Result<Option<String>, FsError> {
    #[cfg(unix)]
    {
        return Ok(unix::cwd(&state.inner, session_id));
    }

    #[cfg(not(unix))]
    {
        let _ = state;
        let _ = session_id;
        Ok(None)
    }
}

#[cfg(unix)]
mod unix {
    use super::TerminalSessionInfo;
    use crate::fs::models::FsError;
    use serde::Serialize;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::thread;
    use tauri::Emitter;

    #[derive(Default)]
    pub struct TerminalRegistry {
        next_id: AtomicU64,
        sessions: Mutex<HashMap<u64, TerminalSession>>,
    }

    struct TerminalSession {
        child: Child,
        master: File,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TerminalOutputPayload {
        session_id: u64,
        data: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TerminalExitPayload {
        session_id: u64,
    }

    pub fn start(
        app: tauri::AppHandle,
        registry: &TerminalRegistry,
        cwd: Option<String>,
    ) -> Result<TerminalSessionInfo, FsError> {
        let session_id = registry.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let cwd_path = resolve_cwd(cwd)?;
        let (master, slave) = open_pty()?;

        let mut command = Command::new(&shell);
        command
            .arg("-i")
            .current_dir(&cwd_path)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor");

        let slave_fd = slave.as_raw_fd();

        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                Ok(())
            });
        }

        command
            .stdin(Stdio::from(
                slave
                    .try_clone()
                    .map_err(|error| terminal_error("pty_clone_failed", error))?,
            ))
            .stdout(Stdio::from(
                slave
                    .try_clone()
                    .map_err(|error| terminal_error("pty_clone_failed", error))?,
            ))
            .stderr(Stdio::from(slave));

        let child = command
            .spawn()
            .map_err(|error| terminal_error("terminal_spawn_failed", error))?;
        let reader = master
            .try_clone()
            .map_err(|error| terminal_error("pty_clone_failed", error))?;

        registry
            .sessions
            .lock()
            .map_err(|_| {
                FsError::new(
                    "terminal_lock_failed",
                    "Unable to lock terminal sessions.",
                    None,
                )
            })?
            .insert(session_id, TerminalSession { child, master });

        spawn_reader(app, session_id, reader);

        Ok(TerminalSessionInfo {
            session_id,
            shell,
            cwd: cwd_path.to_string_lossy().into_owned(),
        })
    }

    pub fn write(
        registry: &TerminalRegistry,
        session_id: u64,
        data: String,
    ) -> Result<(), FsError> {
        let mut sessions = registry.sessions.lock().map_err(|_| {
            FsError::new(
                "terminal_lock_failed",
                "Unable to lock terminal sessions.",
                None,
            )
        })?;
        let session = sessions.get_mut(&session_id).ok_or_else(|| {
            FsError::new(
                "terminal_not_found",
                "Terminal session was not found.",
                None,
            )
        })?;

        session
            .master
            .write_all(data.as_bytes())
            .map_err(|error| terminal_error("terminal_write_failed", error))
    }

    pub fn resize(
        registry: &TerminalRegistry,
        session_id: u64,
        rows: u16,
        cols: u16,
    ) -> Result<(), FsError> {
        let sessions = registry.sessions.lock().map_err(|_| {
            FsError::new(
                "terminal_lock_failed",
                "Unable to lock terminal sessions.",
                None,
            )
        })?;
        let session = sessions.get(&session_id).ok_or_else(|| {
            FsError::new(
                "terminal_not_found",
                "Terminal session was not found.",
                None,
            )
        })?;
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let result = unsafe { libc::ioctl(session.master.as_raw_fd(), libc::TIOCSWINSZ, &size) };

        if result == -1 {
            return Err(terminal_error(
                "terminal_resize_failed",
                std::io::Error::last_os_error(),
            ));
        }

        Ok(())
    }

    pub fn cwd(registry: &TerminalRegistry, session_id: u64) -> Option<String> {
        let sessions = registry.sessions.lock().ok()?;
        let session = sessions.get(&session_id)?;
        read_process_cwd(session.child.id())
    }

    // The shell is the pty's direct child, so its cwd reflects `cd`. On Linux
    // /proc/<pid>/cwd is a symlink to that directory.
    #[cfg(target_os = "linux")]
    fn read_process_cwd(pid: u32) -> Option<String> {
        std::fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }

    #[cfg(not(target_os = "linux"))]
    fn read_process_cwd(_pid: u32) -> Option<String> {
        None
    }

    pub fn close(registry: &TerminalRegistry, session_id: u64) -> Result<(), FsError> {
        let mut session = registry
            .sessions
            .lock()
            .map_err(|_| {
                FsError::new(
                    "terminal_lock_failed",
                    "Unable to lock terminal sessions.",
                    None,
                )
            })?
            .remove(&session_id);

        if let Some(session) = session.as_mut() {
            let _ = session.child.kill();
            let _ = session.child.wait();
        }

        Ok(())
    }

    fn open_pty() -> Result<(File, File), FsError> {
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let result = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };

        if result == -1 {
            return Err(terminal_error(
                "pty_open_failed",
                std::io::Error::last_os_error(),
            ));
        }

        let master = unsafe { File::from_raw_fd(master_fd) };
        let slave = unsafe { File::from_raw_fd(slave_fd) };

        Ok((master, slave))
    }

    fn resolve_cwd(cwd: Option<String>) -> Result<PathBuf, FsError> {
        let cwd = cwd.unwrap_or_default();
        let trimmed = cwd.trim();

        if trimmed.is_empty() || trimmed == "~" {
            return home_dir();
        }

        if let Some(rest) = trimmed.strip_prefix("~/") {
            return Ok(home_dir()?.join(rest));
        }

        Ok(PathBuf::from(trimmed))
    }

    fn home_dir() -> Result<PathBuf, FsError> {
        std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            FsError::new(
                "home_not_found",
                "Unable to resolve a home directory.",
                None,
            )
        })
    }

    fn spawn_reader(app: tauri::AppHandle, session_id: u64, mut reader: File) {
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];

            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        let data = String::from_utf8_lossy(&buffer[..size]).into_owned();
                        let _ = app.emit(
                            "terminal://output",
                            TerminalOutputPayload { session_id, data },
                        );
                    }
                    Err(_) => break,
                }
            }

            let _ = app.emit("terminal://exit", TerminalExitPayload { session_id });
        });
    }

    fn terminal_error(code: &'static str, error: std::io::Error) -> FsError {
        FsError::new(code, format!("Terminal operation failed: {error}"), None)
    }
}
