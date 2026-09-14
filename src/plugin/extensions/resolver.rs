//! `'resolver`: dynamic filetype detection.
//!
//! Config: `'sources` - `#t` for all sources, or `(hash 'nestjs #t)`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{Extension, Setup};
use crate::plugin::core::Core;
use crate::plugin::log;

const NESTJS_FILETYPES: &[&str] =
    &["angular", "javascript", "typescript", "tsx"];

pub fn setup(core: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    let enabled = |name: &str| match s.get("sources") {
        Some(Value::Bool(true)) => true,
        Some(sources) => {
            sources.get(name).and_then(Value::as_bool).unwrap_or(false)
        }
        None => false,
    };
    let nestjs = enabled("nestjs");

    let mut ext = Extension::default();
    if !nestjs {
        return Ok(ext);
    }
    core.log(log::DEBUG, || "resolver: registered nestjs".to_string());

    let cache: Arc<Mutex<HashMap<String, bool>>> = Arc::default();
    ext.hook("pre-activity", 100, move |core, args| {
        let Some(opts) = args.first() else {
            return Value::Null;
        };
        let filetype = opts
            .get("filetype")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !NESTJS_FILETYPES.contains(&filetype) {
            return Value::Null;
        }
        let dir = match opts.get("workspace-dir").and_then(Value::as_str) {
            Some(d) => d.to_string(),
            None => core.editor.cwd.clone(),
        };
        let is_nest =
            *cache.lock().unwrap().entry(dir.clone()).or_insert_with(|| {
                Path::new(&format!("{dir}/nest-cli.json")).exists()
            });
        if is_nest {
            let mut opts = opts.clone();
            opts["force-filetype"] = json!("nest");
            opts
        } else {
            Value::Null
        }
    });
    Ok(ext)
}
