//! Logging.
//!
//! Logs go to the file named by `CORD_LOG_FILE` when set, otherwise they are
//! shown in the editor. `CORD_LOG_LEVEL` overrides the configured level.

use std::io::Write;

use serde_json::Value;

use super::util;

pub const TRACE: u8 = 0;
pub const DEBUG: u8 = 1;
pub const INFO: u8 = 2;
pub const WARN: u8 = 3;
pub const ERROR: u8 = 4;
pub const OFF: u8 = 5;

pub fn level_name(level: u8) -> &'static str {
    match level {
        TRACE => "TRACE",
        DEBUG => "DEBUG",
        INFO => "INFO",
        WARN => "WARN",
        ERROR => "ERROR",
        _ => "OFF",
    }
}

/// Parses a level given as a number or a case-insensitive name.
pub fn parse_level(v: &Value) -> Option<u8> {
    match v {
        Value::Number(n) => n.as_u64().map(|n| n.min(OFF as u64) as u8),
        Value::String(s) => match s.parse::<u8>() {
            Ok(n) => Some(n.min(OFF)),
            Err(_) => match s.to_ascii_uppercase().as_str() {
                "TRACE" => Some(TRACE),
                "DEBUG" => Some(DEBUG),
                "INFO" => Some(INFO),
                "WARN" => Some(WARN),
                "ERROR" => Some(ERROR),
                "OFF" => Some(OFF),
                _ => None,
            },
        },
        _ => None,
    }
}

pub fn env_level() -> Option<u8> {
    util::env("CORD_LOG_LEVEL").and_then(|v| parse_level(&Value::String(v)))
}

pub enum Sink {
    File(String),
    /// Shown in the editor; the core forwards these as `notify` actions.
    Editor,
}

pub struct Logger {
    pub level: u8,
    pub sink: Sink,
}

impl Logger {
    pub fn from_env() -> Logger {
        let sink = match util::env("CORD_LOG_FILE") {
            Some(path) => {
                let path = util::normalize(&path);
                let path =
                    if path.starts_with('/') || path.get(1..2) == Some(":") {
                        path
                    } else {
                        format!(
                            "{}/{path}",
                            std::env::current_dir()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        )
                    };
                Sink::File(path)
            }
            None => Sink::Editor,
        };
        Logger {
            level: env_level().unwrap_or(OFF),
            sink,
        }
    }

    /// Sets the level, unless `CORD_LOG_LEVEL` overrides it.
    pub fn set_level(&mut self, level: u8) {
        if env_level().is_none() {
            self.level = level;
        }
    }

    pub fn enabled(&self, level: u8) -> bool {
        level >= self.level && level < OFF
    }

    /// Appends lines to the log file.
    pub fn write_file(path: &str, lines: &[String]) -> Result<(), String> {
        if lines.is_empty() {
            return Ok(());
        }
        let dir = util::head(path);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(format!("{}\n", lines.join("\n")).as_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn format(level: u8, message: &str) -> String {
        format!(
            "[{}] [{}] {message}",
            util::local_datetime(),
            level_name(level)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels() {
        assert_eq!(parse_level(&json!("Debug")), Some(DEBUG));
        assert_eq!(parse_level(&json!(3)), Some(WARN));
        assert_eq!(parse_level(&json!("2")), Some(INFO));
        assert_eq!(parse_level(&json!("nope")), None);
    }
}
