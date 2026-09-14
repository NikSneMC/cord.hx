//! `'scoped-timestamps`: keeps a separate elapsed-time counter per scope.
//!
//! Config:
//! * `'scope` - `'buffer` (default), `'workspace` or `'idle`
//! * `'pause?` - stop counting while the scope is not active (default #t)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{Extension, Setup};
use crate::plugin::core::Core;
use crate::plugin::util::unix_time;

#[derive(Default)]
struct State {
    /// Effective timestamp per key.
    timestamps: HashMap<String, u64>,
    /// Seconds spent inactive per key.
    inactive: HashMap<String, u64>,
    /// When the key was last active.
    last_active: HashMap<String, u64>,
    current_key: Option<String>,
    last_buffer: Option<String>,
}

impl State {
    fn deactivate(&mut self, key: &str) {
        if let Some(last) = self.last_active.remove(key) {
            let elapsed = unix_time().saturating_sub(last);
            *self.inactive.entry(key.to_string()).or_default() += elapsed;
            self.timestamps.remove(key);
        }
    }

    fn timestamp(&mut self, key: &str, pause: bool) -> u64 {
        if let Some(t) = self.timestamps.get(key)
            && (!pause || self.current_key.as_deref() == Some(key))
        {
            return *t;
        }
        if pause
            && let Some(current) = self.current_key.clone().filter(|c| c != key)
        {
            self.deactivate(&current);
        }
        let now = unix_time();
        let t = if pause {
            self.current_key = Some(key.to_string());
            self.last_active.insert(key.to_string(), now);
            now.saturating_sub(self.inactive.get(key).copied().unwrap_or(0))
        } else {
            now
        };
        self.timestamps.insert(key.to_string(), t);
        t
    }
}

pub fn setup(_: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    let scope =
        s.choice("scope", "buffer", &["buffer", "workspace", "idle"])?;
    let pause = s.bool("pause?", true)?;
    let state: Arc<Mutex<State>> = Arc::default();

    let key = move |core: &Core, opts: &Value| -> Option<String> {
        let text =
            |k: &str| opts.get(k).and_then(Value::as_str).map(str::to_string);
        match scope.as_str() {
            "buffer" if text("type").as_deref() == Some("language") => {
                Some(if core.editor.path.is_empty() {
                    "Cord.unknown".to_string()
                } else {
                    core.editor.path.clone()
                })
            }
            "buffer" => text("filetype"),
            "workspace" => text("workspace-dir"),
            _ => Some(
                if opts.get("idle?").and_then(Value::as_bool).unwrap_or(false) {
                    "Cord.idle".to_string()
                } else {
                    "Cord.active".to_string()
                },
            ),
        }
    };
    let key = Arc::new(key);

    let mut ext = Extension::default();
    {
        let (state, key) = (Arc::clone(&state), Arc::clone(&key));
        ext.variable("scoped-timestamp", move |core, args| {
            let opts = args.first().cloned().unwrap_or_default();
            match key(core, &opts) {
                Some(k) => json!(state.lock().unwrap().timestamp(&k, pause)),
                None => Value::Null,
            }
        });
    }

    if !s.base.timestamp_enabled {
        return Ok(ext);
    }

    {
        let (state, key) = (Arc::clone(&state), Arc::clone(&key));
        ext.hook("post-activity", 0, move |core, args| {
            let (Some(opts), Some(activity)) = (args.first(), args.get(1))
            else {
                return Value::Null;
            };
            let Some(k) = key(core, opts) else {
                return Value::Null;
            };
            let t = state.lock().unwrap().timestamp(&k, pause);
            let mut activity = activity.clone();
            activity["timestamps"]["start"] = json!(t);
            activity
        });
    }

    if pause {
        let state = Arc::clone(&state);
        ext.hook("buf-enter", 100, move |core, _| {
            let mut s = state.lock().unwrap();
            let buffer = Some(core.editor.path.clone());
            if s.last_buffer != buffer {
                s.last_buffer = buffer;
                if let Some(current) = s.current_key.take() {
                    s.deactivate(&current);
                }
            }
            Value::Null
        });
    }

    if s.base.reset_on_idle {
        for event in ["idle-enter", "idle-leave"] {
            let state = Arc::clone(&state);
            ext.hook(event, 50, move |_, _| {
                let mut s = state.lock().unwrap();
                s.timestamps.remove("Cord.idle");
                s.timestamps.remove("Cord.active");
                Value::Null
            });
        }
    }
    Ok(ext)
}
