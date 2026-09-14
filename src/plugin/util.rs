//! System helpers: environment, paths, time and processes.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Name of the cog; forge builds into `target/<name>`.
pub const PACKAGE_NAME: &str = "cord";

pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn home_dir() -> String {
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .unwrap_or_default()
}

pub fn is_windows() -> bool {
    cfg!(windows)
}

pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(unix)]
fn local_tm(secs: i64) -> libc::tm {
    unsafe {
        let t: libc::time_t = secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

/// Unix timestamp of today's local midnight.
pub fn local_midnight() -> u64 {
    #[cfg(unix)]
    unsafe {
        let mut tm = local_tm(unix_time() as i64);
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        libc::mktime(&mut tm).max(0) as u64
    }

    #[cfg(windows)]
    {
        let now = unix_time();
        now - now % 86400
    }
}

/// Current local time as `YYYY-MM-DD HH:MM:SS`.
pub fn local_datetime() -> String {
    let secs = unix_time() as i64;

    #[cfg(unix)]
    {
        let tm = local_tm(secs);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }

    #[cfg(windows)]
    {
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }
}

#[cfg(windows)]
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub struct Uname {
    pub sysname: String,
    pub release: String,
    pub version: String,
    pub machine: String,
}

pub fn uname() -> Uname {
    #[cfg(unix)]
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) == 0 {
            let field = |f: &[libc::c_char]| {
                std::ffi::CStr::from_ptr(f.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            };
            return Uname {
                sysname: field(&u.sysname),
                release: field(&u.release),
                version: field(&u.version),
                machine: field(&u.machine),
            };
        }
    }

    Uname {
        sysname: if is_windows() {
            "Windows_NT"
        } else {
            "unknown"
        }
        .to_string(),
        release: String::new(),
        version: String::new(),
        machine: std::env::consts::ARCH.to_string(),
    }
}

// Paths

/// Expands `~`, converts backslashes and removes trailing slashes.
pub fn normalize(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    if let Some(rest) = p.strip_prefix('~') {
        p = format!("{}{rest}", home_dir());
    }
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
}

/// Last path component.
pub fn tail(path: &str) -> String {
    normalize(path)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Parent directory (`.` for bare names).
pub fn head(path: &str) -> String {
    let p = normalize(path);
    match p.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

/// Everything from the first dot.
pub fn lua_extension(filename: &str) -> Option<&str> {
    filename.find('.').map(|i| &filename[i..])
}

/// Canonical path, without the `\\?\` prefix Windows adds (such paths reject
/// `/` separators).
pub fn realpath(path: &str) -> Option<String> {
    let p = std::fs::canonicalize(path).ok()?;
    let p = p.to_string_lossy();
    let local = p
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'));
    Some(local.unwrap_or(&p).to_string())
}

pub fn is_socket(path: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
    }
    #[cfg(windows)]
    {
        let _ = path;
        false
    }
}

/// Whether a connection to the socket or pipe at `path` can be established.
pub fn probe_socket(path: &str) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }
    #[cfg(windows)]
    {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .is_ok()
    }
}

/// Mirrors how forge locates its home directory.
pub fn steel_home() -> String {
    env("STEEL_HOME").unwrap_or_else(|| {
        let dot_steel = format!("{}/.steel", home_dir());
        if Path::new(&dot_steel).is_dir() {
            dot_steel
        } else {
            format!("{}/steel", data_home())
        }
    })
}

fn data_home() -> String {
    env("XDG_DATA_HOME")
        .unwrap_or_else(|| format!("{}/.local/share", home_dir()))
}

pub fn plugin_root() -> String {
    format!("{}/cogs/{PACKAGE_NAME}", steel_home())
}

pub fn data_path() -> String {
    format!("{}/helix/cord", data_home())
}

pub fn executable_name() -> &'static str {
    if is_windows() { "cord.exe" } else { "cord" }
}

/// `<executable_path>/bin/cord` when configured; otherwise the forge build if it
/// exists, then `cord` in `PATH`, falling back to the forge path.
pub fn executable_path(configured: Option<&str>) -> String {
    let name = executable_name();
    if let Some(prefix) = configured {
        return format!("{}/bin/{name}", normalize(prefix));
    }
    let forge =
        format!("{}/target/{PACKAGE_NAME}/release/{name}", steel_home());
    if Path::new(&forge).is_file() {
        return forge;
    }
    which(name)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(forge)
}

pub fn default_pipe_path() -> &'static str {
    if is_windows() {
        "\\\\.\\pipe\\cord-ipc"
    } else {
        "/tmp/cord-ipc"
    }
}

pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// Processes

fn is_executable_path(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        path.is_file()
    }
}

pub fn is_executable(name: &str) -> bool {
    which(name).is_some()
}

/// Finds an executable in `PATH`, or checks a path directly.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return is_executable_path(path).then(|| path.to_path_buf());
    }

    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(|s| s.to_string())
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            exts.iter().find_map(|ext| {
                let candidate = dir.join(format!("{name}{ext}"));
                is_executable_path(&candidate).then_some(candidate)
            })
        })
    })
}

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a command to completion. Blocks: call it off the core thread.
pub fn run(
    cmd: &str,
    args: &[&str],
    cwd: Option<&str>,
) -> Result<Output, String> {
    let mut command = Command::new(cmd);
    command.args(args).stdin(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|e| format!("Failed to spawn process: {e}"))?;
    Ok(Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
}

pub enum Spawn {
    /// The server printed `Ready`.
    Ready,
    /// Another server instance is already running.
    Retry,
    Error(String),
}

/// Spawns the server detached from the editor and waits until it is ready.
/// Blocks: call it off the core thread.
pub fn spawn_daemon(cmd: &str, args: &[String]) -> Spawn {
    let (tx, rx) = mpsc::channel::<Spawn>();

    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    detach(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Spawn::Error(format!("Failed to spawn process: {e}")),
    };

    let stdout = child.stdout.take().map(|stdout| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            // Keep draining so the server never writes into a closed pipe
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.contains("Ready") {
                    let _ = tx.send(Spawn::Ready);
                }
            }
        })
    });

    let stderr = child.stderr.take().map(|stderr| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stderr).lines();
            if let Some(Ok(line)) = lines.next() {
                let _ =
                    tx.send(if line.contains("another instance is running") {
                        Spawn::Retry
                    } else {
                        Spawn::Error(line)
                    });
            }
            for _ in lines {}
        })
    });

    std::thread::spawn(move || {
        let status = child.wait();
        // Let the readers report what the server printed before exiting
        for reader in [stdout, stderr].into_iter().flatten() {
            let _ = reader.join();
        }
        let _ = tx.send(Spawn::Error(match status {
            Ok(status) => format!("server exited with {status}"),
            Err(e) => e.to_string(),
        }));
    });

    rx.recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| {
            Spawn::Error("timed out waiting for the server".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers_match_vim() {
        assert_eq!(tail("/a/b/c.rs"), "c.rs");
        assert_eq!(head("/a/b/c.rs"), "/a/b");
        assert_eq!(head("/a"), "/");
        assert_eq!(head("a"), ".");
        assert_eq!(normalize("/a/b/"), "/a/b");
        assert_eq!(lua_extension("x.tar.gz"), Some(".tar.gz"));
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("a b/ü"), "a%20b%2F%C3%BC");
    }
}
