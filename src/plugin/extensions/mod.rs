//! Extensions.
//!
//! An extension contributes variables, hooks, assets and configuration.
//! Built-in extensions are implemented natively; custom extensions are hash
//! maps from Scheme:
//!
//! ```scheme
//! 'extensions (list 'diagnostics                                   ; built-in, default config
//!                   (list 'tmux (hash 'interval 5000))              ; built-in with config
//!                   my-extension)                                  ; extension hash map
//! 'extensions (hash 'diagnostics (hash) 'tmux (hash 'interval 5000))
//! ```

mod diagnostics;
mod lastfm;
mod local_time;
mod multiplexer;
mod persistent_timer;
mod resolver;
mod scoped_timestamps;

use std::sync::Arc;

use serde_json::{Value, json};

use super::config::{self, Config, Hook, NativeFn, Natives};
use super::core::Core;
use super::log;
use super::value::to_display;
use super::value::{self, NATIVE_KEY};

pub type Shutdown = Box<dyn FnOnce(&mut Core) + Send>;

/// What an extension contributes.
#[derive(Default)]
pub struct Extension {
    pub hooks: Vec<Hook>,
    /// Configuration merged under the user configuration; may refer to
    /// `natives`.
    pub config: Value,
    pub natives: Vec<(String, NativeFn)>,
    pub shutdown: Option<Shutdown>,
}

impl Extension {
    /// Sets `path` of the configuration to a native procedure.
    fn native(
        &mut self,
        path: &[&str],
        f: impl Fn(&mut Core, &[Value]) -> Value + Send + Sync + 'static,
    ) {
        let name = path.join("/");
        value::set(&mut self.config, path, config::native_marker(&name));
        self.natives.push((name, Arc::new(f)));
    }

    pub fn variable(
        &mut self,
        name: &str,
        f: impl Fn(&mut Core, &[Value]) -> Value + Send + Sync + 'static,
    ) {
        self.native(&["variables", name], f);
    }

    /// Overrides a `'text` option.
    pub fn text(
        &mut self,
        key: &str,
        f: impl Fn(&mut Core, &[Value]) -> Value + Send + Sync + 'static,
    ) {
        self.native(&["text", key], f);
    }

    pub fn hook(
        &mut self,
        event: &str,
        priority: i64,
        f: impl Fn(&mut Core, &[Value]) -> Value + Send + Sync + 'static,
    ) {
        self.hooks.push(Hook {
            event: event.to_string(),
            priority,
            callback: config::Callable::native(f),
        });
    }
}

/// Setup context of a built-in extension.
pub struct Setup<'a> {
    /// The extension's configuration (an object, possibly empty).
    pub config: &'a Value,
    /// The configuration without extensions.
    pub base: &'a Config,
}

impl Setup<'_> {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.config.get(key).filter(|v| !v.is_null())
    }

    pub fn bool(&self, key: &str, default: bool) -> Result<bool, String> {
        match self.get(key) {
            None => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(format!("Invalid '{key} value, must be a boolean")),
        }
    }

    pub fn number(&self, key: &str, default: f64) -> Result<f64, String> {
        match self.get(key) {
            None => Ok(default),
            Some(v) => v.as_f64().ok_or_else(|| {
                format!("Invalid '{key} value, must be a number")
            }),
        }
    }

    /// An enumeration value (a symbol in Scheme).
    pub fn choice(
        &self,
        key: &str,
        default: &str,
        allowed: &[&str],
    ) -> Result<String, String> {
        let value = match self.get(key) {
            None => default.to_string(),
            Some(v) => to_display(v),
        };
        if allowed.contains(&value.as_str()) {
            Ok(value)
        } else {
            let names: Vec<String> =
                allowed.iter().map(|a| format!("'{a}")).collect();
            Err(format!(
                "Invalid '{key} value, must be {}",
                names.join(", ")
            ))
        }
    }
}

type SetupFn = fn(&mut Core, &Setup<'_>) -> Result<Extension, String>;

const BUILT_IN: &[(&str, SetupFn)] = &[
    ("diagnostics", diagnostics::setup),
    ("lastfm", lastfm::setup),
    ("local-time", local_time::setup),
    ("persistent-timer", persistent_timer::setup),
    ("resolver", resolver::setup),
    ("scoped-timestamps", scoped_timestamps::setup),
    ("tmux", multiplexer::setup_tmux),
    ("zellij", multiplexer::setup_zellij),
];

/// Cleanup of the active extensions.
#[derive(Default)]
pub struct Registry {
    shutdowns: Vec<Shutdown>,
}

impl Registry {
    pub fn shutdown(self, core: &mut Core) {
        for f in self.shutdowns {
            f(core);
        }
    }
}

enum Entry {
    BuiltIn(&'static str, SetupFn, Value),
    Custom(String, Value),
}

fn resolve_entry(entry: &Value, config: Value) -> Result<Entry, String> {
    match entry {
        Value::String(name) => BUILT_IN
            .iter()
            .find(|(n, _)| n == name)
            .map(|(n, f)| Entry::BuiltIn(n, *f, config))
            .ok_or_else(|| format!("Unknown built-in extension '{name}'")),
        Value::Array(pair) if pair.len() == 2 => resolve_entry(&pair[0], pair[1].clone()),
        Value::Object(ext) => Ok(Entry::Custom(
            ext.get("name").map(to_display).unwrap_or_else(|| "custom extension".to_string()),
            entry.clone(),
        )),
        _ => Err("Extension entry must be a symbol, a hash map, or (list extension config)".to_string()),
    }
}

fn entries(extensions: &Value) -> Result<Vec<Entry>, String> {
    match extensions {
        Value::Array(list) => {
            list.iter().map(|e| resolve_entry(e, Value::Null)).collect()
        }
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                resolve_entry(
                    &Value::String(k.clone()),
                    if v.is_object() {
                        v.clone()
                    } else {
                        Value::Null
                    },
                )
            })
            .collect(),
        _ => Ok(Vec::new()),
    }
}

fn custom(ext: &Value) -> Extension {
    let mut out = Extension::default();
    if let Some(Value::Object(hooks)) = ext.get("hooks") {
        for (event, spec) in hooks {
            if let Some((callback, priority)) = config::hook_spec(spec, 50) {
                out.hooks.push(Hook {
                    event: event.clone(),
                    priority,
                    callback,
                });
            }
        }
    }
    let mut layer = match ext.get("config") {
        Some(Value::Object(config)) => config.clone(),
        _ => Default::default(),
    };
    for key in ["variables", "hooks", "assets", "extensions"] {
        layer.remove(key);
    }
    for key in ["variables", "assets"] {
        if let Some(v @ Value::Object(_)) = ext.get(key) {
            layer.insert(key.to_string(), v.clone());
        }
    }
    out.config = Value::Object(layer);
    out
}

/// Prefixes the native procedure markers in `v`.
fn prefix_natives(v: &mut Value, prefix: &str) {
    match v {
        Value::Object(map) => match map.get_mut(NATIVE_KEY) {
            Some(Value::String(name)) => *name = format!("{prefix}{name}"),
            _ => map.values_mut().for_each(|v| prefix_natives(v, prefix)),
        },
        Value::Array(items) => {
            items.iter_mut().for_each(|v| prefix_natives(v, prefix))
        }
        _ => {}
    }
}

/// Sets up the configured extensions and builds the final configuration.
pub fn build(core: &mut Core) -> Result<(Config, Vec<Hook>, Registry), String> {
    let user = core.user_config.clone();
    let base = Config::build(&user, &[], &Natives::new())?;

    let mut layers = Vec::new();
    let mut natives = Natives::new();
    let mut hooks = Vec::new();
    let mut registry = Registry::default();

    for (i, entry) in entries(&base.extensions)?.into_iter().enumerate() {
        let (label, mut ext) = match entry {
            Entry::BuiltIn(name, setup, config) => {
                let config = if config.is_object() {
                    config
                } else {
                    json!({})
                };
                core.log(log::DEBUG, || {
                    format!("Setting up extension: {name}")
                });
                setup(
                    core,
                    &Setup {
                        config: &config,
                        base: &base,
                    },
                )
                .map_err(|e| format!("Extension '{name}' setup failed: {e}"))
                .map(|ext| (name.to_string(), ext))?
            }
            Entry::Custom(name, value) => (name, custom(&value)),
        };
        core.log(log::DEBUG, || format!("Loading extension: {label}"));

        // Natives are named after their path; keep extensions apart
        let prefix = format!("{i}:");
        prefix_natives(&mut ext.config, &prefix);
        natives.extend(
            ext.natives
                .into_iter()
                .map(|(n, f)| (format!("{prefix}{n}"), f)),
        );
        layers.push(ext.config);
        hooks.extend(ext.hooks);
        registry.shutdowns.extend(ext.shutdown);
    }

    let config = Config::build(&user, &layers, &natives)?;
    let mut all = config.hooks.clone();
    all.extend(hooks);
    Ok((config, all, registry))
}
