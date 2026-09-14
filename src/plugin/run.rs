//! Cached command output for procedures, which must not block the editor thread.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::core::{self, Msg};
use super::util;

#[derive(Default)]
struct Entry {
    output: Option<String>,
    fetched: Option<Instant>,
    running: bool,
}

type Key = (Vec<String>, String);

static CACHE: LazyLock<Mutex<HashMap<Key, Entry>>> =
    LazyLock::new(Default::default);

/// Trimmed stdout of the last run of `command` in `cwd`; `None` while none has
/// finished or the last one failed. Stale entries are refreshed in the background.
pub fn run(command: Vec<String>, cwd: String, ttl: Duration) -> Option<String> {
    let key = (command, cwd);
    let mut cache = CACHE.lock().unwrap();
    let entry = cache.entry(key.clone()).or_default();
    let stale = entry.fetched.is_none_or(|t| t.elapsed() >= ttl);
    if stale && !entry.running && !key.0.is_empty() {
        entry.running = true;
        std::thread::spawn(move || refresh(key));
    }
    entry.output.clone()
}

fn refresh(key: Key) {
    let (command, cwd) = &key;
    let args: Vec<&str> = command[1..].iter().map(String::as_str).collect();
    let cwd = (!cwd.is_empty()).then_some(cwd.as_str());
    let output = util::run(&command[0], &args, cwd)
        .ok()
        .filter(|o| o.code == 0)
        .map(|o| o.stdout.trim().to_string());

    let changed = {
        let mut cache = CACHE.lock().unwrap();
        let entry = cache.entry(key.clone()).or_default();
        entry.running = false;
        entry.fetched = Some(Instant::now());
        let changed = entry.output != output;
        entry.output = output;
        changed
    };
    if changed {
        core::send(Msg::Run(Box::new(|core| {
            // Workspace rule predicates may depend on the output
            core.rule_cache.clear();
            if core.manager.active {
                core.apply_rule_visibility();
            }
            core.manager_queue_update(true);
        })));
    }
}
