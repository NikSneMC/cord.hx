//! `'local-time`: shows the local time of day as the activity timestamp.
//!
//! Config: `'affect-idle?` (default #t) also applies to the idle activity.

use serde_json::{Value, json};

use super::{Extension, Setup};
use crate::plugin::core::Core;
use crate::plugin::util;

pub fn setup(_: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    let affect_idle = s.bool("affect-idle?", true)?;
    let mut ext = Extension::default();
    ext.variable("local-timestamp", |_, _| json!(util::local_midnight()));
    ext.hook("post-activity", 0, move |_, args| {
        let idle = args
            .first()
            .and_then(|o| o.get("idle?"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match args.get(1) {
            Some(activity) if affect_idle || !idle => {
                let mut activity = activity.clone();
                activity["timestamps"]["start"] = json!(util::local_midnight());
                activity
            }
            _ => Value::Null,
        }
    });
    Ok(ext)
}
