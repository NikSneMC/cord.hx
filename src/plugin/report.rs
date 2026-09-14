//! Debug and health reports.

use std::path::Path;

use serde_json::{Value, json};

use super::core::Core;
use super::log;
use super::util;
use super::validate::{Validation, validate};
use super::value::{self, is_proc};

/// Renders a configuration value as Scheme source.
pub fn inspect(v: &Value, indent: &str, string_keys: bool) -> String {
    let next = format!("{indent}  ");
    match v {
        Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        Value::Object(_) if is_proc(v) => "<procedure>".to_string(),
        Value::Object(map) if map.is_empty() => "(hash)".to_string(),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = if string_keys {
                        format!("\"{k}\"")
                    } else {
                        format!("'{k}")
                    };
                    format!(
                        "{next}{key} {}",
                        inspect(&map[k], &next, k == "assets")
                    )
                })
                .collect();
            format!("(hash\n{})", entries.join("\n"))
        }
        Value::Array(items) if items.is_empty() => "'()".to_string(),
        Value::Array(items) => {
            let items: Vec<String> =
                items.iter().map(|x| inspect(x, &next, false)).collect();
            format!("(list {})", items.join(" "))
        }
        Value::Bool(b) => if *b { "#t" } else { "#f" }.to_string(),
        Value::Null => "void".to_string(),
        other => value::to_display(other),
    }
}

fn validation_lines(v: &Validation) -> Vec<String> {
    if v.is_valid() {
        return vec!["no issues found".to_string()];
    }
    let mut out = Vec::new();
    for e in &v.errors {
        out.push(format!("- error: {}", e.msg));
        out.push(format!("  hint: {}", e.hint));
    }
    out.extend(v.warnings.iter().map(|w| format!("- warning: {w}")));
    out
}

struct ServerInfo {
    version: String,
    path: String,
    executable: bool,
}

fn server_info(exe: &str) -> ServerInfo {
    if !Path::new(exe).exists() {
        return ServerInfo {
            version: "not installed".into(),
            path: exe.into(),
            executable: false,
        };
    }
    let executable = util::is_windows() || util::which(exe).is_some();
    let version = executable
        .then(|| util::run(exe, &["-v"], None).ok())
        .flatten()
        .filter(|o| o.code == 0 && !o.stdout.trim().is_empty())
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    ServerInfo {
        version,
        path: exe.into(),
        executable,
    }
}

fn git_sha() -> String {
    let root = util::plugin_root();
    if !Path::new(&format!("{root}/.git")).exists() {
        return "unknown".into();
    }
    if !util::is_executable("git") {
        return "git not found".into();
    }
    util::run("git", &["rev-parse", "--short", "HEAD"], Some(&root))
        .ok()
        .filter(|o| o.code == 0 && !o.stdout.trim().is_empty())
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn discord_pipes(custom: &[String]) -> Vec<(String, bool)> {
    let paths: Vec<String> = if !custom.is_empty() {
        custom.to_vec()
    } else if util::is_windows() {
        (0..=10)
            .map(|i| format!("\\\\.\\pipe\\discord-ipc-{i}"))
            .collect()
    } else {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for base in [
            util::env("XDG_RUNTIME_DIR"),
            util::env("TMPDIR"),
            util::env("TMP"),
            util::env("TEMP"),
            Some("/tmp".into()),
        ]
        .into_iter()
        .flatten()
        {
            if seen.contains(&base) {
                continue;
            }
            out.extend(
                (0..=10)
                    .map(|i| format!("{base}/discord-ipc-{i}"))
                    .filter(|p| util::is_socket(p)),
            );
            seen.push(base);
        }
        out
    };
    paths
        .into_iter()
        .map(|p| {
            let reachable = util::probe_socket(&p);
            (p, reachable)
        })
        .collect()
}

/// Everything the reports need from the core, gathered on the core thread.
struct Snapshot {
    exe: String,
    status: String,
    pipe_path: String,
    client_alive: bool,
    manager_alive: bool,
    custom_pipes: Vec<String>,
    user: Value,
    merged: Value,
}

fn snapshot(core: &Core) -> Snapshot {
    Snapshot {
        exe: core.config.executable(),
        status: core.server.status.describe().to_string(),
        pipe_path: core.config.pipe_path(),
        client_alive: core.server.is_connected(),
        manager_alive: core.manager.active,
        custom_pipes: core.config.discord_pipe_paths.clone(),
        user: core.user_config.clone(),
        merged: core.config.merged.clone(),
    }
}

fn debug_report(s: Snapshot) -> String {
    let mut lines: Vec<String> = Vec::new();
    let section = |lines: &mut Vec<String>, title: &str| {
        lines.extend(["".into(), format!("### {title}"), "".into()])
    };
    let kv = |lines: &mut Vec<String>, k: &str, v: &dyn std::fmt::Display| {
        lines.push(format!("**`{k}`**: `{v}`"))
    };

    let server = server_info(&s.exe);
    section(&mut lines, "version");
    kv(&mut lines, "server", &server.version);
    kv(&mut lines, "executable", &server.executable);
    kv(&mut lines, "executable path", &server.path);
    kv(&mut lines, "git sha", &git_sha());
    kv(&mut lines, "plugin", &env!("CARGO_PKG_VERSION"));
    kv(&mut lines, "steel dylib", &"libcord_hx");
    section(&mut lines, "warnings");
    lines.push(
        if server.executable {
            "none"
        } else {
            "- server is not executable"
        }
        .into(),
    );

    section(&mut lines, "status");
    kv(&mut lines, "status", &s.status.to_lowercase());
    kv(&mut lines, "pipe path", &s.pipe_path);
    kv(&mut lines, "pipe exists", &Path::new(&s.pipe_path).exists());
    kv(&mut lines, "client alive", &s.client_alive);
    kv(&mut lines, "manager alive", &s.manager_alive);

    section(&mut lines, "discord rpc");
    if !s.custom_pipes.is_empty() {
        lines.push("(using custom pipe paths)".into());
    }
    let pipes = discord_pipes(&s.custom_pipes);
    if pipes.is_empty() {
        lines.push("none found".into());
    }
    for (path, reachable) in pipes {
        lines.push(format!(
            "- `{path}`: {}",
            if reachable {
                "reachable"
            } else {
                "unreachable"
            }
        ));
    }

    let os = util::uname();
    section(&mut lines, "os");
    kv(&mut lines, "sysname", &os.sysname);
    kv(&mut lines, "release", &os.release);
    kv(&mut lines, "machine", &os.machine);
    if let Some(wsl) = util::env("WSL_DISTRO_NAME") {
        kv(&mut lines, "wsl", &wsl);
    }

    section(&mut lines, "config validation");
    lines.extend(validation_lines(&validate(&s.user)));

    section(&mut lines, "config (user)");
    if s.user.as_object().is_none_or(|m| m.is_empty()) {
        lines.push("(none)".into());
    } else {
        lines.push("```scheme".into());
        lines.extend(inspect(&s.user, "", false).lines().map(str::to_string));
        lines.push("```".into());
    }

    section(&mut lines, "config (merged)");
    lines.push("```scheme".into());
    lines.extend(inspect(&s.merged, "", false).lines().map(str::to_string));
    lines.push("```".into());

    lines.join("\n")
}

fn health_report(s: Snapshot) -> String {
    let os = util::uname();
    let server = server_info(&s.exe);
    let mut lines = vec![
        "# cord.hx".to_string(),
        String::new(),
        "System information:".into(),
        format!("  Sysname: `{}`", os.sysname),
        format!("  Architecture: `{}`", os.machine),
        format!("  Release: `{}`", os.release),
        format!("  Version: `{}`", os.version),
    ];
    if let Some(wsl) = util::env("WSL_DISTRO_NAME") {
        lines.push(format!("  Running inside WSL (`{wsl}`)"));
    }
    lines.extend([
        String::new(),
        format!("Cord connection status: `{}`", s.status),
        if server.executable {
            format!("OK: server executable found at `{}`", server.path)
        } else {
            format!("WARNING: server executable missing at `{}`", server.path)
        },
        if util::is_executable("curl") {
            "OK: `curl` is installed".into()
        } else {
            "WARNING: `curl` is not installed or not in PATH (needed by the lastfm extension)".into()
        },
        String::new(),
        "Configuration:".into(),
    ]);
    lines.extend(validation_lines(&validate(&s.user)));
    lines.join("\n")
}

fn open(core: &mut Core, name: &'static str, build: fn(Snapshot) -> String) {
    let s = snapshot(core);
    core.spawn(move || build(s), move |core, content| {
        core.action(json!({ "type": "scratch", "name": name, "content": content, "language": "markdown" }));
    });
}

pub fn show_debug(core: &mut Core) {
    open(core, "cord_debug", debug_report);
}

pub fn show_health(core: &mut Core) {
    open(core, "cord_health", health_report);
}

pub fn copy_debug(core: &mut Core) {
    let s = snapshot(core);
    core.spawn(
        move || debug_report(s),
        |core, content| {
            // Notify first: writing the clipboard clears the status line
            core.log_notify("copied to clipboard", log::INFO);
            core.action(json!({ "type": "clipboard", "text": content }));
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspects_as_scheme() {
        let v = json!({ "b": [1, "x"], "a": { "__cord_proc": 1 }, "assets": { "rs": true } });
        assert_eq!(
            inspect(&v, "", false),
            "(hash\n  'a <procedure>\n  'assets (hash\n    \"rs\" #t)\n  'b (list 1 \"x\"))"
        );
    }
}
