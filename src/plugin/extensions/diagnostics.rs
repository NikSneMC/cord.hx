//! `'diagnostics`: appends the number of problems to the activity text.
//!
//! Config:
//! * `'scope` - `'buffer` (default) or `'workspace`
//! * `'severity` - minimum severity counted: `'error`, `'warning` (default), `'info` or `'hint`
//! * `'override?` - replace the `viewing`/`editing` (or `workspace`) text (default #t)
//!
//! Counts come from Helix (`editor-document-diagnostic-counts`); builds that
//! do not expose diagnostics report zero problems.

use serde_json::{Value, json};

use super::{Extension, Setup};
use crate::plugin::core::Core;
use crate::plugin::value::to_display;

fn count(core: &Core, workspace: bool, min: usize) -> u64 {
    let counts = if workspace {
        core.editor.workspace_diagnostics
    } else {
        core.editor.diagnostics
    };
    counts.map(|c| c[min..].iter().sum()).unwrap_or(0)
}

fn suffix(core: &Core, workspace: bool, min: usize) -> String {
    match count(core, workspace, min) {
        0 => String::new(),
        1 => " - 1 problem".to_string(),
        n => format!(" - {n} problems"),
    }
}

pub fn setup(_: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    let workspace =
        s.choice("scope", "buffer", &["buffer", "workspace"])? == "workspace";
    let min = match s
        .choice("severity", "warning", &["error", "warning", "info", "hint"])?
        .as_str()
    {
        "hint" => 0,
        "info" => 1,
        "warning" => 2,
        _ => 3,
    };
    let override_text = s.bool("override?", true)?;

    let mut ext = Extension::default();
    ext.variable("diagnostics", move |core, _| {
        json!(count(core, workspace, min))
    });

    if override_text {
        let texts: &[(&str, &'static str, &'static str)] = if workspace {
            &[("workspace", "In ", "workspace")]
        } else {
            &[
                ("viewing", "Viewing ", "filename"),
                ("editing", "Editing ", "filename"),
            ]
        };
        for &(key, prefix, field) in texts {
            ext.text(key, move |core, args| {
                let v = args.first().and_then(|o| o.get(field)).cloned();
                json!(format!(
                    "{prefix}{}{}",
                    to_display(&v.unwrap_or(Value::Null)),
                    suffix(core, workspace, min)
                ))
            });
        }
    }
    Ok(ext)
}
