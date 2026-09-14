//! The plugin core.
//!
//! All state lives on one thread that outlives Steel engine reloads. Scheme
//! sends it messages (editor events, commands, configuration) and receives
//! actions it has to perform in the editor: showing messages, opening
//! buffers and calling Scheme procedures from the configuration. A procedure
//! call blocks the core thread until Scheme replies, which never blocks the
//! editor thread, since Scheme only ever sends messages to the core.

use std::collections::{BTreeMap, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use super::config::{Callable, Config, Hook, Opt};
use super::extensions::Registry;
use super::log::{self, Logger, Sink};
use super::manager::{Manager, Persisted};
use super::server::{self, Server};
use super::value::{self, ERROR_KEY};

/// Editor state sent along with every event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Editor {
    /// Path of the focused document; empty for scratch buffers.
    pub path: String,
    /// Helix language name.
    pub language: String,
    /// 1-based line of the primary cursor.
    pub line: u64,
    /// 1-based column of the primary cursor.
    pub column: u64,
    pub cwd: String,
    /// `[hint info warning error]` counts of the focused document.
    pub diagnostics: Option<[u64; 4]>,
    /// The same counts summed over all documents.
    pub workspace_diagnostics: Option<[u64; 4]>,
}

fn counts(v: Option<&Value>) -> Option<[u64; 4]> {
    let a = v?.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0);
    Some([n(0), n(1), n(2), n(3)])
}

impl Editor {
    pub fn from_value(v: &Value) -> Editor {
        let s = |k: &str| value::str_of(v, k).unwrap_or_default().to_string();
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(1);
        Editor {
            path: s("path"),
            language: s("language"),
            line: n("line"),
            column: n("column"),
            cwd: s("cwd"),
            diagnostics: counts(v.get("diagnostics")),
            workspace_diagnostics: counts(v.get("workspace-diagnostics")),
        }
    }
}

pub enum Msg {
    /// A (re)loaded Steel engine connects to the core.
    Attach {
        session: u64,
        actions: UnboundedSender<Value>,
    },
    /// Starts the plugin; `config` is the user configuration or null.
    Start {
        config: Value,
        editor: Editor,
    },
    Configure {
        config: Value,
    },
    Event {
        kind: String,
        editor: Editor,
    },
    Command {
        args: Vec<String>,
        editor: Editor,
    },
    Manager {
        op: String,
        args: Vec<Value>,
    },
    Run(Box<dyn FnOnce(&mut Core) + Send>),
}

/// Session of the Steel engine currently attached.
pub static SESSION: AtomicU64 = AtomicU64::new(0);

/// Messages that may arrive while the core waits for a procedure call.
enum Inbound {
    Reply {
        session: u64,
        id: u64,
        value: Value,
    },
    /// A manager method called by the procedure itself.
    Manager {
        op: String,
        args: Vec<Value>,
    },
}

/// Number of procedure calls the core is waiting for.
static CALLS: AtomicUsize = AtomicUsize::new(0);

/// Channels to the core thread: messages, and replies to procedure calls.
static CORE: OnceLock<(Sender<Msg>, Sender<Inbound>)> = OnceLock::new();

/// Sends a message to the core, starting it if needed.
pub fn send(msg: Msg) {
    let (tx, _) = CORE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let core_tx = tx.clone();
        std::thread::Builder::new()
            .name("cord-core".to_string())
            .spawn(move || Core::new(core_tx, reply_rx).run(rx))
            .expect("failed to start the cord core thread");
        (tx, reply_tx)
    });
    let _ = tx.send(msg);
}

/// Delivers the result of a procedure call.
pub fn reply(session: u64, id: u64, value: Value) {
    if let Some((_, tx)) = CORE.get() {
        let _ = tx.send(Inbound::Reply { session, id, value });
    }
}

/// Calls a manager method. Called from a procedure, it runs as soon as the
/// procedure returned, before the core continues (e.g. `skip-update` in
/// `post-activity` skips the activity being built, like in cord.nvim).
pub fn manager(op: String, args: Vec<Value>) {
    match CORE.get() {
        Some((_, tx)) if CALLS.load(Ordering::SeqCst) > 0 => {
            let _ = tx.send(Inbound::Manager { op, args });
        }
        _ => send(Msg::Manager { op, args }),
    }
}

pub type Task = Box<dyn FnOnce(&mut Core) + Send>;

#[derive(Default)]
struct Timers {
    next: u64,
    queue: BTreeMap<(Instant, u64), Task>,
    deadlines: HashMap<u64, Instant>,
}

/// How long a procedure call may take before it is abandoned.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Core {
    tx: Sender<Msg>,
    replies: Receiver<Inbound>,
    timers: Timers,

    session: u64,
    actions: Option<UnboundedSender<Value>>,
    next_call: u64,
    /// Session whose procedures the configuration refers to.
    pub config_session: u64,

    pub logger: Logger,
    pub started: bool,
    /// The user configuration given to `cord-configure!`.
    pub user_config: Value,
    pub config: Config,
    pub hooks: Vec<Hook>,
    pub extensions: Registry,
    pub editor: Editor,
    pub server: Server,
    pub manager: Manager,
    pub persisted: Persisted,
    /// Index of the workspace rule matching a workspace directory and buffer.
    pub rule_cache: HashMap<(String, String), Option<usize>>,
}

impl Core {
    fn new(tx: Sender<Msg>, replies: Receiver<Inbound>) -> Core {
        Core {
            tx,
            replies,
            timers: Timers::default(),
            session: 0,
            actions: None,
            next_call: 0,
            config_session: 0,
            logger: Logger::from_env(),
            started: false,
            user_config: Value::Null,
            config: Config::default(),
            hooks: Vec::new(),
            extensions: Registry::default(),
            editor: Editor::default(),
            server: Server::default(),
            manager: Manager::default(),
            persisted: Persisted::default(),
            rule_cache: HashMap::new(),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.timers.queue.keys().next() {
                Some((at, _)) => match rx
                    .recv_timeout(at.saturating_duration_since(Instant::now()))
                {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                },
            };

            if let Some(msg) = msg {
                self.guard(|core| core.handle(msg));
            }

            loop {
                let now = Instant::now();
                let Some(key) = self
                    .timers
                    .queue
                    .keys()
                    .next()
                    .copied()
                    .filter(|(at, _)| *at <= now)
                else {
                    break;
                };
                let task = self.timers.queue.remove(&key).unwrap();
                self.timers.deadlines.remove(&key.1);
                self.guard(task);
            }
        }
    }

    /// Runs `f`, logging panics instead of taking the core thread down.
    fn guard(&mut self, f: impl FnOnce(&mut Core)) {
        if let Err(panic) =
            std::panic::catch_unwind(AssertUnwindSafe(|| f(self)))
        {
            let msg = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            self.log_notify(&format!("internal error: {msg}"), log::ERROR);
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Attach { session, actions } => {
                self.log(log::DEBUG, || format!("Attached session {session}"));
                self.session = session;
                self.actions = Some(actions);
            }
            Msg::Start { config, editor } => {
                self.editor = editor;
                self.user_config = config;
                self.config_session = self.session;
                self.start();
            }
            Msg::Configure { config } => {
                self.user_config = config;
                self.config_session = self.session;
                if self.started {
                    self.start();
                }
            }
            Msg::Event { kind, editor } => {
                self.editor = editor;
                self.on_editor_event(&kind);
            }
            Msg::Command { args, editor } => {
                self.editor = editor;
                super::commands::handle(self, &args);
            }
            Msg::Manager { op, args } => self.manager_api(&op, &args),
            Msg::Run(task) => task(self),
        }
    }

    // Scheduling

    /// Runs `task` on the core thread after `delay`. Returns a timer id.
    pub fn after(
        &mut self,
        delay: Duration,
        task: impl FnOnce(&mut Core) + Send + 'static,
    ) -> u64 {
        self.timers.next += 1;
        let id = self.timers.next;
        let at = Instant::now() + delay;
        self.timers.queue.insert((at, id), Box::new(task));
        self.timers.deadlines.insert(id, at);
        id
    }

    pub fn cancel(&mut self, id: Option<u64>) {
        if let Some(id) = id
            && let Some(at) = self.timers.deadlines.remove(&id)
        {
            self.timers.queue.remove(&(at, id));
        }
    }

    /// Runs `work` on a separate thread and `then` with its result on the core
    /// thread.
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut Core, T) + Send + 'static,
    ) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = work();
            let _ = tx.send(Msg::Run(Box::new(move |core| then(core, result))));
        });
    }

    /// Returns a function posting tasks to the core thread from other threads.
    pub fn sender(&self) -> impl Fn(Task) + Send + Clone + 'static {
        let tx = self.tx.clone();
        move |task| {
            let _ = tx.send(Msg::Run(task));
        }
    }

    // Scheme bridge

    /// Sends an action to Scheme.
    pub fn action(&mut self, action: Value) {
        if let Some(tx) = &self.actions
            && tx.unbounded_send(action).is_err()
        {
            self.actions = None;
        }
    }

    /// Calls a procedure. Errors are returned as messages.
    pub fn call(
        &mut self,
        callable: &Callable,
        args: &[Value],
    ) -> Result<Value, String> {
        match callable {
            Callable::Native(f) => Ok(f(self, args)),
            Callable::Proc(id) => self.call_proc(*id, args),
        }
    }

    fn call_proc(
        &mut self,
        proc: u64,
        args: &[Value],
    ) -> Result<Value, String> {
        let session = self.config_session;
        if session != SESSION.load(Ordering::SeqCst) || self.actions.is_none() {
            return Err("procedure of a previous Steel engine".to_string());
        }
        self.next_call += 1;
        let id = self.next_call;
        CALLS.fetch_add(1, Ordering::SeqCst);
        self.action(json!({ "type": "call", "session": session, "id": id, "proc": proc, "args": args }));
        let mut methods = Vec::new();
        let result = self.await_reply(session, id, &mut methods);
        CALLS.fetch_sub(1, Ordering::SeqCst);
        // Run manager methods called by the procedure once it returned: they
        // may call procedures themselves, which Scheme can only answer now
        for (op, args) in methods {
            self.manager_api(&op, &args);
        }
        result
    }

    fn await_reply(
        &mut self,
        session: u64,
        id: u64,
        methods: &mut Vec<(String, Vec<Value>)>,
    ) -> Result<Value, String> {
        let started = Instant::now();
        loop {
            match self.replies.recv_timeout(Duration::from_millis(50)) {
                Ok(Inbound::Reply {
                    session: s,
                    id: i,
                    value,
                }) if s == session && i == id => {
                    return match value.get(ERROR_KEY) {
                        Some(err) => Err(value::to_display(err)),
                        None => Ok(value),
                    };
                }
                Ok(Inbound::Reply { .. }) => {}
                Ok(Inbound::Manager { op, args }) => methods.push((op, args)),
                Err(RecvTimeoutError::Timeout) => {
                    if SESSION.load(Ordering::SeqCst) != session {
                        return Err("the Steel engine was reloaded".to_string());
                    }
                    if started.elapsed() > CALL_TIMEOUT {
                        return Err("procedure call timed out".to_string());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("no reply channel".to_string());
                }
            }
        }
    }

    /// Like `call`, but notifies errors and returns null.
    pub fn call_or_notify(
        &mut self,
        callable: &Callable,
        args: &[Value],
    ) -> Value {
        match self.call(callable, args) {
            Ok(v) => v,
            Err(e) => {
                self.log_notify(&e, log::ERROR);
                Value::Null
            }
        }
    }

    /// Resolves an option: procedures are called with `args`.
    pub fn resolve(&mut self, opt: &Opt, args: &[Value]) -> Value {
        match opt {
            Opt::Value(v) => v.clone(),
            Opt::Call(c) => self.call_or_notify(c, args),
        }
    }

    // Hooks

    fn hooks_for(&self, event: &str) -> Vec<Callable> {
        self.hooks
            .iter()
            .filter(|h| h.event == event)
            .map(|h| h.callback.clone())
            .collect()
    }

    /// Runs the hooks of `event` for their side effects.
    pub fn run_hooks(&mut self, event: &str, args: &[Value]) {
        self.log(log::TRACE, || format!("hooks-run: event={event}"));
        for hook in self.hooks_for(event) {
            self.call_or_notify(&hook, args);
        }
    }

    /// Runs the hooks of `event`, threading `value` through them: a hook
    /// returning a hash map replaces it for the following hooks.
    pub fn fold_hooks(
        &mut self,
        event: &str,
        args: &[Value],
        value: Value,
    ) -> Value {
        self.log(log::TRACE, || format!("hooks-fold: event={event}"));
        let mut value = value;
        for hook in self.hooks_for(event) {
            let mut call_args = args.to_vec();
            call_args.push(value.clone());
            if let Value::Object(result) =
                self.call_or_notify(&hook, &call_args)
                && !result.contains_key(value::PROC_KEY)
            {
                value = Value::Object(result);
            }
        }
        value
    }

    /// Registers the configured hooks, sorted by priority. Hooks of the same
    /// priority keep their registration order.
    pub fn set_hooks(&mut self, mut hooks: Vec<Hook>) {
        hooks
            .retain(|h| super::config::HOOK_EVENTS.contains(&h.event.as_str()));
        hooks.sort_by_key(|h| std::cmp::Reverse(h.priority));
        self.hooks = hooks;
    }

    // Logging

    pub fn log(&mut self, level: u8, message: impl FnOnce() -> String) {
        if self.logger.enabled(level) {
            self.emit(level, &message());
        }
    }

    /// Shows a message regardless of the log level.
    pub fn log_notify(&mut self, message: &str, level: u8) {
        self.emit(level, message);
    }

    fn emit(&mut self, level: u8, message: &str) {
        match &self.logger.sink {
            Sink::File(path) => {
                let path = path.clone();
                if let Err(e) =
                    Logger::write_file(&path, &[Logger::format(level, message)])
                {
                    self.notify(
                        &format!("Failed to write log file: {e}"),
                        log::ERROR,
                    );
                }
            }
            Sink::Editor => self.notify(message, level),
        }
    }

    /// Shows a message in the editor.
    pub fn notify(&mut self, message: &str, level: u8) {
        self.action(json!({ "type": "notify", "message": format!("[cord.hx] {message}"), "level": level }));
    }

    /// Logs messages sent by the server.
    pub fn log_server(&mut self, logs: &[Value]) {
        let entries: Vec<(u8, String)> = logs
            .iter()
            .filter_map(|item| {
                let level = item.get("level")?.as_u64()? as u8;
                let message = item.get("message")?.as_str()?.to_string();
                self.logger.enabled(level).then_some((level, message))
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        match &self.logger.sink {
            Sink::File(path) => {
                let lines: Vec<String> = entries
                    .iter()
                    .map(|(level, m)| {
                        Logger::format(*level, &format!("[SERVER] {m}"))
                    })
                    .collect();
                let _ = Logger::write_file(&path.clone(), &lines);
            }
            Sink::Editor => {
                let max = entries.iter().map(|(l, _)| *l).max().unwrap_or(0);
                let joined: Vec<&str> =
                    entries.iter().map(|(_, m)| m.as_str()).collect();
                self.notify(&joined.join("\n[cord.hx] "), max);
            }
        }
    }

    // Lifecycle

    /// Applies the configuration and starts or restarts the plugin.
    fn start(&mut self) {
        let first = !self.started;
        self.started = true;

        let (config, hooks, registry) = match super::extensions::build(self) {
            Ok(built) => built,
            Err(e) => {
                self.log_notify(&e, log::ERROR);
                return;
            }
        };
        let previous = std::mem::replace(&mut self.config, config);
        self.logger.set_level(self.config.log_level);
        *super::icons::THEME.write().unwrap() =
            (self.config.theme.clone(), self.config.flavor.clone());
        for warning in self.config.warnings() {
            self.log_notify(&warning, log::WARN);
        }

        self.manager_cleanup();
        self.rule_cache.clear();
        let old = std::mem::replace(&mut self.extensions, registry);
        old.shutdown(self);
        self.set_hooks(hooks);

        if !self.config.enabled {
            server::cleanup(self);
            return;
        }

        if !first
            && self.server.is_connected()
            && server::same_connection(&previous, &self.config)
        {
            // Keep the connection, so the presence does not blink
            self.log(log::DEBUG, || {
                "Configuration applied to the running connection".to_string()
            });
            server::send_initialize(self);
            if self.server.is_ready() {
                server::create_manager(self);
            }
        } else if first || !self.server.is_connected() {
            server::initialize(self);
        } else {
            server::restart_connection(self);
        }
    }

    fn on_editor_event(&mut self, kind: &str) {
        match kind {
            "buf-enter" => self.manager_on_buf_enter(),
            "cursor" => self.manager_on_cursor_moved(),
            "focus-gained" => self.manager_on_focus_gained(),
            "focus-lost" => self.manager_on_focus_lost(),
            _ => {}
        }
    }
}
