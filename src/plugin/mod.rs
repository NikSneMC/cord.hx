//! The Steel module of the plugin. Scheme stays a thin proxy because Helix
//! compiles it on startup and on every engine reload.

mod builder;
mod client;
mod codec;
mod commands;
mod config;
mod core;
mod extensions;
mod icons;
mod log;
mod manager;
mod report;
mod run;
mod server;
mod util;
mod validate;
mod value;
mod workspace;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use abi_stable::std_types::RResult;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use serde_json::Value;
use steel::steel_vm::ffi::{
    FFIArg, FFIModule, FFIValue, FfiFutureExt, RegisterFFIFn,
};

use self::core::{Editor, Msg, SESSION};

type ActionReceiver = Arc<futures::lock::Mutex<UnboundedReceiver<Value>>>;

/// Actions for the attached Steel engine.
static ACTIONS: Mutex<Option<ActionReceiver>> = Mutex::new(None);

fn guarded<T: Default>(f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_default()
}

/// Attaches a (re)loaded Steel engine. Returns its session id.
fn attach() -> isize {
    guarded(|| {
        let session = SESSION.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = unbounded();
        // Replacing the receiver ends the pending `next-action` of the
        // previous engine
        *ACTIONS.lock().unwrap() =
            Some(Arc::new(futures::lock::Mutex::new(rx)));
        core::send(Msg::Attach {
            session,
            actions: tx,
        });
        session as isize
    })
}

/// A future resolving to the next action, or `#f` once the engine is detached.
fn next_action() -> FFIValue {
    let rx = ACTIONS.lock().unwrap().clone();
    let fut = async move {
        let next = match rx {
            Some(rx) => rx.lock().await.next().await,
            None => None,
        };
        RResult::ROk(match next {
            Some(action) => value::to_ffi(&action),
            None => FFIValue::BoolV(false),
        })
    };
    FFIValue::from(fut.into_ffi())
}

fn reply(session: isize, id: isize, result: FFIArg<'static>) {
    guarded(|| core::reply(session as u64, id as u64, value::from_ffi(result)));
}

fn start(config: FFIArg<'static>, editor: FFIArg<'static>) {
    guarded(|| {
        core::send(Msg::Start {
            config: value::from_ffi(config),
            editor: Editor::from_value(&value::from_ffi(editor)),
        })
    });
}

fn configure(config: FFIArg<'static>) {
    guarded(|| {
        core::send(Msg::Configure {
            config: value::from_ffi(config),
        })
    });
}

fn event(kind: String, editor: FFIArg<'static>) {
    guarded(|| {
        core::send(Msg::Event {
            kind,
            editor: Editor::from_value(&value::from_ffi(editor)),
        })
    });
}

fn command(args: FFIArg<'static>, editor: FFIArg<'static>) {
    guarded(|| {
        let args = match value::from_ffi(args) {
            Value::Array(items) => {
                items.iter().map(value::to_display).collect()
            }
            Value::String(s) => {
                s.split_whitespace().map(str::to_string).collect()
            }
            _ => Vec::new(),
        };
        core::send(Msg::Command {
            args,
            editor: Editor::from_value(&value::from_ffi(editor)),
        });
    });
}

/// Manager method called from Scheme (see `cord/api.scm`).
fn manager(op: String, args: FFIArg<'static>) {
    guarded(|| {
        let args = match value::from_ffi(args) {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        core::manager(op, args);
    });
}

/// URL of a built-in icon; theme and flavor default to the configured ones.
fn icon(
    name: String,
    theme: FFIArg<'static>,
    flavor: FFIArg<'static>,
) -> String {
    guarded(|| {
        let (default_theme, default_flavor) =
            icons::THEME.read().unwrap().clone();
        let or = |v: Value, default: String| match v {
            Value::String(s) => s,
            _ if default.is_empty() => "default".to_string(),
            _ => default,
        };
        icons::icon_url(
            &name,
            &or(value::from_ffi(theme), default_theme),
            &or(value::from_ffi(flavor), default_flavor),
        )
    })
}

fn run_command(
    command: FFIArg<'static>,
    cwd: FFIArg<'static>,
    ttl: FFIArg<'static>,
) -> Option<String> {
    guarded(|| {
        let command = match value::from_ffi(command) {
            Value::Array(items) => {
                items.iter().map(value::to_display).collect()
            }
            _ => return None,
        };
        let cwd = value::from_ffi(cwd)
            .as_str()
            .unwrap_or_default()
            .to_string();
        let ttl = value::from_ffi(ttl).as_f64().unwrap_or(30.0).max(0.0);
        run::run(command, cwd, std::time::Duration::from_secs_f64(ttl))
    })
}

pub fn module() -> FFIModule {
    let mut module = FFIModule::new("steel/cord-hx");
    module
        .register_fn("cord-attach", attach)
        .register_fn("cord-next-action", next_action)
        .register_fn("cord-reply", reply)
        .register_fn("cord-start", start)
        .register_fn("cord-configure", configure)
        .register_fn("cord-event", event)
        .register_fn("cord-command", command)
        .register_fn("cord-manager", manager)
        .register_fn("cord-icon", icon)
        .register_fn("cord-run", run_command);
    module
}
