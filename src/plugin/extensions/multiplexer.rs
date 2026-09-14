//! `'tmux` and `'zellij`: follow the terminal multiplexer session.
//!
//! * The presence is hidden while the session is detached.
//! * When another pane of the Helix window (tmux) or tab (zellij) is focused,
//!   or a zellij pane was opened in place of Helix, and it runs a known
//!   command (`lazygit`, `yazi`, `man`, ... see the `cli` entries of
//!   `mappings.tsv`), the activity shows that command instead. It is sent when
//!   the command appears or changes, or the session is attached again: using
//!   it is invisible to Helix, so once another editor instance has taken the
//!   presence over, returning to the command does not take it back.
//!
//! Config:
//! * `'interval` - milliseconds between status checks (default 2000)
//! * `'on-attach` - `'show` (default) or a procedure
//! * `'on-detach` - `'hide` (default), `'idle` or a procedure
//! * `'cli?` - report commands focused next to Helix (default #t)
//! * `'focus-events?` - also check when Helix gains or loses focus (default #t)

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Extension, Setup};
use crate::plugin::config::Callable;
use crate::plugin::core::Core;
use crate::plugin::icons;
use crate::plugin::util;
use crate::plugin::value::{proc_id, to_display};

#[derive(Clone)]
enum Action {
    Show,
    Hide,
    Idle,
    Proc(u64),
    Nothing,
}

fn action(v: Option<&Value>, default: Action) -> Action {
    match v {
        None | Some(Value::Null) => default,
        Some(v) => match proc_id(v) {
            Some(id) => Action::Proc(id),
            None => match to_display(v).as_str() {
                "show" => Action::Show,
                "hide" => Action::Hide,
                "idle" => Action::Idle,
                _ => Action::Nothing,
            },
        },
    }
}

struct Settings {
    interval: u64,
    cli: bool,
    focus_events: bool,
    on_attach: Action,
    on_detach: Action,
}

/// Where Helix runs: the tmux pane id or the zellij pane id.
#[derive(Clone)]
struct Pane(String);

#[derive(Default)]
struct Status {
    session: Option<String>,
    window: Option<String>,
    attached: Option<u64>,
    /// Command of the focused pane next to Helix.
    cli: Option<String>,
}

#[derive(Default)]
struct State {
    pane: Option<Pane>,
    status: Status,
    detached: bool,
    stopped: bool,
}

struct Kind {
    /// The pane of Helix, if it runs inside this multiplexer.
    detect: fn() -> Option<(Pane, Option<String>)>,
    /// Queries the status. Blocks: runs off the core thread.
    query: fn(&Pane) -> Option<Status>,
}

/// Command name of a command line: `/usr/bin/lazygit -p x` is `lazygit`.
fn command_name(line: &str) -> Option<String> {
    let program = line.split_whitespace().next()?;
    Some(util::tail(program))
}

fn tmux_query(pane: &Pane) -> Option<Status> {
    let output = util::run(
        "tmux",
        &[
            "display-message",
            "-t",
            &pane.0,
            "-p",
            "#{session_attached}\t#{session_name}\t#{window_name}",
            ";",
            "list-panes",
            "-t",
            &pane.0,
            "-F",
            "#{pane_id}\t#{pane_active}\t#{pane_current_command}",
        ],
        None,
    )
    .ok()
    .filter(|o| o.code == 0)?;
    let mut lines = output.stdout.lines();
    let mut info = lines.next()?.split('\t');
    let attached = info.next()?.parse().unwrap_or(0);
    let session = info.next().map(str::to_string);
    let window = info.next().map(str::to_string);
    let cli = lines.find_map(|line| {
        let mut f = line.split('\t');
        let (id, active, command) = (f.next()?, f.next()?, f.next()?);
        (id != pane.0 && active == "1").then(|| command_name(command))?
    });
    Some(Status {
        session,
        window,
        attached: Some(attached),
        cli,
    })
}

/// Terminal pane focused by each attached client, from `zellij action
/// list-clients` (`is_focused` of `list-panes` is set per layer, so hidden
/// floating panes count as focused too).
fn zellij_client_panes(stdout: &str) -> Vec<Option<u64>> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("CLIENT_ID"))
        .map(|l| {
            l.split_whitespace()
                .nth(1)
                .and_then(|p| p.strip_prefix("terminal_"))
                .and_then(|id| id.parse().ok())
        })
        .collect()
}

/// Command of the pane shown instead of Helix: one opened in place of it
/// (`zellij run --in-place` suppresses Helix and takes its geometry), or
/// one a client focuses in the same tab.
fn zellij_cli(
    own: &Value,
    clients: &[Option<u64>],
    panes: &[Value],
) -> Option<String> {
    let terminal = |p: &&Value| {
        p["is_plugin"] == Value::Bool(false) && p["id"] != own["id"]
    };
    let geometry = |p: &Value| {
        [
            "tab_id",
            "is_floating",
            "pane_x",
            "pane_y",
            "pane_rows",
            "pane_columns",
        ]
        .map(|k| p[k].clone())
    };
    let in_place = || {
        panes.iter().filter(terminal).find(|p| {
            own["is_suppressed"] == Value::Bool(true)
                && p["is_suppressed"] != Value::Bool(true)
                && geometry(p) == geometry(own)
        })
    };
    let focused = || {
        clients.iter().flatten().find_map(|&id| {
            panes.iter().filter(terminal).find(|p| {
                p["id"].as_u64() == Some(id) && p["tab_id"] == own["tab_id"]
            })
        })
    };
    in_place()
        .or_else(focused)
        .and_then(|p| p["pane_command"].as_str())
        .and_then(command_name)
}

fn zellij_query(pane: &Pane) -> Option<Status> {
    let run = |args: &[&str]| {
        util::run("zellij", args, None).ok().filter(|o| o.code == 0)
    };
    let clients =
        zellij_client_panes(&run(&["action", "list-clients"])?.stdout);

    let panes: Value = run(&["action", "list-panes", "--all", "--json"])
        .and_then(|o| serde_json::from_str(&o.stdout).ok())
        .unwrap_or_default();
    let panes = panes.as_array().cloned().unwrap_or_default();
    let own = panes.iter().find(|p| {
        p["is_plugin"] == Value::Bool(false)
            && pane.0.parse().ok() == p["id"].as_u64()
    });
    Some(Status {
        session: util::env("ZELLIJ_SESSION_NAME"),
        window: own.and_then(|p| p["tab_name"].as_str()).map(str::to_string),
        attached: Some(clients.len() as u64),
        cli: own.and_then(|own| zellij_cli(own, &clients, &panes)),
    })
}

static TMUX: Kind = Kind {
    detect: || {
        let pane = util::env("TMUX_PANE")?;
        util::is_executable("tmux").then_some((Pane(pane), None))
    },
    query: tmux_query,
};

static ZELLIJ: Kind = Kind {
    detect: || {
        util::env("ZELLIJ")?;
        let pane = util::env("ZELLIJ_PANE_ID").unwrap_or_default();
        util::is_executable("zellij")
            .then(|| (Pane(pane), util::env("ZELLIJ_SESSION_NAME")))
    },
    query: zellij_query,
};

/// Applies a new status: attach/detach actions on transitions of the
/// attached client count, and activity updates for focused commands.
fn apply(
    core: &mut Core,
    settings: &Settings,
    state: &Mutex<State>,
    mut status: Status,
) {
    if !settings.cli
        || status
            .cli
            .as_deref()
            .is_some_and(|c| icons::lookup_cli(c).is_none())
    {
        status.cli = None;
    }
    let (transition, cli, cli_changed) = {
        let mut s = state.lock().unwrap();
        let cli_changed = s.status.cli != status.cli;
        let attached = status.attached.unwrap_or(1);
        let transition = if attached == 0 && !s.detached {
            s.detached = true;
            Some(false)
        } else if attached > 0 && s.detached {
            s.detached = false;
            Some(true)
        } else {
            None
        };
        s.status = status;
        (transition, s.status.cli.is_some(), cli_changed)
    };

    match (transition, &settings.on_attach, &settings.on_detach) {
        (Some(false), _, Action::Hide) => core.manager_suppress(),
        (Some(false), _, Action::Idle) => core.manager_idle(),
        (Some(false), _, Action::Proc(p))
        | (Some(true), Action::Proc(p), _) => {
            core.call_or_notify(&Callable::Proc(*p), &[]);
        }
        (Some(true), Action::Show, Action::Hide) => core.manager_resume(),
        (Some(true), Action::Show, Action::Idle) => core.manager_unidle(),
        _ => {}
    }

    // Update only when the command changes or the session is attached again:
    // re-sending it on every check would take the presence back from other
    // editor instances. A command staying focused still counts as activity,
    // so this instance does not idle.
    if cli_changed || (cli && transition == Some(true)) {
        core.manager_queue_update(true);
    } else if cli {
        core.manager_record_activity();
    }
}

fn update(
    core: &mut Core,
    kind: &'static Kind,
    settings: Arc<Settings>,
    state: Arc<Mutex<State>>,
) {
    let Some(pane) = state.lock().unwrap().pane.clone() else {
        return;
    };
    core.spawn(
        move || (kind.query)(&pane),
        move |core, status| {
            if let Some(status) = status {
                apply(core, &settings, &state, status);
            }
        },
    );
}

fn poll(
    core: &mut Core,
    kind: &'static Kind,
    settings: Arc<Settings>,
    state: Arc<Mutex<State>>,
) {
    if state.lock().unwrap().stopped {
        return;
    }
    update(core, kind, Arc::clone(&settings), Arc::clone(&state));
    if settings.interval > 0 {
        let interval = Duration::from_millis(settings.interval);
        core.after(interval, move |core| poll(core, kind, settings, state));
    }
}

fn setup(
    kind: &'static Kind,
    prefix: &str,
    s: &Setup<'_>,
) -> Result<Extension, String> {
    let settings = Arc::new(Settings {
        interval: s.number("interval", 2000.0)?.max(0.0) as u64,
        cli: s.bool("cli?", true)?,
        focus_events: s.bool("focus-events?", true)?,
        on_attach: action(s.get("on-attach"), Action::Show),
        on_detach: action(s.get("on-detach"), Action::Hide),
    });
    let state: Arc<Mutex<State>> = Arc::default();
    let mut ext = Extension::default();

    type Get = fn(&State) -> Value;
    let variables: [(&str, Get); 5] = [
        ("?", |s| json!(s.pane.is_some())),
        ("-session", |s| {
            s.status.session.as_ref().map_or(Value::Null, |v| json!(v))
        }),
        ("-window", |s| {
            s.status.window.as_ref().map_or(Value::Null, |v| json!(v))
        }),
        ("-attached-count", |s| {
            s.status.attached.map_or(Value::Null, |n| json!(n))
        }),
        ("-cli", |s| {
            s.status.cli.as_ref().map_or(Value::Null, |v| json!(v))
        }),
    ];
    for (suffix, get) in variables {
        let name = if suffix == "?" {
            format!("in-{prefix}?")
        } else {
            format!("{prefix}{suffix}")
        };
        let state = Arc::clone(&state);
        ext.variable(&name, move |_, _| get(&state.lock().unwrap()));
    }

    {
        let (settings, state) = (Arc::clone(&settings), Arc::clone(&state));
        ext.hook("ready", 50, move |core, _| {
            let Some((pane, session)) = (kind.detect)() else {
                return Value::Null;
            };
            {
                let mut s = state.lock().unwrap();
                s.pane = Some(pane);
                s.status.session = session;
            }
            poll(core, kind, Arc::clone(&settings), Arc::clone(&state));
            Value::Null
        });
    }

    {
        let state = Arc::clone(&state);
        ext.hook("pre-activity", 75, move |_, args| {
            let cli = state.lock().unwrap().status.cli.clone();
            match (cli, args.first()) {
                (Some(cli), Some(opts)) => {
                    let mut opts = opts.clone();
                    opts["force-cli"] = json!(cli);
                    opts
                }
                _ => Value::Null,
            }
        });
    }

    if settings.focus_events {
        for event in ["focus-gained", "focus-lost"] {
            let (settings, state) = (Arc::clone(&settings), Arc::clone(&state));
            ext.hook(event, 50, move |core, _| {
                update(core, kind, Arc::clone(&settings), Arc::clone(&state));
                Value::Null
            });
        }
    }

    ext.shutdown =
        Some(Box::new(move |_| state.lock().unwrap().stopped = true));
    Ok(ext)
}

pub fn setup_tmux(_: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    setup(&TMUX, "tmux", s)
}

pub fn setup_zellij(_: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    setup(&ZELLIJ, "zellij", s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names() {
        assert_eq!(
            command_name("/usr/bin/lazygit -p x").as_deref(),
            Some("lazygit")
        );
        assert_eq!(command_name("yazi").as_deref(), Some("yazi"));
        assert_eq!(command_name("  "), None);
    }

    #[test]
    fn zellij_clients() {
        let out = "CLIENT_ID ZELLIJ_PANE_ID RUNNING_COMMAND\n1         terminal_12    lazygit\n2         plugin_0\n";
        assert_eq!(zellij_client_panes(out), vec![Some(12), None]);
    }

    #[test]
    fn zellij_panes() {
        let pane = |id: u64,
                    cmd: &str,
                    x: u64,
                    suppressed: bool,
                    floating: bool| {
            json!({ "id": id, "is_plugin": false, "tab_id": 0, "pane_command": cmd,
                    "is_suppressed": suppressed, "is_floating": floating,
                    "pane_x": x, "pane_y": 1, "pane_rows": 67, "pane_columns": 187 })
        };
        let hx = pane(10, "/bin/hx", 188, true, false);
        let panes = vec![
            json!({ "id": 0, "is_plugin": true, "tab_id": 0 }),
            pane(0, "claude", 0, false, false),
            pane(2, "fish", 94, false, true),
            hx.clone(),
            pane(12, "lazygit", 188, false, false),
        ];
        // In place of Helix, while another pane is focused
        assert_eq!(
            zellij_cli(&hx, &[Some(0)], &panes).as_deref(),
            Some("lazygit")
        );

        let mut hx = hx;
        hx["is_suppressed"] = json!(false);
        let panes = vec![
            pane(0, "claude", 0, false, false),
            pane(2, "fish", 94, false, true),
            hx.clone(),
        ];
        assert_eq!(
            zellij_cli(&hx, &[Some(0)], &panes).as_deref(),
            Some("claude")
        );
        assert_eq!(zellij_cli(&hx, &[Some(10), None], &panes), None);
    }
}
