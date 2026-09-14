//! Activity builder.
//!
//! `opts` is a JSON object with kebab-case keys, as seen by Scheme
//! procedures: `filename`, `filetype`, `workspace`, `cursor-line`, ...

use serde_json::{Map, Value, json};

use super::config::{Asset, Callable, Opt, RuleText, View, WorkspaceRule};
use super::core::Core;
use super::icons::{self, Lookup};
use super::log;
use super::util;
use super::value::{self, to_display};
use super::workspace::Workspace;

/// Result of building an activity.
pub enum Built {
    /// A text option returned `#t`: skip the update.
    Skip,
    /// A text option returned `#f`: clear the activity.
    Clear,
    Activity(Value),
}

fn get_str<'a>(opts: &'a Value, key: &str) -> &'a str {
    opts.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn insert(opts: &mut Value, key: &str, v: Value) {
    if let Value::Object(map) = opts {
        map.insert(key.to_string(), v);
    }
}

fn blank_to_null(v: Value) -> Value {
    match v {
        Value::String(s) if s.is_empty() => Value::Null,
        other => other,
    }
}

impl Core {
    /// The first workspace rule matching the current workspace and buffer.
    pub fn workspace_rule(&mut self) -> Option<WorkspaceRule> {
        let ws = self.manager.current_workspace()?;
        let key = (ws.dir.clone(), self.editor.path.clone());
        let index = match self.rule_cache.get(&key) {
            Some(index) => *index,
            None => {
                let index = (0..self.config.workspace_rules.len())
                    .find(|&i| self.rule_matches(i, &ws));
                self.rule_cache.insert(key, index);
                index
            }
        };
        index.map(|i| self.config.workspace_rules[i].clone())
    }

    fn rule_matches(&mut self, index: usize, ws: &Workspace) -> bool {
        let rule = &self.config.workspace_rules[index];
        let path = &self.editor.path;
        let target = if path.is_empty() { &ws.dir } else { path };
        let same_dir = |p: &String| {
            *p == ws.dir
                || util::realpath(p)
                    .is_some_and(|r| Some(r) == util::realpath(&ws.dir))
        };
        if (!rule.paths.is_empty() && !rule.paths.iter().any(same_dir))
            || (!rule.names.is_empty() && !rule.names.contains(&ws.name))
            || (!rule.globs.is_empty()
                && !rule.globs.iter().any(|re| re.is_match(target)))
        {
            return false;
        }
        let Some(predicate) = rule.predicate else {
            return true;
        };
        let context = json!({
            "workspace": ws.name,
            "workspace-dir": ws.dir,
            "filename": util::tail(path),
            "path": path,
        });
        self.call(&Callable::Proc(predicate), &[context])
            .is_ok_and(|v| !v.is_null() && v != Value::Bool(false))
    }

    /// `opts` extended with the configured variables. Scheme procedures are
    /// passed along; native variables are evaluated.
    pub fn variable_args(&mut self, opts: &Value) -> Value {
        let mut args = opts.clone();
        let variables: Vec<(String, Opt)> = self
            .config
            .variables
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, var) in variables {
            let v = match var {
                Opt::Value(v) => v,
                Opt::Call(Callable::Proc(id)) => value::proc_marker(id),
                Opt::Call(native) => {
                    self.call_or_notify(&native, std::slice::from_ref(opts))
                }
            };
            insert(&mut args, &name, v);
        }
        args
    }

    /// Replaces `${name}` placeholders with variables or `opts` values.
    /// Unknown placeholders are kept.
    pub fn substitute(&mut self, template: &str, opts: &Value) -> String {
        let mut out = String::new();
        let mut rest = template;
        while let Some(start) = rest.find("${") {
            let Some(len) = rest[start + 2..].find('}') else {
                break;
            };
            let name = &rest[start + 2..start + 2 + len];
            out.push_str(&rest[..start]);

            let var = self.config.variables.get(name).cloned();
            let replacement = match var {
                Some(Opt::Value(v)) if !v.is_null() => Some(to_display(&v)),
                Some(Opt::Call(callable)) => {
                    let args = self.variable_args(opts);
                    Some(to_display(&self.call_or_notify(&callable, &[args])))
                }
                _ => opts.get(name).filter(|v| !v.is_null()).map(to_display),
            };
            match replacement {
                Some(r) => out.push_str(&r),
                None => {
                    self.log(log::TRACE, || format!("undefined variable ${{{name}}}, keeping placeholder"));
                    out.push_str(&rest[start..start + 3 + len]);
                }
            }
            rest = &rest[start + 3 + len..];
        }
        out.push_str(rest);
        out
    }

    /// Resolves an option: procedures are called with `opts` and the
    /// variables, strings get `${...}` substitution when variables are enabled
    /// (or `force_substitute`).
    pub fn get_option(
        &mut self,
        opt: &Opt,
        opts: &Value,
        force_substitute: bool,
    ) -> Value {
        match opt {
            Opt::Value(Value::String(s))
                if force_substitute || self.config.variables_enabled =>
            {
                Value::String(self.substitute(s, opts))
            }
            Opt::Value(v) => v.clone(),
            Opt::Call(callable) => {
                let args = self.variable_args(opts);
                self.call_or_notify(callable, &[args])
            }
        }
    }

    fn custom_asset(
        &self,
        filename: &str,
        filetype: &str,
    ) -> Option<(Asset, bool)> {
        let lookup = |key: &str| self.config.assets.get(key).cloned();
        lookup(filetype)
            .or_else(|| lookup(filename))
            .or_else(|| util::lua_extension(filename).and_then(lookup))
            .map(|a| (a, false))
            .or_else(|| lookup("Cord.override").map(|a| (a, true)))
    }

    /// Resolves `type`, `icon`, `name`, `tooltip` and `text` from the
    /// mappings and custom assets.
    fn resolve_asset(&mut self, mut opts: Value) -> Value {
        if get_str(&opts, "filetype").is_empty() {
            if get_str(&opts, "filename").is_empty() {
                insert(&mut opts, "filename", json!("a new file"));
                insert(&mut opts, "filetype", json!("Cord.new"));
            } else {
                insert(&mut opts, "filetype", json!("Cord.unknown"));
            }
        }

        let found = icons::lookup(Lookup {
            filetype: get_str(&opts, "filetype"),
            filename: get_str(&opts, "filename"),
            force_fileext: opts.get("force-fileext").and_then(Value::as_str),
            force_filetype: opts.get("force-filetype").and_then(Value::as_str),
            force_cli: opts.get("force-cli").and_then(Value::as_str),
            match_in_mappings: self.config.match_in_mappings,
        });
        let (ty, icon, tooltip) = match found {
            Some(f) => (f.ty, f.icon, Value::String(f.tooltip)),
            None => (
                "language".to_string(),
                icons::default_icon("language").to_string(),
                Value::Null,
            ),
        };
        insert(&mut opts, "type", json!(ty));
        insert(&mut opts, "icon", json!(self.config.icon(&icon)));
        insert(&mut opts, "tooltip", tooltip.clone());
        insert(&mut opts, "name", tooltip.clone());

        let filename = get_str(&opts, "filename").to_string();
        let filetype = get_str(&opts, "filetype").to_string();
        match self.custom_asset(&filename, &filetype) {
            None => opts,
            Some((Asset::Icon(url), _)) => {
                insert(&mut opts, "icon", json!(url));
                opts
            }
            Some((
                Asset::Custom {
                    name,
                    icon: asset_icon,
                    tooltip: asset_tooltip,
                    text,
                    ty: asset_ty,
                },
                is_override,
            )) => {
                let name = self.get_option(&name, &opts, false);
                let asset_tooltip =
                    self.get_option(&asset_tooltip, &opts, false);
                let text = self.get_option(&text, &opts, false);
                let asset_icon = self.get_option(&asset_icon, &opts, false);
                insert(&mut opts, "name", name);
                insert(
                    &mut opts,
                    "tooltip",
                    if asset_tooltip.is_null() {
                        tooltip
                    } else {
                        asset_tooltip
                    },
                );
                insert(
                    &mut opts,
                    "type",
                    json!(asset_ty.clone().unwrap_or(ty)),
                );
                insert(&mut opts, "text", text);
                if is_override {
                    insert(&mut opts, "filetype", json!("Cord.override"));
                }
                let icon = if !asset_icon.is_null() {
                    asset_icon
                } else if let Some(t) = &asset_ty {
                    json!(self.config.icon(icons::default_icon(t)))
                } else {
                    opts.get("icon").cloned().unwrap_or(Value::Null)
                };
                insert(&mut opts, "icon", icon);
                opts
            }
        }
    }

    fn view_assets(
        &mut self,
        view: View,
        primary_image: Value,
        primary_text: Value,
        opts: &Value,
    ) -> Value {
        let editor_icon = json!(self.config.editor_icon);
        let tooltip = self.config.editor_tooltip.clone();
        let mut assets = Map::new();
        let mut put = |k: &str, v: Value| {
            assets.insert(k.to_string(), v);
        };
        match view {
            View::Editor => {
                put("large-image", editor_icon);
                put("large-text", self.get_option(&tooltip, opts, false));
            }
            View::Asset => {
                put("large-image", primary_image);
                put("large-text", primary_text);
            }
            _ if self.config.swap_icons => {
                put("large-image", editor_icon);
                put("large-text", self.get_option(&tooltip, opts, false));
                put("small-image", primary_image);
                put("small-text", primary_text);
            }
            _ => {
                put("large-image", primary_image);
                put("large-text", primary_text);
                put("small-image", editor_icon);
                put("small-text", self.get_option(&tooltip, opts, false));
            }
        }
        Value::Object(assets)
    }

    /// Builds the activity for `opts`. Returns the enriched `opts` too.
    pub fn build_activity(&mut self, opts: Value) -> (Value, Built) {
        let opts = self.resolve_asset(opts);
        let ty = get_str(&opts, "type").to_string();

        let file_text = match opts.get("text").filter(|t| !t.is_null()) {
            Some(text) => text.clone(),
            None => {
                let key = if ty != "language" {
                    ty.clone()
                } else if opts
                    .get("read-only?")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "viewing".to_string()
                } else {
                    "editing".to_string()
                };
                let opt = self
                    .config
                    .text
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(Opt::none);
                match self.get_option(&opt, &opts, false) {
                    Value::Bool(true) => return (opts, Built::Skip),
                    Value::Bool(false) => return (opts, Built::Clear),
                    v => blank_to_null(v),
                }
            }
        };
        let workspace_opt = self
            .config
            .text
            .get("workspace")
            .cloned()
            .unwrap_or_else(Opt::none);
        let workspace_text = match self.get_option(&workspace_opt, &opts, false)
        {
            Value::Bool(true) => return (opts, Built::Skip),
            Value::Bool(false) => return (opts, Built::Clear),
            v => blank_to_null(v),
        };

        let (mut details, mut state) = if self.config.swap_fields {
            (workspace_text, file_text)
        } else {
            (file_text, workspace_text)
        };

        let rule = self.workspace_rule();
        if let Some(rule) = rule {
            for (field, target) in
                [(&rule.details, &mut details), (&rule.state, &mut state)]
            {
                match field {
                    RuleText::Keep => {}
                    RuleText::Omit => *target = Value::Null,
                    RuleText::Set(opt) => {
                        *target =
                            blank_to_null(self.get_option(opt, &opts, true))
                    }
                }
            }
        }

        let view = match self.config.view {
            View::Auto if get_str(&opts, "filetype") == "Cord.new" => {
                View::Editor
            }
            v => v,
        };
        let image = opts.get("icon").cloned().unwrap_or(Value::Null);
        let text = match opts.get("tooltip").filter(|t| !t.is_null()) {
            Some(t) => t.clone(),
            None => opts.get("filetype").cloned().unwrap_or(Value::Null),
        };
        let assets = self.view_assets(view, image, text, &opts);

        let activity = json!({
            "details": details,
            "state": state,
            "assets": assets,
            "timestamps": { "start": opts.get("timestamp").cloned().unwrap_or(Value::Null) },
            "buttons": opts.get("buttons").cloned().unwrap_or(Value::Null),
        });
        (opts, Built::Activity(activity))
    }

    pub fn build_idle_activity(&mut self, opts: &Value) -> Value {
        let (details_opt, state_opt) = if self.config.swap_fields {
            (
                self.config.idle_state.clone(),
                self.config.idle_details.clone(),
            )
        } else {
            (
                self.config.idle_details.clone(),
                self.config.idle_state.clone(),
            )
        };
        let details = self.get_option(&details_opt, opts, false);
        let state = self.get_option(&state_opt, opts, false);
        let view = match self.config.view {
            View::Auto => View::Full,
            v => v,
        };
        let icon_opt = self.config.idle_icon.clone();
        let tooltip_opt = self.config.idle_tooltip.clone();
        let image = self.get_option(&icon_opt, opts, false);
        let text = self.get_option(&tooltip_opt, opts, false);
        let assets = self.view_assets(view, image, text, opts);
        json!({
            "details": details,
            "state": state,
            "assets": assets,
            "timestamps": { "start": opts.get("timestamp").cloned().unwrap_or(Value::Null) },
            "buttons": opts.get("buttons").cloned().unwrap_or(Value::Null),
            "idle?": self.config.smart_idle,
        })
    }
}
