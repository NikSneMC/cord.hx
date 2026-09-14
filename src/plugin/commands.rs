//! `:cord` commands.

use super::core::Core;
use super::log;
use super::report;
use super::server;
use super::util;

fn toggle_idle_feature(core: &mut Core, enable: Option<bool>) {
    let value = enable.unwrap_or(!core.config.idle_enabled);
    core.config.idle_enabled = value;
    if enable.is_some() || !value {
        core.manager_unidle();
    }
}

fn feature(core: &mut Core, name: &str, enable: Option<bool>) {
    match name {
        "idle" => toggle_idle_feature(core, enable),
        _ => core.log_notify(&format!("Unknown option: '{name}'"), log::ERROR),
    }
}

fn version(core: &mut Core) {
    let exe = core.config.executable();
    core.spawn(
        move || util::run(&exe, &["-v"], None),
        |core, output| match output {
            Ok(o) if o.code == 0 => core.log_notify(
                &format!("Server version: {}", o.stdout.trim()),
                log::INFO,
            ),
            _ => core.log_notify(
                "Could not determine the server version",
                log::WARN,
            ),
        },
    );
}

/// Handles `:cord <args>`.
pub fn handle(core: &mut Core, args: &[String]) {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.replace('"', ""))
        .filter(|a| !a.is_empty())
        .collect();
    let Some(cmd) = args.first() else {
        core.log_notify("No command provided", log::ERROR);
        return;
    };
    let sub = args.get(1).map(String::as_str);

    let unknown_sub = |core: &mut Core, sub: &str| {
        core.log_notify(
            &format!("Unknown option: '{sub}' for command '{cmd}'"),
            log::ERROR,
        )
    };

    match (cmd.as_str(), sub) {
        ("enable", None) => core.manager_resume(),
        ("enable", Some(f)) => feature(core, f, Some(true)),
        ("disable", None) => core.manager_hide(),
        ("disable", Some(f)) => feature(core, f, Some(false)),
        ("toggle", None) => core.manager_toggle(),
        ("toggle", Some(f)) => feature(core, f, None),

        ("presence", None | Some("toggle")) => core.manager_toggle(),
        ("presence", Some("show")) => core.manager_resume(),
        ("presence", Some("hide")) => core.manager_hide(),
        ("presence", Some("suppress")) => core.manager_suppress(),
        ("presence", Some("toggle-suppress")) => core.manager_toggle_suppress(),
        ("presence", Some(s)) => unknown_sub(core, s),

        ("idle", None | Some("toggle")) => core.manager_toggle_idle(false),
        ("idle", Some("show")) => core.manager_idle(),
        ("idle", Some("hide")) => core.manager_unidle(),
        ("idle", Some("force")) => core.manager_force_idle(),
        ("idle", Some(s)) => unknown_sub(core, s),

        ("debug", None) => report::show_debug(core),
        ("debug", Some("copy")) => report::copy_debug(core),
        ("debug", Some(s)) => unknown_sub(core, s),

        (
            "status" | "version" | "restart" | "reconnect" | "shutdown"
            | "health",
            Some(s),
        ) => core.log_notify(
            &format!("Unknown subcommand: '{s}' for command '{cmd}'"),
            log::ERROR,
        ),
        ("status", None) => {
            let status = core.server.status.describe();
            core.log_notify(&format!("Status: {status}"), log::INFO);
        }
        ("version", None) => version(core),
        ("restart", None) => server::restart(core),
        ("reconnect", None) => server::reconnect(core),
        ("shutdown", None) => server::shutdown(core),
        ("health", None) => report::show_health(core),

        (other, _) => {
            core.log_notify(&format!("Unknown command: '{other}'"), log::ERROR)
        }
    }
}
