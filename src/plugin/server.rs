//! Connection to the cord server.

use serde_json::{Value, json};

use super::client::{self, Connection};
use super::config::Config;
use super::core::Core;
use super::log;
use super::util::{self, Spawn};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Disconnected,
    Initializing,
    Initialized,
    Connecting,
    Connected,
    Ready,
}

impl Status {
    pub fn describe(self) -> &'static str {
        match self {
            Status::Disconnected => "Disconnected",
            Status::Initializing => "Connecting to server",
            Status::Initialized => "Connected to server",
            Status::Connecting => "Connecting to Discord",
            Status::Connected => "Handshaking with Discord",
            Status::Ready => "Connected to Discord",
        }
    }
}

#[derive(Default)]
pub struct Server {
    pub status: Status,
    connection: Option<Connection>,
    /// Incremented on every connection, to ignore messages of older ones.
    generation: u64,
    initializing: bool,
    shut_down: bool,
    /// Reconnect once the current connection is closed.
    reconnect_on_close: bool,
    /// The `initialize` event last sent on this connection.
    initialized_with: Option<Value>,
}

impl Server {
    pub fn is_connected(&self) -> bool {
        self.connection.as_ref().is_some_and(Connection::is_open)
    }

    pub fn is_ready(&self) -> bool {
        self.status == Status::Ready
    }
}

/// Whether two configurations can share a server connection.
pub fn same_connection(a: &Config, b: &Config) -> bool {
    a.pipe_path() == b.pipe_path()
        && a.executable() == b.executable()
        && a.client_id == b.client_id
}

pub fn send(core: &mut Core, ty: &str, data: Value) {
    core.log(log::TRACE, || format!("Sender:send_event: type={ty}"));
    let Some(connection) = &core.server.connection else {
        return;
    };
    if let Err(e) = connection.send(&json!({ "type": ty, "data": data })) {
        connection.close();
        core.log(log::DEBUG, || format!("Failed to send {ty}: {e}"));
    }
}

pub fn initialize(core: &mut Core) {
    if core.server.initializing {
        core.log(log::DEBUG, || {
            "Initialization already in progress".to_string()
        });
        return;
    }
    core.server.initializing = true;
    core.log(log::DEBUG, || "Initializing server...".to_string());
    let path = core.config.pipe_path();
    connect(core, path, false);
}

fn fail(core: &mut Core, message: String) {
    core.server.initializing = false;
    core.server.status = Status::Disconnected;
    core.log(log::ERROR, || message);
}

fn connect(core: &mut Core, path: String, retried: bool) {
    core.server.status = Status::Initializing;
    core.server.shut_down = false;
    core.log(log::DEBUG, || format!("Connecting to server at {path}"));

    core.server.generation += 1;
    let generation = core.server.generation;
    let post = core.sender();
    let result = client::connect(&path, move |message| {
        post(Box::new(move |core| on_message(core, generation, message)));
    });

    match result {
        Ok(connection) => {
            core.server.connection = Some(connection);
            core.server.initialized_with = None;
            core.server.initializing = false;
            core.server.status = Status::Initialized;
            core.log(log::DEBUG, || "Connected to server".to_string());
            send_initialize(core);
        }
        Err(_) if retried => {
            fail(core, format!("Failed to connect to pipe: {path}"))
        }
        Err(code) => match code.as_str() {
            "ENOENT" | "ECONNRESET" => spawn(core, path),
            "ECONNREFUSED" | "ETIMEDOUT" => {
                core.log(log::DEBUG, || {
                    "Found stale pipe. Removing...".to_string()
                });
                let _ = std::fs::remove_file(&path);
                spawn(core, path);
            }
            _ => fail(core, format!("Failed to connect to pipe: {code}")),
        },
    }
}

fn spawn(core: &mut Core, path: String) {
    let exe = core.config.executable();
    if !std::path::Path::new(&exe).exists() {
        core.server.initializing = false;
        core.server.status = Status::Disconnected;
        core.log_notify(
            &format!(
                "Could not find the server executable at {exe}. Reinstall the plugin with forge or set advanced.server.executable-path"
            ),
            log::ERROR,
        );
        return;
    }

    let c = &core.config;
    let mut args = vec![
        "-p".to_string(),
        path.clone(),
        "-c".to_string(),
        c.client_id.clone(),
        "-t".to_string(),
        c.server_timeout.to_string(),
        "-r".to_string(),
        if c.reconnect_enabled {
            c.reconnect_interval
        } else {
            0
        }
        .to_string(),
    ];
    if c.reconnect_initial {
        args.push("-i".to_string());
    }

    core.log(log::DEBUG, || {
        "Pipe not found. Spawning server executable...".to_string()
    });
    core.spawn(
        move || util::spawn_daemon(&exe, &args),
        move |core, result| match result {
            Spawn::Ready => {
                core.log(log::DEBUG, || {
                    "Spawn: server signaled Ready".to_string()
                });
                connect(core, path, true);
            }
            Spawn::Retry => {
                core.log(log::DEBUG, || {
                    "Spawn: another instance detected; will retry connect"
                        .to_string()
                });
                connect(core, path, false);
            }
            Spawn::Error(e) => fail(core, e),
        },
    );
}

fn initialize_event(config: &Config) -> Value {
    json!({
        "log_level": config.log_level,
        "timestamp": { "shared": config.shared_timestamp },
        "advanced": {
            "discord": {
                "pipe_paths": config.discord_pipe_paths,
                "sync": {
                    "enabled": config.sync_enabled,
                    "mode": config.sync_mode,
                    "interval": config.sync_interval,
                    "reset_on_update": config.sync_reset_on_update,
                    "pad": config.sync_pad,
                }
            }
        }
    })
}

/// Sends the `initialize` event, unless it was already sent with the same
/// configuration.
pub fn send_initialize(core: &mut Core) {
    let event = initialize_event(&core.config);
    if core.server.initialized_with.as_ref() == Some(&event) {
        return;
    }
    core.server.initialized_with = Some(event.clone());
    send(core, "initialize", event);
}

fn on_message(core: &mut Core, generation: u64, message: Option<Value>) {
    if generation != core.server.generation {
        return;
    }
    let Some(message) = message else {
        on_close(core);
        return;
    };
    let Some(ty) = message.get("type").and_then(Value::as_str) else {
        core.log(log::ERROR, || "Invalid event format".to_string());
        return;
    };
    let data = message.get("data").cloned().unwrap_or(Value::Null);
    core.log(log::TRACE, || format!("Receiver:on_event: type={ty}"));
    match ty {
        "status_update" => {
            let status = data
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            on_status_update(core, &status);
        }
        "restart" => {
            core.persisted = Default::default();
            core.server.reconnect_on_close = true;
        }
        "log" => core.log_server(std::slice::from_ref(&data)),
        "log_batch" => {
            if let Value::Array(logs) = &data {
                core.log_server(logs);
            }
        }
        "decode_error" => core.log(log::ERROR, || {
            format!(
                "Failed to decode event: {}",
                super::value::to_display(&data)
            )
        }),
        _ => {}
    }
}

fn on_status_update(core: &mut Core, status: &str) {
    match status {
        "connecting" => {
            core.server.status = Status::Connecting;
            core.log(log::DEBUG, || "Connecting to Discord...".to_string());
        }
        "connected" => {
            core.server.status = Status::Connected;
            core.log(log::DEBUG, || "Handshaking with Discord...".to_string());
        }
        "ready" => {
            if core.server.status == Status::Ready {
                core.log(log::DEBUG, || {
                    "status_update: ready (already ready, skipping)".to_string()
                });
                return;
            }
            core.server.status = Status::Ready;
            core.log(log::INFO, || "Connected to Discord".to_string());
            create_manager(core);
        }
        "disconnected" => {
            let was_connected =
                matches!(core.server.status, Status::Ready | Status::Connected);
            core.server.status = Status::Initialized;
            core.manager_cleanup();
            run_shutdown_hooks(core);
            if was_connected && core.config.reconnect_enabled {
                core.log(log::INFO, || "Reconnecting...".to_string());
            }
        }
        _ => {}
    }
}

/// Creates the activity manager for the ready connection.
pub fn create_manager(core: &mut Core) {
    core.manager_cleanup();
    core.manager_new();
    core.manager_run();
}

fn run_shutdown_hooks(core: &mut Core) {
    if !core.server.shut_down {
        core.server.shut_down = true;
        core.run_hooks("shutdown", &[]);
    }
}

fn on_close(core: &mut Core) {
    core.log(log::DEBUG, || "client: on_close".to_string());
    core.server.connection = None;
    core.server.initialized_with = None;
    core.server.status = Status::Disconnected;
    core.manager_cleanup();
    run_shutdown_hooks(core);
    if std::mem::take(&mut core.server.reconnect_on_close) {
        initialize(core);
    }
}

/// Drops the manager and closes the connection.
pub fn cleanup(core: &mut Core) {
    core.manager_cleanup();
    if let Some(connection) = &core.server.connection {
        connection.close();
    }
}

/// Closes the connection and connects again with the current configuration.
pub fn restart_connection(core: &mut Core) {
    if core.server.is_connected() {
        core.server.reconnect_on_close = true;
        cleanup(core);
    } else {
        initialize(core);
    }
}

/// `:cord restart`: restarts the server, which affects every editor.
pub fn restart(core: &mut Core) {
    core.log(log::DEBUG, || "Restarting...".to_string());
    core.manager_cleanup();
    if !core.server.is_connected() {
        initialize(core);
        return;
    }
    core.server.status = Status::Disconnected;
    run_shutdown_hooks(core);
    core.server.reconnect_on_close = true;
    send(core, "restart", Value::Null);
}

pub fn reconnect(core: &mut Core) {
    core.log(log::DEBUG, || "Reconnecting...".to_string());
    if core.server.is_connected() {
        send(core, "reconnect", Value::Null);
    } else {
        initialize(core);
    }
}

pub fn shutdown(core: &mut Core) {
    if !core.server.is_connected() {
        core.log_notify("Server is not running", log::INFO);
        return;
    }
    send(core, "shutdown", Value::Null);
    cleanup(core);
    core.log_notify("Stopped server", log::INFO);
}
