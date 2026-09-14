//! Activity manager: turns editor events into activity updates, with
//! debouncing and idle detection.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::builder::Built;
use super::codec::to_wire;
use super::config::CursorUpdate;
use super::core::Core;
use super::log;
use super::server;
use super::util;
use super::workspace::{self, Search, Workspace};

/// Time without cursor movement before an `on-hold` update.
const HOLD_DELAY: Duration = Duration::from_millis(1000);

/// State kept across managers (reconnects) and Steel engine reloads.
#[derive(Clone, Debug, Default)]
pub struct Persisted {
    pub timestamp: Option<u64>,
    pub paused: bool,
    /// Paused by a workspace rule with `'visible? #f`.
    pub hidden_by_rule: bool,
    pub idle: bool,
    pub forced: bool,
}

type Slot = fn(&mut Manager) -> &mut Option<u64>;

/// The manager of the ready server connection. Inactive otherwise.
#[derive(Default)]
pub struct Manager {
    pub active: bool,
    pub ready: bool,
    pub focused: bool,
    pub paused: bool,
    pub events_enabled: bool,
    pub opts: Option<Value>,
    pub last_opts: Option<Value>,
    pub skip_update: bool,
    workspaces: HashMap<String, Workspace>,
    current_workspace: Option<Workspace>,
    workspace_loaded: bool,

    pub idle: bool,
    pub forced: bool,
    last_activity: Option<Instant>,
    idle_timer: Option<u64>,

    debounce_timer: Option<u64>,
    debounce_target: Option<Instant>,
    last_update: Option<Instant>,
    pending_force: bool,
    delay_phase: bool,

    hold_timer: Option<u64>,
}

impl Manager {
    pub fn current_workspace(&self) -> Option<Workspace> {
        self.current_workspace.clone()
    }
}

impl Core {
    fn discover_workspace(&self) -> Workspace {
        let cwd = &self.editor.cwd;
        let limit = self.config.limit_to_cwd.then_some(cwd.as_str());
        let search = Search {
            root_markers: &self.config.root_markers,
            limit,
            cwd,
        };
        workspace::discover(&self.editor.path, &search)
    }

    fn buffer_parent(&self) -> String {
        if self.editor.path.is_empty() {
            self.editor.cwd.clone()
        } else {
            util::head(&self.editor.path)
        }
    }

    /// The manager as passed to Scheme hooks.
    pub fn manager_value(&self) -> Value {
        let m = &self.manager;
        json!({ "ready?": m.ready, "paused?": m.paused, "idle?": m.idle, "focused?": m.focused })
    }

    /// (Re)schedules the manager timer stored in `slot`.
    fn schedule(&mut self, slot: Slot, delay: Duration, task: fn(&mut Core)) {
        let old = slot(&mut self.manager).take();
        self.cancel(old);
        let id = self.after(delay, move |core| {
            *slot(&mut core.manager) = None;
            task(core);
        });
        *slot(&mut self.manager) = Some(id);
    }

    fn unschedule(&mut self, slot: Slot) {
        let old = slot(&mut self.manager).take();
        self.cancel(old);
    }

    pub fn manager_new(&mut self) {
        let workspace = self.discover_workspace();
        self.log(log::DEBUG, || {
            format!("manager-new: workspace={}", workspace.dir)
        });
        let p = &self.persisted;
        self.manager = Manager {
            active: true,
            focused: true,
            paused: p.paused,
            events_enabled: !p.paused,
            workspaces: HashMap::from([(
                self.buffer_parent(),
                workspace.clone(),
            )]),
            current_workspace: Some(workspace),
            idle: p.idle,
            forced: p.idle && p.forced,
            ..Manager::default()
        };
    }

    pub fn manager_run(&mut self) {
        self.log(log::DEBUG, || "manager-run".to_string());
        let m = &mut self.manager;
        m.ready = true;
        m.last_activity = Some(Instant::now());
        let (paused, idle) = (m.paused, m.idle);

        let mgr = self.manager_value();
        self.run_hooks("ready", std::slice::from_ref(&mgr));
        self.run_hooks("buf-enter", &[mgr]);
        self.apply_rule_visibility();
        if !paused && !self.manager.paused {
            if idle {
                self.activity_update_idle();
            } else {
                self.manager_queue_update(true);
            }
            self.idle_timer_start();
        }
    }

    pub fn manager_cleanup(&mut self) {
        if !self.manager.active {
            return;
        }
        self.log(log::DEBUG, || "manager-cleanup".to_string());
        self.unschedule(|m| &mut m.debounce_timer);
        self.unschedule(|m| &mut m.idle_timer);
        self.unschedule(|m| &mut m.hold_timer);
        self.manager = Manager::default();
    }

    // Idle

    fn idle_timer_start(&mut self) {
        if self.config.idle_enabled {
            let timeout = self.config.idle_timeout;
            self.log(log::TRACE, || {
                format!("IdleTimer: starting; timeout={timeout}")
            });
            self.schedule(
                |m| &mut m.idle_timer,
                Duration::ZERO,
                Core::idle_check,
            );
        }
    }

    fn idle_check(&mut self) {
        let timeout = Duration::from_millis(self.config.idle_timeout);
        let m = &self.manager;
        if m.idle || (!self.config.idle_enabled && !m.forced) {
            self.schedule(|m| &mut m.idle_timer, timeout, Core::idle_check);
            return;
        }
        let elapsed = m.last_activity.map(|t| t.elapsed()).unwrap_or_default();
        let should_idle = m.forced
            || (elapsed >= timeout && (self.config.ignore_focus || !m.focused));
        self.log(log::TRACE, || {
            format!(
                "IdleTimer.check: elapsed={}, should_idle={should_idle}",
                elapsed.as_millis()
            )
        });
        if should_idle {
            self.log(log::DEBUG, || "IdleTimer: entering idle".to_string());
            self.set_idle(true, self.manager.forced);
            self.activity_update_idle();
            self.schedule(|m| &mut m.idle_timer, timeout, Core::idle_check);
        } else {
            self.schedule(
                |m| &mut m.idle_timer,
                timeout
                    .checked_sub(elapsed)
                    .filter(|d| !d.is_zero())
                    .unwrap_or(timeout),
                Core::idle_check,
            );
        }
    }

    fn set_idle(&mut self, idle: bool, forced: bool) {
        self.manager.idle = idle;
        self.manager.forced = forced;
        self.persisted.idle = idle;
        self.persisted.forced = forced;
    }

    pub fn manager_record_activity(&mut self) {
        self.manager.last_activity = Some(Instant::now());
    }

    // Debouncing

    pub fn manager_queue_update(&mut self, force: bool) {
        if !(self.manager.events_enabled && self.manager.ready) {
            return;
        }
        let delay = Duration::from_millis(self.config.debounce_delay);
        let interval = Duration::from_millis(self.config.debounce_interval);
        if delay.is_zero() && interval.is_zero() {
            self.do_update(force);
            return;
        }

        let now = Instant::now();
        let m = &mut self.manager;
        m.pending_force |= force;
        let throttled = m.last_update.filter(|l| {
            !interval.is_zero() && now.duration_since(*l) < interval
        });

        if m.debounce_timer.is_some() {
            let near = m
                .debounce_target
                .is_some_and(|t| t.saturating_duration_since(now) < delay);
            if !delay.is_zero() && near && !m.delay_phase {
                m.debounce_target = Some(now + delay);
                m.delay_phase = true;
                self.log(log::TRACE, || {
                    "Debouncer: near-fire burst, rescheduling".to_string()
                });
                self.schedule(
                    |m| &mut m.debounce_timer,
                    delay,
                    Core::debounce_execute,
                );
            } else {
                self.log(log::TRACE, || {
                    "Debouncer: timer running, coalescing".to_string()
                });
            }
        } else if let Some(last) = throttled {
            let remaining = interval - now.duration_since(last);
            m.debounce_target = Some(now + remaining);
            m.delay_phase = false;
            self.log(log::TRACE, || {
                format!(
                    "Debouncer: throttling, scheduling in {}ms",
                    remaining.as_millis()
                )
            });
            self.schedule(
                |m| &mut m.debounce_timer,
                remaining,
                Core::debounce_execute,
            );
        } else if !delay.is_zero() {
            m.debounce_target = Some(now + delay);
            m.delay_phase = true;
            self.log(log::TRACE, || {
                format!("Debouncer: delaying for {}ms", delay.as_millis())
            });
            self.schedule(
                |m| &mut m.debounce_timer,
                delay,
                Core::debounce_execute,
            );
        } else {
            self.debounce_execute();
        }
    }

    fn debounce_execute(&mut self) {
        let m = &mut self.manager;
        m.last_update = Some(Instant::now());
        m.delay_phase = false;
        let force = std::mem::take(&mut m.pending_force);
        self.do_update(force);
    }

    // Options

    fn build_buttons(&mut self, opts: &Value) -> Value {
        let buttons = self.config.buttons.clone();
        let args = std::slice::from_ref(opts);
        Value::Array(
            buttons
                .iter()
                .filter_map(|b| {
                    let label = self.resolve(&b.label, args);
                    let url = self.resolve(&b.url, args);
                    (!label.is_null() && !url.is_null())
                        .then(|| json!({ "label": label, "url": url }))
                })
                .collect(),
        )
    }

    /// `full` false reuses the timestamp and buttons of the previous options.
    fn options_build(&mut self, full: bool) -> Value {
        let e = &self.editor;
        let m = &self.manager;
        let ws = m
            .current_workspace
            .clone()
            .unwrap_or_else(|| workspace::describe(&e.cwd));
        let file =
            |v: String| if e.path.is_empty() { String::new() } else { v };
        let mut opts = json!({
            "filename": file(util::tail(&e.path)),
            "filetype": file(e.language.clone()),
            "read-only?": false,
            "cursor-line": e.line,
            "cursor-column": e.column,
            // 0-based, like cord.nvim
            "cursor-char": e.column.saturating_sub(1),
            // Helix has no buffer types
            "buftype": "",
            "workspace-dir": ws.dir,
            "workspace": ws.name,
            "worktree": ws.worktree,
            "repo-url": ws.repo_url,
            "focused?": m.focused,
            "idle?": m.idle,
        });

        if !full {
            let previous = m.opts.as_ref().or(m.last_opts.as_ref());
            for key in ["timestamp", "buttons"] {
                opts[key] = previous
                    .and_then(|p| p.get(key))
                    .cloned()
                    .unwrap_or_default();
            }
            return opts;
        }

        let last_ts = m
            .last_opts
            .as_ref()
            .and_then(|o| o.get("timestamp"))
            .filter(|t| !t.is_null())
            .cloned();
        let c = &self.config;
        opts["timestamp"] = if !c.timestamp_enabled {
            Value::Null
        } else if c.reset_on_change || (c.reset_on_idle && m.idle) {
            json!(util::unix_time())
        } else if c.shared_timestamp {
            Value::Null
        } else if let Some(ts) = last_ts {
            ts
        } else {
            json!(self.persisted.timestamp.unwrap_or_else(util::unix_time))
        };
        opts["buttons"] = self.build_buttons(&opts);
        opts
    }

    fn options_changed(curr: &Value, prev: Option<&Value>) -> bool {
        let Some(prev) = prev else { return true };
        [
            "filename",
            "filetype",
            "read-only?",
            "cursor-line",
            "cursor-column",
            "focused?",
        ]
        .iter()
        .any(|k| curr.get(*k) != prev.get(*k))
    }

    // Updates

    fn do_update(&mut self, force: bool) {
        if !(self.manager.events_enabled && self.manager.ready) {
            return;
        }
        let opts = self.options_build(true);
        self.manager.opts = Some(opts.clone());
        if !self.manager.workspace_loaded {
            self.manager.workspace_loaded = true;
            self.run_hooks("workspace-change", std::slice::from_ref(&opts));
            if self.manager.paused {
                return;
            }
        }
        let m = &self.manager;
        if m.forced
            || !(force || Self::options_changed(&opts, m.last_opts.as_ref()))
        {
            return;
        }
        self.log(log::DEBUG, || format!("do-update: updating; force={force}"));
        if self.manager.idle {
            self.run_hooks("idle-leave", std::slice::from_ref(&opts));
        }
        self.activity_update();
    }

    fn activity_update(&mut self) {
        self.log(log::DEBUG, || "activity-update".to_string());
        self.set_idle(false, false);
        self.manager_record_activity();
        let m = &mut self.manager;
        m.last_opts = m.opts.clone();
        let mut opts = m.opts.clone().unwrap_or_else(|| json!({}));
        opts["idle?"] = json!(false);

        let opts = self.fold_hooks("pre-activity", &[], opts);
        let (opts, built) = self.build_activity(opts);
        self.manager.opts = Some(opts.clone());
        match built {
            Built::Skip => {}
            Built::Clear => self.manager_clear_activity(false),
            Built::Activity(activity) => {
                self.manager_set_activity(activity, false);
                if let Some(ts) = opts["timestamp"].as_u64() {
                    self.persisted.timestamp = Some(ts);
                }
            }
        }
    }

    fn activity_update_idle(&mut self) {
        self.log(log::DEBUG, || "activity-update-idle".to_string());
        let mut opts = match self.manager.opts.clone() {
            Some(opts) => opts,
            None => self.options_build(true),
        };
        self.manager_record_activity();
        opts["idle?"] = json!(true);

        if self.config.idle_show_status {
            opts["buttons"] = self.build_buttons(&opts);
            if self.config.timestamp_enabled && self.config.reset_on_idle {
                opts["timestamp"] = json!(util::unix_time());
            }
            self.manager.opts = Some(opts.clone());
            let activity = self.build_idle_activity(&opts);
            self.manager_set_activity(activity, false);
        } else {
            self.manager.opts = Some(opts.clone());
            self.manager_clear_activity(false);
        }
        self.run_hooks("idle-enter", &[opts]);
    }

    /// Runs `post-activity` hooks on `activity` and sends it to the server.
    pub fn manager_set_activity(&mut self, activity: Value, force: bool) {
        if !self.manager.ready || self.manager.paused {
            return;
        }
        let opts = self.manager.opts.clone().unwrap_or_default();
        let activity = self.fold_hooks("post-activity", &[opts], activity);
        if !force && std::mem::take(&mut self.manager.skip_update) {
            return;
        }
        server::send(
            self,
            "update_activity",
            json!({ "activity": to_wire(activity), "force": force }),
        );
    }

    pub fn manager_clear_activity(&mut self, force: bool) {
        server::send(self, "clear_activity", json!(force));
    }

    pub fn manager_skip_update(&mut self) {
        self.manager.skip_update = true;
    }

    // Editor events

    pub fn manager_on_buf_enter(&mut self) {
        if !self.manager.active {
            return;
        }
        self.log(log::TRACE, || "manager-on-buf-enter".to_string());
        let mgr = self.manager_value();
        self.run_hooks("buf-enter", &[mgr]);

        let parent = self.buffer_parent();
        let info = match self.manager.workspaces.get(&parent) {
            Some(ws) => ws.clone(),
            None => {
                let ws = self.discover_workspace();
                self.manager.workspaces.insert(parent, ws.clone());
                ws
            }
        };
        let current = self.manager.current_workspace.replace(info.clone());
        if current.is_none_or(|c| c.dir != info.dir) {
            let opts = self.options_build(false);
            self.run_hooks("workspace-change", &[opts]);
        }
        self.apply_rule_visibility();
        self.manager_queue_update(false);
    }

    /// Hides the presence while a workspace rule with `'visible? #f` matches,
    /// and shows it again afterwards. Presences hidden otherwise stay hidden.
    pub fn apply_rule_visibility(&mut self) {
        let hidden = self.workspace_rule().is_some_and(|rule| !rule.visible);
        match (hidden, self.persisted.hidden_by_rule) {
            (true, false) if !self.manager.paused => {
                self.log(log::DEBUG, || {
                    "Workspace rule: hiding the presence".to_string()
                });
                self.manager_suppress();
                self.persisted.hidden_by_rule = true;
            }
            (false, true) => {
                self.log(log::DEBUG, || {
                    "Workspace rule: showing the presence".to_string()
                });
                self.persisted.hidden_by_rule = false;
                self.manager_resume();
            }
            _ => {}
        }
    }

    fn on_focus(&mut self, focused: bool) {
        if !self.manager.events_enabled {
            return;
        }
        self.manager.focused = focused;
        let mgr = self.manager_value();
        if focused {
            self.run_hooks("focus-gained", &[mgr]);
            if self.config.unidle_on_focus {
                self.manager_queue_update(true);
            }
        } else {
            self.run_hooks("focus-lost", &[mgr]);
        }
    }

    pub fn manager_on_focus_gained(&mut self) {
        self.on_focus(true);
    }

    pub fn manager_on_focus_lost(&mut self) {
        self.on_focus(false);
    }

    pub fn manager_on_cursor_moved(&mut self) {
        if !self.config.autocmds {
            return;
        }
        match self.config.cursor_update {
            CursorUpdate::OnMove => self.manager_on_cursor_update(),
            CursorUpdate::OnHold => self.schedule(
                |m| &mut m.hold_timer,
                HOLD_DELAY,
                Core::manager_on_cursor_update,
            ),
            CursorUpdate::None => {}
        }
    }

    fn manager_on_cursor_update(&mut self) {
        if self.manager.events_enabled {
            self.manager_queue_update(false);
        }
    }

    // Commands

    pub fn manager_idle(&mut self) {
        if self.manager.active {
            self.activity_update_idle();
        }
    }

    pub fn manager_force_idle(&mut self) {
        if self.manager.active {
            self.set_idle(true, true);
            self.activity_update_idle();
            self.log(log::TRACE, || "IdleTimer: forced".to_string());
        }
    }

    pub fn manager_unidle(&mut self) {
        if self.manager.active {
            self.set_idle(false, false);
            self.manager_queue_update(true);
        }
    }

    pub fn manager_toggle_idle(&mut self, force: bool) {
        let m = &self.manager;
        if !m.active {
            return;
        }
        if m.forced || m.idle {
            self.manager_unidle();
        } else if force {
            self.manager_force_idle();
        } else {
            self.manager_idle();
        }
    }

    pub fn manager_pause(&mut self) {
        if !self.manager.active || self.manager.paused {
            return;
        }
        self.manager.paused = true;
        self.manager.events_enabled = false;
        self.persisted.paused = true;
        self.unschedule(|m| &mut m.idle_timer);
        self.log(log::DEBUG, || "manager-pause".to_string());
    }

    pub fn manager_resume(&mut self) {
        if !self.manager.active || !self.manager.paused {
            return;
        }
        self.manager.paused = false;
        self.manager.events_enabled = true;
        self.persisted.paused = false;
        self.persisted.hidden_by_rule = false;
        self.manager_queue_update(true);
        self.set_idle(false, false);
        self.idle_timer_start();
        self.log(log::DEBUG, || "manager-resume".to_string());
    }

    pub fn manager_hide(&mut self) {
        if self.manager.active {
            self.manager_pause();
            self.manager_clear_activity(true);
        }
    }

    pub fn manager_suppress(&mut self) {
        if self.manager.active {
            self.manager_pause();
            self.manager_clear_activity(false);
        }
    }

    pub fn manager_toggle(&mut self) {
        match (self.manager.active, self.manager.paused) {
            (true, true) => self.manager_resume(),
            (true, false) => self.manager_hide(),
            _ => {}
        }
    }

    pub fn manager_toggle_suppress(&mut self) {
        match (self.manager.active, self.manager.paused) {
            (true, true) => self.manager_resume(),
            (true, false) => self.manager_suppress(),
            _ => {}
        }
    }

    /// Manager methods callable from Scheme (`cord/api.scm`).
    pub fn manager_api(&mut self, op: &str, args: &[Value]) {
        let flag =
            |i: usize| args.get(i).and_then(Value::as_bool).unwrap_or(false);
        match op {
            "queue-update" => self.manager_queue_update(flag(0)),
            "skip-update" => self.manager_skip_update(),
            "set-activity" => {
                if let Some(activity) = args.first().filter(|a| a.is_object()) {
                    self.manager_set_activity(activity.clone(), flag(1));
                }
            }
            "clear-activity" => self.manager_clear_activity(flag(0)),
            "pause" => self.manager_pause(),
            "resume" => self.manager_resume(),
            "pause-events" => self.manager.events_enabled = false,
            "resume-events" => {
                self.manager.events_enabled = true;
                self.manager_queue_update(true);
            }
            "hide" => self.manager_hide(),
            "suppress" => self.manager_suppress(),
            "toggle" => self.manager_toggle(),
            "toggle-suppress" => self.manager_toggle_suppress(),
            "idle" => self.manager_idle(),
            "force-idle" => self.manager_force_idle(),
            "unidle" => self.manager_unidle(),
            "toggle-idle" => self.manager_toggle_idle(flag(0)),
            other => self.log_notify(
                &format!("Unknown manager method: {other}"),
                log::ERROR,
            ),
        }
    }
}
