//! Configuration.
//!
//! The Scheme configuration arrives as a JSON value with kebab-case keys
//! (boolean options end in `?`) and procedures as markers. It is merged over
//! the configuration of extensions and the defaults, then read into [`Config`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use regex::Regex;
use serde_json::{Value, json};

use super::core::Core;
use super::icons;
use super::log::parse_level;
use super::util;
use super::value::{self, NATIVE_KEY, get, proc_id, to_display};

pub type NativeFn = Arc<dyn Fn(&mut Core, &[Value]) -> Value + Send + Sync>;

/// Native procedures referenced by `{"__cord_native": name}` markers.
pub type Natives = HashMap<String, NativeFn>;

/// A procedure: either defined in Scheme or implemented natively.
#[derive(Clone)]
pub enum Callable {
    Proc(u64),
    Native(NativeFn),
}

impl Callable {
    pub fn native(
        f: impl Fn(&mut Core, &[Value]) -> Value + Send + Sync + 'static,
    ) -> Self {
        Callable::Native(Arc::new(f))
    }
}

/// An option value: plain data or a procedure receiving `opts`.
#[derive(Clone)]
pub enum Opt {
    Value(Value),
    Call(Callable),
}

impl Opt {
    /// Reads a value; procedure markers become procedures.
    pub fn parse(v: &Value) -> Opt {
        Opt::resolve(v, &Natives::new())
    }

    fn resolve(v: &Value, natives: &Natives) -> Opt {
        if let Some(id) = proc_id(v) {
            return Opt::Call(Callable::Proc(id));
        }
        match v.get(NATIVE_KEY).and_then(Value::as_str) {
            Some(name) => natives
                .get(name)
                .or_else(|| DEFAULT_NATIVES.get(name))
                .map(|f| Opt::Call(Callable::Native(Arc::clone(f))))
                .unwrap_or_else(Opt::none),
            None => Opt::Value(v.clone()),
        }
    }

    pub fn str(s: &str) -> Opt {
        Opt::Value(Value::String(s.to_string()))
    }

    pub fn none() -> Opt {
        Opt::Value(Value::Null)
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Opt::Value(Value::Null))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Full,
    Editor,
    Asset,
    Auto,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorUpdate {
    OnHold,
    OnMove,
    None,
}

#[derive(Clone)]
pub struct Button {
    pub label: Opt,
    pub url: Opt,
}

#[derive(Clone)]
pub enum Asset {
    Icon(String),
    Custom {
        name: Opt,
        icon: Opt,
        tooltip: Opt,
        text: Opt,
        ty: Option<String>,
    },
}

/// Text of a workspace rule field.
#[derive(Clone)]
pub enum RuleText {
    /// Not set: keep the default text.
    Keep,
    /// `#f`: omit the line.
    Omit,
    Set(Opt),
}

/// A workspace rule. All matchers it sets have to match.
#[derive(Clone)]
pub struct WorkspaceRule {
    /// Workspace directories, normalized.
    pub paths: Vec<String>,
    pub names: Vec<String>,
    /// Regexes matched against the buffer path (the workspace directory for
    /// buffers without a file).
    pub globs: Vec<Regex>,
    /// Procedure receiving `(hash 'workspace 'workspace-dir 'filename 'path)`.
    pub predicate: Option<u64>,
    pub visible: bool,
    pub details: RuleText,
    pub state: RuleText,
}

#[derive(Clone)]
pub struct Hook {
    pub event: String,
    pub priority: i64,
    pub callback: Callable,
}

pub const HOOK_EVENTS: &[&str] = &[
    "ready",
    "shutdown",
    "pre-activity",
    "post-activity",
    "idle-enter",
    "idle-leave",
    "focus-gained",
    "focus-lost",
    "workspace-change",
    "buf-enter",
];

pub const TEXT_KEYS: &[&str] = &[
    "workspace",
    "viewing",
    "editing",
    "file-browser",
    "plugin-manager",
    "lsp",
    "docs",
    "vcs",
    "notes",
    "debug",
    "test",
    "diagnostics",
    "games",
    "terminal",
    "dashboard",
];

pub fn native_marker(name: &str) -> Value {
    json!({ NATIVE_KEY: name })
}

/// `"<prefix><opts[key]>"`
pub fn text_native(prefix: &'static str, key: &'static str) -> NativeFn {
    Arc::new(move |_, args| {
        let v = args.first().and_then(|o| o.get(key)).cloned();
        json!(format!("{prefix}{}", to_display(&v.unwrap_or(json!("")))))
    })
}

const DEFAULT_TEXTS: &[(&str, &str, &str)] = &[
    ("viewing", "Viewing ", "filename"),
    ("editing", "Editing ", "filename"),
    ("file-browser", "Browsing files in ", "name"),
    ("plugin-manager", "Managing plugins in ", "name"),
    ("lsp", "Configuring LSP in ", "name"),
    ("docs", "Reading ", "name"),
    ("vcs", "Committing changes in ", "name"),
    ("notes", "Taking notes in ", "name"),
    ("debug", "Debugging in ", "name"),
    ("test", "Testing in ", "name"),
    ("diagnostics", "Fixing problems in ", "name"),
    ("games", "Playing ", "name"),
    ("terminal", "Running commands in ", "name"),
];

static DEFAULT_NATIVES: LazyLock<Natives> = LazyLock::new(|| {
    let mut natives = Natives::new();
    let workspace: NativeFn = Arc::new(|_, args| {
        let opts = args.first().cloned().unwrap_or_default();
        let name = opts.get("workspace").cloned().unwrap_or(json!(""));
        let worktree = match opts.get("worktree") {
            Some(Value::String(w)) => format!(" (worktree {w})"),
            _ => String::new(),
        };
        json!(format!("In {}{worktree}", to_display(&name)))
    });
    natives.insert("text/workspace".into(), workspace);
    for (key, prefix, field) in DEFAULT_TEXTS {
        natives.insert(format!("text/{key}"), text_native(prefix, field));
    }
    natives
});

static DEFAULTS: LazyLock<Value> = LazyLock::new(|| {
    let mut text = serde_json::Map::new();
    text.insert("workspace".into(), native_marker("text/workspace"));
    for (key, _, _) in DEFAULT_TEXTS {
        text.insert(key.to_string(), native_marker(&format!("text/{key}")));
    }
    text.insert("dashboard".into(), json!("Home"));

    json!({
        "enabled?": true,
        "log-level": "off",
        "editor": { "client": "auto" },
        "display": {
            "theme": "default", "flavor": "dark", "view": "full",
            "swap-fields?": false, "swap-icons?": false
        },
        "timestamp": {
            "enabled?": true, "reset-on-idle?": false,
            "reset-on-change?": false, "shared?": false
        },
        "idle": {
            "enabled?": true, "timeout": 300000, "show-status?": true,
            "ignore-focus?": true, "unidle-on-focus?": true,
            "smart-idle?": true, "details": "Idling", "tooltip": "💤"
        },
        "text": text,
        "buttons": [],
        "hooks": {},
        "extensions": [],
        "workspace-rules": [],
        "advanced": {
            "plugin": {
                "autocmds?": true, "cursor-update": "on-hold",
                "match-in-mappings?": true,
                "debounce": { "delay": 50, "interval": 750 }
            },
            "server": { "timeout": 300000 },
            "discord": {
                "pipe-paths": [],
                "reconnect": { "enabled?": false, "interval": 5000, "initial?": true },
                "sync": {
                    "enabled?": false, "mode": "periodic", "interval": 12000,
                    "reset-on-update?": true, "pad?": false
                }
            },
            "workspace": {
                "root-markers": [".git", ".hg", ".svn"],
                "limit-to-cwd?": false
            }
        }
    })
});

#[derive(Clone)]
pub struct Config {
    pub enabled: bool,
    pub log_level: u8,

    pub client_id: String,
    pub editor_tooltip: Opt,
    pub editor_icon: String,

    pub theme: String,
    pub flavor: String,
    pub view: View,
    pub swap_fields: bool,
    pub swap_icons: bool,

    pub timestamp_enabled: bool,
    pub reset_on_idle: bool,
    pub reset_on_change: bool,
    pub shared_timestamp: bool,

    pub idle_enabled: bool,
    pub idle_timeout: u64,
    pub idle_show_status: bool,
    pub ignore_focus: bool,
    pub unidle_on_focus: bool,
    pub smart_idle: bool,
    pub idle_details: Opt,
    pub idle_state: Opt,
    pub idle_tooltip: Opt,
    pub idle_icon: Opt,

    pub text: BTreeMap<String, Opt>,
    pub buttons: Vec<Button>,
    pub assets: HashMap<String, Asset>,
    /// Whether `${...}` placeholders are substituted.
    pub variables_enabled: bool,
    pub variables: HashMap<String, Opt>,
    pub hooks: Vec<Hook>,
    pub workspace_rules: Vec<WorkspaceRule>,
    pub extensions: Value,

    pub autocmds: bool,
    pub cursor_update: CursorUpdate,
    pub match_in_mappings: bool,
    pub debounce_delay: u64,
    pub debounce_interval: u64,

    pub pipe_path: Option<String>,
    pub executable_path: Option<String>,
    pub server_timeout: u64,

    pub discord_pipe_paths: Vec<String>,
    pub reconnect_enabled: bool,
    pub reconnect_interval: u64,
    pub reconnect_initial: bool,
    pub sync_enabled: bool,
    pub sync_mode: String,
    pub sync_interval: u64,
    pub sync_reset_on_update: bool,
    pub sync_pad: bool,

    pub root_markers: Vec<String>,
    pub limit_to_cwd: bool,

    /// The defaults with the extension and user configurations merged in.
    pub merged: Value,
}

impl Default for Config {
    fn default() -> Self {
        Config::build(&Value::Null, &[], &Natives::new())
            .expect("the default configuration is valid")
    }
}

/// Priority of a hook: a number or `'highest` .. `'lowest`.
pub fn priority(v: Option<&Value>, default: i64) -> i64 {
    match v {
        Some(Value::Number(n)) => {
            n.as_f64().map(|f| f as i64).unwrap_or(default)
        }
        Some(Value::String(s)) => match s.as_str() {
            "highest" => 100,
            "high" => 75,
            "normal" => 50,
            "low" => 25,
            "lowest" => 0,
            _ => default,
        },
        _ => default,
    }
}

/// `(callback, priority)` of a hook given as a procedure or as
/// `(hash 'callback f 'priority n)`.
pub fn hook_spec(v: &Value, default_priority: i64) -> Option<(Callable, i64)> {
    if let Some(id) = proc_id(v) {
        return Some((Callable::Proc(id), default_priority));
    }
    let id = proc_id(v.get("callback")?)?;
    Some((
        Callable::Proc(id),
        priority(v.get("priority"), default_priority),
    ))
}

fn rule_text(rule: &Value, key: &str) -> RuleText {
    match rule.get(key) {
        None | Some(Value::Null) => RuleText::Keep,
        Some(Value::Bool(false)) => RuleText::Omit,
        Some(v) => RuleText::Set(Opt::parse(v)),
    }
}

/// Expands a leading `~` (optionally after `^`) in a path regex to the home
/// directory.
pub fn expand_path_regex(re: &str) -> String {
    let home = regex::escape(&util::home_dir());
    if let Some(rest) = re.strip_prefix("^~") {
        format!("^{home}{rest}")
    } else if let Some(rest) = re.strip_prefix('~') {
        format!("{home}{rest}")
    } else {
        re.to_string()
    }
}

/// A string or a list of strings.
fn one_or_many(v: Option<&Value>, what: &str) -> Result<Vec<String>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("Invalid {what}: {}", to_display(i)))
            })
            .collect(),
        Some(other) => Err(format!("Invalid {what}: {}", to_display(other))),
    }
}

fn workspace_rule(rule: &Value) -> Result<WorkspaceRule, String> {
    let globs = one_or_many(rule.get("glob"), "workspace rule 'glob")?
        .iter()
        .map(|src| {
            Regex::new(&expand_path_regex(src)).map_err(|e| {
                format!("Invalid workspace rule 'glob: {src} ({e})")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let out = WorkspaceRule {
        paths: one_or_many(rule.get("path"), "workspace rule 'path")?
            .iter()
            .map(|p| util::normalize(p))
            .collect(),
        names: one_or_many(rule.get("name"), "workspace rule 'name")?,
        globs,
        predicate: rule.get("predicate").and_then(proc_id),
        visible: rule
            .get("visible?")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        details: rule_text(rule, "details"),
        state: rule_text(rule, "state"),
    };
    if out.paths.is_empty()
        && out.names.is_empty()
        && out.globs.is_empty()
        && out.predicate.is_none()
    {
        return Err(
            "Each workspace rule needs a 'path, 'name, 'glob or 'predicate"
                .into(),
        );
    }
    Ok(out)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn one_of<'a>(
    v: &'a str,
    allowed: &[&str],
    what: &str,
) -> Result<&'a str, String> {
    if allowed.contains(&v) {
        Ok(v)
    } else {
        let names: Vec<String> =
            allowed.iter().map(|a| format!("'{a}")).collect();
        Err(format!("{what} must be one of {}", names.join(", ")))
    }
}

impl Config {
    /// Merges `user` over the extension `layers` (the first one wins) and the
    /// defaults, then fills unset text keys from `'text 'default`.
    pub fn build(
        user: &Value,
        layers: &[Value],
        natives: &Natives,
    ) -> Result<Config, String> {
        let mut merged = DEFAULTS.clone();
        for layer in layers.iter().rev() {
            value::deep_merge(&mut merged, layer);
        }

        let mut user = if user.is_object() {
            user.clone()
        } else {
            json!({})
        };
        // `'variables #t` must not replace the variables of extensions
        if get(&user, &["variables"]) == Some(&Value::Bool(true)) {
            user.as_object_mut().unwrap().remove("variables");
        }
        value::deep_merge(&mut merged, &user);

        if let Some(default) = get(&user, &["text", "default"]).cloned() {
            for key in TEXT_KEYS {
                if get(&user, &["text", key]).is_none() {
                    value::set(&mut merged, &["text", key], default.clone());
                }
            }
        }

        Config::read(merged, natives)
    }

    fn read(merged: Value, natives: &Natives) -> Result<Config, String> {
        let at = |path: &[&str]| get(&merged, path);
        let bool = |path: &[&str]| {
            at(path)
                .and_then(Value::as_bool)
                .or_else(|| get(&DEFAULTS, path).and_then(Value::as_bool))
                .unwrap_or(false)
        };
        let int = |path: &[&str]| {
            at(path)
                .and_then(Value::as_f64)
                .or_else(|| get(&DEFAULTS, path).and_then(Value::as_f64))
                .map(|n| n.max(0.0) as u64)
                .unwrap_or(0)
        };
        let string = |path: &[&str]| {
            at(path)
                .map(to_display)
                .or_else(|| get(&DEFAULTS, path).map(to_display))
                .unwrap_or_default()
        };
        let opt = |path: &[&str]| {
            at(path)
                .map(|v| Opt::resolve(v, natives))
                .unwrap_or_else(Opt::none)
        };
        let entries = |path: &[&str]| {
            at(path)
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter().filter(|(_, v)| !v.is_null()).collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        let log_level = match super::log::env_level()
            .or_else(|| at(&["log-level"]).and_then(parse_level))
        {
            Some(level) => level,
            None => {
                return Err(format!(
                    "Unknown log level: {}",
                    string(&["log-level"])
                ));
            }
        };

        let view = match one_of(
            &string(&["display", "view"]),
            &["auto", "editor", "asset", "full"],
            "display.view",
        )? {
            "editor" => View::Editor,
            "asset" => View::Asset,
            "auto" => View::Auto,
            _ => View::Full,
        };

        let cursor_update = match one_of(
            &string(&["advanced", "plugin", "cursor-update"]),
            &["on-hold", "on-move", "none"],
            "advanced.plugin.cursor-update",
        )? {
            "on-move" => CursorUpdate::OnMove,
            "none" => CursorUpdate::None,
            _ => CursorUpdate::OnHold,
        };

        let buttons: Vec<Button> = at(&["buttons"])
            .and_then(Value::as_array)
            .map(|buttons| {
                buttons
                    .iter()
                    .map(|b| Button {
                        label: b
                            .get("label")
                            .map(Opt::parse)
                            .unwrap_or_else(Opt::none),
                        url: b
                            .get("url")
                            .map(Opt::parse)
                            .unwrap_or_else(Opt::none),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if buttons.len() > 2 {
            return Err("There cannot be more than 2 buttons".into());
        }
        let url_re = Regex::new(r"^https?://[^\s]+$").unwrap();
        for button in &buttons {
            if button.label.is_none() || button.url.is_none() {
                return Err("Each button must have a label and a URL".into());
            }
            if let Opt::Value(Value::String(url)) = &button.url
                && !url_re.is_match(url)
            {
                return Err(format!("`{url}` is not a valid button URL"));
            }
        }

        let client = string(&["editor", "client"]);
        let preset = match client.as_str() {
            "auto" => Some(icons::detect_client()),
            _ if !client.is_empty()
                && client.chars().all(|c| c.is_ascii_digit()) =>
            {
                None
            }
            name => match icons::client(name) {
                Some(c) => Some(c),
                None => return Err(format!("Unknown client: {client}")),
            },
        };
        let default = preset.unwrap_or_else(|| icons::client("helix").unwrap());
        let client_id = preset.map_or(client, |c| c.id.to_string());
        let theme = string(&["display", "theme"]);
        let flavor = string(&["display", "flavor"]);
        let mut editor_icon = string(&["editor", "icon"]);
        if editor_icon.is_empty() {
            editor_icon = icons::icon_url(default.icon, &theme, &flavor);
        }
        let mut editor_tooltip = opt(&["editor", "tooltip"]);
        if editor_tooltip.is_none() {
            editor_tooltip = Opt::str(default.tooltip);
        }

        let mut idle_icon = opt(&["idle", "icon"]);
        if idle_icon.is_none() {
            idle_icon = Opt::str(&icons::icon_url(
                icons::DEFAULT_IDLE_ICON,
                &theme,
                &flavor,
            ));
        }

        let sync_enabled = bool(&["advanced", "discord", "sync", "enabled?"]);
        let sync_mode = string(&["advanced", "discord", "sync", "mode"]);
        if sync_enabled {
            one_of(
                &sync_mode,
                &["periodic", "defer"],
                "advanced.discord.sync.mode",
            )?;
        }

        let workspace_rules = match at(&["workspace-rules"]) {
            None => Vec::new(),
            Some(Value::Array(rules)) => rules
                .iter()
                .map(workspace_rule)
                .collect::<Result<_, String>>()?,
            Some(_) => return Err("workspace-rules must be a list".into()),
        };

        let assets = entries(&["assets"])
            .into_iter()
            .filter_map(|(k, a)| {
                let asset = match a {
                    Value::String(icon) => Asset::Icon(icon.clone()),
                    Value::Object(_) => {
                        let field = |key: &str| {
                            a.get(key)
                                .map(|v| Opt::resolve(v, natives))
                                .unwrap_or_else(Opt::none)
                        };
                        Asset::Custom {
                            name: field("name"),
                            icon: field("icon"),
                            tooltip: field("tooltip"),
                            text: field("text"),
                            ty: a
                                .get("type")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        }
                    }
                    _ => return None,
                };
                Some((k.clone(), asset))
            })
            .collect();

        let hooks = entries(&["hooks"])
            .into_iter()
            .filter_map(|(event, spec)| {
                let (callback, priority) = hook_spec(spec, 200)?;
                Some(Hook {
                    event: event.clone(),
                    priority,
                    callback,
                })
            })
            .collect();

        let optional = |path: &[&str]| {
            at(path).and_then(Value::as_str).map(str::to_string)
        };

        Ok(Config {
            enabled: bool(&["enabled?"]),
            log_level,
            client_id,
            editor_tooltip,
            editor_icon,
            view,
            swap_fields: bool(&["display", "swap-fields?"]),
            swap_icons: bool(&["display", "swap-icons?"]),
            timestamp_enabled: bool(&["timestamp", "enabled?"]),
            reset_on_idle: bool(&["timestamp", "reset-on-idle?"]),
            reset_on_change: bool(&["timestamp", "reset-on-change?"]),
            shared_timestamp: bool(&["timestamp", "shared?"]),
            idle_enabled: bool(&["idle", "enabled?"]),
            idle_timeout: int(&["idle", "timeout"]),
            idle_show_status: bool(&["idle", "show-status?"]),
            ignore_focus: bool(&["idle", "ignore-focus?"]),
            unidle_on_focus: bool(&["idle", "unidle-on-focus?"]),
            smart_idle: bool(&["idle", "smart-idle?"]),
            idle_details: opt(&["idle", "details"]),
            idle_state: opt(&["idle", "state"]),
            idle_tooltip: opt(&["idle", "tooltip"]),
            idle_icon,
            text: entries(&["text"])
                .into_iter()
                .map(|(k, v)| (k.clone(), Opt::resolve(v, natives)))
                .collect(),
            buttons,
            assets,
            variables_enabled: !matches!(
                at(&["variables"]),
                Some(Value::Bool(false))
            ),
            variables: entries(&["variables"])
                .into_iter()
                .map(|(k, v)| (k.clone(), Opt::resolve(v, natives)))
                .collect(),
            hooks,
            workspace_rules,
            extensions: at(&["extensions"]).cloned().unwrap_or(json!([])),
            autocmds: bool(&["advanced", "plugin", "autocmds?"]),
            cursor_update,
            match_in_mappings: bool(&[
                "advanced",
                "plugin",
                "match-in-mappings?",
            ]),
            debounce_delay: int(&["advanced", "plugin", "debounce", "delay"]),
            debounce_interval: int(&[
                "advanced", "plugin", "debounce", "interval",
            ]),
            pipe_path: optional(&["advanced", "server", "pipe-path"]),
            executable_path: optional(&[
                "advanced",
                "server",
                "executable-path",
            ]),
            server_timeout: int(&["advanced", "server", "timeout"]),
            discord_pipe_paths: strings(at(&[
                "advanced",
                "discord",
                "pipe-paths",
            ])),
            reconnect_enabled: bool(&[
                "advanced",
                "discord",
                "reconnect",
                "enabled?",
            ]),
            reconnect_interval: int(&[
                "advanced",
                "discord",
                "reconnect",
                "interval",
            ]),
            reconnect_initial: bool(&[
                "advanced",
                "discord",
                "reconnect",
                "initial?",
            ]),
            sync_enabled,
            sync_mode,
            sync_interval: int(&["advanced", "discord", "sync", "interval"]),
            sync_reset_on_update: bool(&[
                "advanced",
                "discord",
                "sync",
                "reset-on-update?",
            ]),
            sync_pad: bool(&["advanced", "discord", "sync", "pad?"]),
            root_markers: strings(at(&[
                "advanced",
                "workspace",
                "root-markers",
            ])),
            limit_to_cwd: bool(&["advanced", "workspace", "limit-to-cwd?"]),
            theme,
            flavor,
            merged,
        })
    }

    /// Theme and flavor problems, reported without rejecting the configuration.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !icons::THEMES.contains(&self.theme.as_str()) {
            out.push(format!("Unknown theme: {}", self.theme));
        }
        if !icons::FLAVORS.contains(&self.flavor.as_str()) {
            out.push(format!("Unknown flavor: {}", self.flavor));
        }
        out
    }

    pub fn icon(&self, name: &str) -> String {
        icons::icon_url(name, &self.theme, &self.flavor)
    }

    pub fn pipe_path(&self) -> String {
        self.pipe_path
            .clone()
            .unwrap_or_else(|| util::default_pipe_path().to_string())
    }

    pub fn executable(&self) -> String {
        util::executable_path(self.executable_path.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_and_user_config() {
        let user = json!({
            "display": { "theme": "catppuccin", "swap-fields?": true },
            "idle": { "timeout": 1000 },
            "text": { "default": "Hacking", "editing": { "__cord_proc": 3 } },
            "advanced": { "plugin": { "cursor-update": "on-move" } }
        });
        let layer = json!({ "idle": { "timeout": 5 }, "display": { "flavor": "light" } });
        let config = Config::build(&user, &[layer], &Natives::new()).unwrap();
        assert_eq!(config.theme, "catppuccin");
        assert_eq!(config.flavor, "light");
        assert!(config.swap_fields);
        assert_eq!(config.idle_timeout, 1000);
        assert_eq!(config.cursor_update, CursorUpdate::OnMove);
        assert_eq!(config.client_id, icons::client("helix").unwrap().id);
        assert!(matches!(
            config.text["editing"],
            Opt::Call(Callable::Proc(3))
        ));
        assert!(
            matches!(&config.text["viewing"], Opt::Value(Value::String(s)) if s == "Hacking")
        );
    }

    #[test]
    fn rejects_invalid_configs() {
        assert!(
            Config::build(
                &json!({ "editor": { "client": "vim" } }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
        assert!(
            Config::build(
                &json!({ "log-level": "loud" }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
        assert!(
            Config::build(
                &json!({ "buttons": [{ "label": "a", "url": "ftp://x" }] }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
        assert!(
            Config::build(
                &json!({ "workspace-rules": [{ "glob": "(" }] }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
        assert!(
            Config::build(
                &json!({ "editor": { "client": "123" } }),
                &[],
                &Natives::new()
            )
            .is_ok()
        );
    }

    #[test]
    fn clients() {
        let helix = icons::client("helix").unwrap();
        let config = Config::build(&json!({}), &[], &Natives::new()).unwrap();
        assert_eq!(config.client_id, helix.id);
        assert!(
            matches!(&config.editor_tooltip, Opt::Value(Value::String(t)) if t == "Helix")
        );
        let config = Config::build(
            &json!({ "editor": { "client": "123", "tooltip": "Mine" } }),
            &[],
            &Natives::new(),
        )
        .unwrap();
        assert_eq!(config.client_id, "123");
        assert!(config.editor_icon.contains("/default/dark/helix.png"));
        let config = Config::build(
            &json!({ "editor": { "client": "nova" } }),
            &[],
            &Natives::new(),
        )
        .unwrap();
        assert_eq!(config.client_id, icons::client("nova").unwrap().id);
        assert!(
            Config::build(
                &json!({ "editor": { "client": "vscode" } }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
    }

    #[test]
    fn workspace_rules() {
        let home = util::home_dir();
        let config = Config::build(
            &json!({ "workspace-rules": [
                { "glob": ["^~/work", "x"], "state": false },
                { "path": "~/secret/", "name": ["a", "b"], "visible?": false },
            ] }),
            &[],
            &Natives::new(),
        )
        .unwrap();
        let rule = &config.workspace_rules[0];
        assert!(rule.globs[0].is_match(&format!("{home}/work/a")));
        assert!(matches!(rule.state, RuleText::Omit));
        assert!(matches!(rule.details, RuleText::Keep));
        assert!(rule.visible);
        let rule = &config.workspace_rules[1];
        assert_eq!(rule.paths, vec![format!("{home}/secret")]);
        assert_eq!(rule.names, vec!["a", "b"]);
        assert!(!rule.visible);
        assert!(
            Config::build(
                &json!({ "workspace-rules": [{ "state": "x" }] }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
        assert!(
            Config::build(
                &json!({ "workspace-rules": [{ "glob": "(" }] }),
                &[],
                &Natives::new()
            )
            .is_err()
        );
    }
}
