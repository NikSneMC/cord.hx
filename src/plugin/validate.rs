//! Validation of user configurations, reported by `:cord-health` and
//! `:cord-debug`.

use serde_json::Value;

use super::value::{self, get, type_name};

const TEXT_TYPES: &[&str] = &["string", "boolean", "function"];
const HOOK_TYPES: &[&str] = &["function", "hash"];
const ENUM_TYPES: &[&str] = &["string"];
const BOOL: &[&str] = &["boolean"];
const NUMBER: &[&str] = &["number"];
const STRING: &[&str] = &["string"];
const HASH: &[&str] = &["hash"];
const LIST: &[&str] = &["list"];
const STRING_OR_FUNCTION: &[&str] = &["string", "function"];

const FIELDS: &[(&str, &[&str])] = &[
    ("enabled?", BOOL),
    ("log-level", &["string", "number"]),
    ("editor", HASH),
    ("editor.client", ENUM_TYPES),
    ("editor.tooltip", STRING_OR_FUNCTION),
    ("editor.icon", STRING),
    ("display", HASH),
    ("display.theme", ENUM_TYPES),
    ("display.flavor", ENUM_TYPES),
    ("display.view", ENUM_TYPES),
    ("display.swap-fields?", BOOL),
    ("display.swap-icons?", BOOL),
    ("timestamp", HASH),
    ("timestamp.enabled?", BOOL),
    ("timestamp.reset-on-idle?", BOOL),
    ("timestamp.reset-on-change?", BOOL),
    ("timestamp.shared?", BOOL),
    ("idle", HASH),
    ("idle.enabled?", BOOL),
    ("idle.timeout", NUMBER),
    ("idle.show-status?", BOOL),
    ("idle.ignore-focus?", BOOL),
    ("idle.unidle-on-focus?", BOOL),
    ("idle.smart-idle?", BOOL),
    ("idle.details", STRING_OR_FUNCTION),
    ("idle.state", STRING_OR_FUNCTION),
    ("idle.tooltip", STRING_OR_FUNCTION),
    ("idle.icon", STRING_OR_FUNCTION),
    ("text", HASH),
    ("text.default", TEXT_TYPES),
    ("text.workspace", TEXT_TYPES),
    ("text.viewing", TEXT_TYPES),
    ("text.editing", TEXT_TYPES),
    ("text.file-browser", TEXT_TYPES),
    ("text.plugin-manager", TEXT_TYPES),
    ("text.lsp", TEXT_TYPES),
    ("text.docs", TEXT_TYPES),
    ("text.vcs", TEXT_TYPES),
    ("text.notes", TEXT_TYPES),
    ("text.debug", TEXT_TYPES),
    ("text.test", TEXT_TYPES),
    ("text.games", TEXT_TYPES),
    ("text.diagnostics", TEXT_TYPES),
    ("text.terminal", TEXT_TYPES),
    ("text.dashboard", TEXT_TYPES),
    ("buttons", LIST),
    ("assets", HASH),
    ("variables", &["boolean", "hash"]),
    ("extensions", &["list", "hash"]),
    ("workspace-rules", LIST),
    ("hooks", HASH),
    ("hooks.ready", HOOK_TYPES),
    ("hooks.shutdown", HOOK_TYPES),
    ("hooks.pre-activity", HOOK_TYPES),
    ("hooks.post-activity", HOOK_TYPES),
    ("hooks.idle-enter", HOOK_TYPES),
    ("hooks.idle-leave", HOOK_TYPES),
    ("hooks.focus-gained", HOOK_TYPES),
    ("hooks.focus-lost", HOOK_TYPES),
    ("hooks.workspace-change", HOOK_TYPES),
    ("hooks.buf-enter", HOOK_TYPES),
    ("advanced", HASH),
    ("advanced.plugin", HASH),
    ("advanced.plugin.autocmds?", BOOL),
    ("advanced.plugin.cursor-update", ENUM_TYPES),
    ("advanced.plugin.match-in-mappings?", BOOL),
    ("advanced.plugin.debounce", HASH),
    ("advanced.plugin.debounce.delay", NUMBER),
    ("advanced.plugin.debounce.interval", NUMBER),
    ("advanced.server", HASH),
    ("advanced.server.pipe-path", STRING),
    ("advanced.server.executable-path", STRING),
    ("advanced.server.timeout", NUMBER),
    ("advanced.discord", HASH),
    ("advanced.discord.pipe-paths", LIST),
    ("advanced.discord.reconnect", HASH),
    ("advanced.discord.reconnect.enabled?", BOOL),
    ("advanced.discord.reconnect.interval", NUMBER),
    ("advanced.discord.reconnect.initial?", BOOL),
    ("advanced.discord.sync", HASH),
    ("advanced.discord.sync.enabled?", BOOL),
    ("advanced.discord.sync.mode", ENUM_TYPES),
    ("advanced.discord.sync.interval", NUMBER),
    ("advanced.discord.sync.reset-on-update?", BOOL),
    ("advanced.discord.sync.pad?", BOOL),
    ("advanced.workspace", HASH),
    ("advanced.workspace.root-markers", LIST),
    ("advanced.workspace.limit-to-cwd?", BOOL),
];

/// Sections that are not checked entry by entry.
const OPAQUE: &[&str] = &[
    "buttons",
    "extensions",
    "workspace-rules",
    "advanced.discord.pipe-paths",
    "advanced.workspace.root-markers",
];

/// Sections whose keys are user-defined, with the types of their values.
const DICTS: &[(&str, &[&str])] = &[
    ("assets", &["string", "hash"]),
    ("variables", &["string", "function"]),
];

pub struct ValidationError {
    pub msg: String,
    pub hint: String,
}

pub struct Validation {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<String>,
}

impl Validation {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }
}

fn field_types(path: &str) -> Option<&'static [&'static str]> {
    FIELDS.iter().find(|(p, _)| *p == path).map(|(_, t)| *t)
}

pub fn validate(user: &Value) -> Validation {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let mut type_error = |path: &str, v: &Value, allowed: &[&str]| {
        errors.push(ValidationError {
            msg: format!("Invalid type '{}' for `{path}`", type_name(v)),
            hint: format!("Allowed types: '{}'", allowed.join("', '")),
        });
    };

    fn walk(
        config: &Value,
        prefix: &str,
        warnings: &mut Vec<String>,
        type_error: &mut dyn FnMut(&str, &Value, &[&str]),
    ) {
        let Some(map) = config.as_object().filter(|_| !value::is_proc(config))
        else {
            return;
        };
        for (k, v) in map {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            let base = path.split('.').next().unwrap_or_default();
            if let Some((_, types)) = DICTS.iter().find(|(d, _)| *d == base) {
                if path != base {
                    if !types.contains(&type_name(v)) {
                        type_error(&path, v, types);
                    }
                } else if let Some(entries) =
                    v.as_object().filter(|_| !value::is_proc(v))
                {
                    for (k, entry) in entries {
                        if !types.contains(&type_name(entry)) {
                            type_error(&format!("{path}.{k}"), entry, types);
                        }
                    }
                }
            } else if field_types(&path).is_none() {
                warnings.push(format!("Unknown configuration entry: `{path}`"));
            } else if v.is_object()
                && !OPAQUE.contains(&path.as_str())
                && prefix != "hooks"
            {
                walk(v, &path, warnings, type_error);
            }
        }
    }

    walk(user, "", &mut warnings, &mut type_error);

    for (path, allowed) in FIELDS {
        let parts: Vec<&str> = path.split('.').collect();
        if let Some(v) = get(user, &parts)
            && !allowed.contains(&type_name(v))
        {
            type_error(path, v, allowed);
        }
    }

    Validation { errors, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validation() {
        let v = validate(&json!({
            "idle": { "timeout": "soon", "bogus": 1 },
            "assets": { "rs": 1 },
            "hooks": { "ready": { "__cord_proc": 1 } }
        }));
        let msgs: Vec<_> = v.errors.iter().map(|e| e.msg.as_str()).collect();
        assert!(msgs.contains(&"Invalid type 'number' for `assets.rs`"));
        assert!(msgs.contains(&"Invalid type 'string' for `idle.timeout`"));
        assert_eq!(
            v.warnings,
            vec!["Unknown configuration entry: `idle.bogus`"]
        );
    }
}
