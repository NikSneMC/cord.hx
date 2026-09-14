//! `'persistent-timer`: accumulates time per scope across sessions.
//!
//! Config:
//! * `'scope` - `'workspace` (default), `'file`, `'filetype` or `'global`
//! * `'file` - JSON file the timers are saved to
//! * `'mode` - `'all` (default), `'active` or `'idle`
//! * `'save-on` - list of `'focus-change` and `'periodic` (default both)
//! * `'save-interval` - seconds between periodic saves (default 30)
//!
//! Helix has no exit hook: `'exit` is accepted but ignored, and timers are also
//! saved when the configuration is reapplied. The file format is shared with
//! cord.nvim:
//! `{ key: { modes: { mode: { total_time } } } }`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{Extension, Setup};
use crate::plugin::core::Core;
use crate::plugin::log;
use crate::plugin::util::{self, unix_time};
use crate::plugin::value::to_display;

const MODES: &[&str] = &["all", "active", "idle"];

#[derive(Clone, Default)]
struct Bucket {
    total: u64,
    session_start: Option<u64>,
    display_start: Option<u64>,
}

impl Bucket {
    /// The bucket with the running session folded into `total`.
    fn settle(&self) -> Bucket {
        match self.session_start {
            Some(start) => Bucket {
                total: self.total + unix_time().saturating_sub(start),
                ..Default::default()
            },
            None => self.clone(),
        }
    }
}

#[derive(Default)]
struct State {
    timers: HashMap<String, HashMap<String, Bucket>>,
    current_key: Option<String>,
    last_save: u64,
    loaded: bool,
    stopped: bool,
}

struct Settings {
    scope: String,
    file: String,
    mode: String,
    instance: String,
}

impl State {
    fn bucket(&mut self, key: &str, mode: &str) -> &mut Bucket {
        self.timers
            .entry(key.to_string())
            .or_default()
            .entry(mode.to_string())
            .or_default()
    }

    fn stop(&mut self, key: &str) {
        for mode in MODES {
            let b = self.bucket(key, mode);
            *b = b.settle();
        }
    }

    fn start(&mut self, key: &str, mode: &str) {
        self.current_key = Some(key.to_string());
        let modes: &[&str] = if mode == "all" {
            &["all"]
        } else {
            &["all", mode]
        };
        for m in modes {
            let b = self.bucket(key, m);
            if b.session_start.is_none() {
                b.session_start = Some(unix_time());
            }
        }
    }

    fn merge(&mut self, data: &Value) {
        let Some(data) = data.as_object() else { return };
        for (key, entry) in data {
            let Some(modes) = entry.get("modes").and_then(Value::as_object)
            else {
                continue;
            };
            for (mode, b) in modes {
                if let Some(total) = b.get("total_time").and_then(Value::as_u64)
                {
                    let bucket = self.bucket(key, mode);
                    bucket.total = bucket.total.max(total);
                }
            }
        }
    }

    fn snapshot(&self) -> Map<String, Value> {
        self.timers
            .iter()
            .map(|(key, modes)| {
                let modes: Map<String, Value> = modes
                    .iter()
                    .map(|(m, b)| {
                        (m.clone(), json!({ "total_time": b.settle().total }))
                    })
                    .collect();
                (key.clone(), json!({ "modes": modes }))
            })
            .collect()
    }
}

fn read_file(file: &str) -> Option<Value> {
    let content = std::fs::read_to_string(file).ok()?;
    serde_json::from_str::<Value>(&content)
        .ok()
        .filter(Value::is_object)
}

/// Entries of `out` merged with those saved by other sessions, keeping the
/// larger total of each mode.
fn merge_snapshot(
    mut out: Map<String, Value>,
    data: &Value,
) -> Map<String, Value> {
    let Some(data) = data.as_object() else {
        return out;
    };
    for (key, entry) in data {
        let Some(theirs) = entry.get("modes").and_then(Value::as_object) else {
            continue;
        };
        let ours = out
            .entry(key.clone())
            .or_insert_with(|| json!({ "modes": {} }));
        for (mode, b) in theirs {
            let Some(total) = b.get("total_time").and_then(Value::as_u64)
            else {
                continue;
            };
            let own = ours["modes"]
                .get(mode)
                .and_then(|m| m.get("total_time"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if total > own {
                ours["modes"][mode] = json!({ "total_time": total });
            }
        }
    }
    out
}

fn save(core: &mut Core, state: &Mutex<State>, settings: &Settings) {
    let snapshot = {
        let s = state.lock().unwrap();
        // Saving before the file was loaded would drop the stored timers
        if !s.loaded {
            return;
        }
        s.snapshot()
    };
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(util::head(&settings.file))
            .map_err(|e| e.to_string())?;
        let data = match read_file(&settings.file) {
            Some(data) => merge_snapshot(snapshot, &data),
            None => snapshot,
        };
        let temp = format!("{}.tmp.{}", settings.file, settings.instance);
        std::fs::write(&temp, Value::Object(data).to_string())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &settings.file).map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => state.lock().unwrap().last_save = unix_time(),
        Err(e) => core
            .log(log::DEBUG, || format!("persistent-timer: save failed: {e}")),
    }
}

fn schedule_save(
    core: &mut Core,
    state: Arc<Mutex<State>>,
    settings: Arc<Settings>,
    interval: Duration,
) {
    core.after(interval, move |core| {
        if state.lock().unwrap().stopped {
            return;
        }
        save(core, &state, &settings);
        schedule_save(core, state, settings, interval);
    });
}

pub fn setup(core: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    let scope = s.choice(
        "scope",
        "workspace",
        &["workspace", "file", "filetype", "global"],
    )?;
    let mode = s.choice("mode", "all", &["active", "idle", "all"])?;
    let file = match s.get("file") {
        None => format!(
            "{}/extensions/persistent-timer/data.json",
            util::data_path()
        ),
        Some(Value::String(f)) if f.to_lowercase().ends_with(".json") => {
            util::normalize(f)
        }
        Some(_) => {
            return Err(
                "Invalid 'file value, must be a path ending with .json".into(),
            );
        }
    };
    let save_on: Vec<String> = match s.get("save-on") {
        None => vec!["focus-change".into(), "periodic".into()],
        Some(Value::Array(items)) => items.iter().map(to_display).collect(),
        Some(_) => return Err("Invalid 'save-on value, must be a list of 'focus-change and 'periodic".into()),
    };
    if save_on
        .iter()
        .any(|t| !["focus-change", "periodic", "exit"].contains(&t.as_str()))
    {
        return Err("Invalid 'save-on value, must be a list of 'focus-change and 'periodic".into());
    }
    let interval = s.number("save-interval", 30.0)?;
    if interval <= 0.0 {
        return Err(
            "Invalid 'save-interval value, must be a positive number".into()
        );
    }

    let state: Arc<Mutex<State>> = Arc::default();
    let settings = Arc::new(Settings {
        scope,
        file,
        mode,
        instance: format!("{}_{}", std::process::id(), unix_time()),
    });

    let mut ext = Extension::default();

    if save_on.iter().any(|t| t == "focus-change") {
        let (state, settings) = (Arc::clone(&state), Arc::clone(&settings));
        ext.hook("focus-lost", 50, move |core, _| {
            if unix_time().saturating_sub(state.lock().unwrap().last_save) >= 5
            {
                save(core, &state, &settings);
            }
            Value::Null
        });
    }
    if save_on.iter().any(|t| t == "periodic") {
        schedule_save(
            core,
            Arc::clone(&state),
            Arc::clone(&settings),
            Duration::from_secs_f64(interval),
        );
    }

    {
        let (state, settings) = (Arc::clone(&state), Arc::clone(&settings));
        ext.hook("post-activity", 10, move |core, args| {
            let (Some(opts), Some(activity)) = (args.first(), args.get(1))
            else {
                return Value::Null;
            };
            let mut s = state.lock().unwrap();
            if !s.loaded {
                match read_file(&settings.file) {
                    Some(data) => s.merge(&data),
                    None => core.log(log::DEBUG, || {
                        "persistent-timer: no readable timer file yet"
                            .to_string()
                    }),
                }
                s.loaded = true;
            }

            let text = |k: &str| {
                opts.get(k).and_then(Value::as_str).map(str::to_string)
            };
            let key = match settings.scope.as_str() {
                "workspace" => text("workspace-dir")
                    .unwrap_or_else(|| core.editor.cwd.clone()),
                "file" if !core.editor.path.is_empty() => {
                    core.editor.path.clone()
                }
                "file" => "CORD.UNKNOWN_FILE".to_string(),
                "filetype" => {
                    text("filetype").unwrap_or_else(|| "unknown".to_string())
                }
                _ => "CORD.GLOBAL".to_string(),
            };
            let idle =
                opts.get("idle?").and_then(Value::as_bool).unwrap_or(false);
            let run = match settings.mode.as_str() {
                "active" => (!idle).then_some("active"),
                "idle" => idle.then_some("idle"),
                _ => Some("all"),
            };

            let Some(run) = run else {
                if let Some(current) = s.current_key.take() {
                    s.stop(&current);
                }
                return Value::Null;
            };
            if s.current_key.as_deref() != Some(key.as_str()) {
                if let Some(current) = s.current_key.clone() {
                    s.stop(&current);
                }
                s.start(&key, run);
            }
            let b = s.bucket(&key, &settings.mode);
            let start = *b.display_start.get_or_insert(
                b.session_start
                    .unwrap_or_else(unix_time)
                    .saturating_sub(b.total),
            );
            let mut activity = activity.clone();
            activity["timestamps"]["start"] = json!(start);
            activity
        });
    }

    ext.shutdown = Some(Box::new(move |core| {
        save(core, &state, &settings);
        state.lock().unwrap().stopped = true;
    }));
    Ok(ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_keep_other_sessions() {
        let mut state = State::default();
        state.merge(
            &json!({ "/a": { "modes": { "all": { "total_time": 10 } } } }),
        );
        state.bucket("/a", "all").total += 5;
        let out = merge_snapshot(
            state.snapshot(),
            &json!({ "/a": { "modes": { "all": { "total_time": 12 } } }, "/b": { "modes": { "all": { "total_time": 7 } } } }),
        );
        assert_eq!(out["/a"]["modes"]["all"]["total_time"], 15);
        assert_eq!(out["/b"]["modes"]["all"]["total_time"], 7);
    }
}
