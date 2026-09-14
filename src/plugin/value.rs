//! Values exchanged with Scheme.
//!
//! Scheme data is represented as JSON values. The Scheme side converts symbols
//! to strings and procedures to `{"__cord_proc": id}` markers before handing
//! data over (see `cogs/bridge.scm`), and converts hash map keys back to
//! symbols when receiving data.

use abi_stable::std_types::{RHashMap, RString, RVec};
use serde_json::{Map, Number, Value};
use steel::steel_vm::ffi::{FFIArg, FFIValue};

/// Key of the marker object standing for a Scheme procedure.
pub const PROC_KEY: &str = "__cord_proc";
/// Key of the marker object standing for a native procedure (see
/// `config::Natives`).
pub const NATIVE_KEY: &str = "__cord_native";
/// Key of the object a Scheme procedure call returns when it raised an error.
pub const ERROR_KEY: &str = "__cord_error";

pub fn from_ffi(arg: FFIArg<'_>) -> Value {
    match arg {
        FFIArg::StringRef(s) => Value::String(s.as_str().to_string()),
        FFIArg::StringV(s) => Value::String(s.into_string()),
        FFIArg::BoolV(b) => Value::Bool(b),
        FFIArg::IntV(i) => Value::Number((i as i64).into()),
        FFIArg::NumV(n) => number(n),
        FFIArg::CharV { c } => Value::String(c.to_string()),
        FFIArg::Vector(items) => {
            Value::Array(items.into_iter().map(from_ffi).collect())
        }
        FFIArg::HashMap(map) => Value::Object(
            map.into_iter()
                .map(|entry| {
                    let (k, v) = entry.into_tuple();
                    (key_string(k), from_ffi(v))
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

fn key_string(key: FFIArg<'_>) -> String {
    match from_ffi(key) {
        Value::String(s) => s,
        other => to_display(&other),
    }
}

fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        Value::Number((n as i64).into())
    } else {
        Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

pub fn to_ffi(value: &Value) -> FFIValue {
    match value {
        Value::Null => FFIValue::Void,
        Value::Bool(b) => FFIValue::BoolV(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => FFIValue::IntV(i as isize),
            None => FFIValue::NumV(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => FFIValue::StringV(RString::from(s.as_str())),
        Value::Array(items) => {
            FFIValue::Vector(items.iter().map(to_ffi).collect::<RVec<_>>())
        }
        Value::Object(map) => {
            let mut out = RHashMap::new();
            for (k, v) in map {
                out.insert(
                    FFIValue::StringV(RString::from(k.as_str())),
                    to_ffi(v),
                );
            }
            FFIValue::HashMap(out)
        }
    }
}

/// Id of a procedure marker.
pub fn proc_id(value: &Value) -> Option<u64> {
    match value {
        Value::Object(map) if map.len() == 1 => map.get(PROC_KEY)?.as_u64(),
        _ => None,
    }
}

pub fn proc_marker(id: u64) -> Value {
    let mut map = Map::new();
    map.insert(PROC_KEY.to_string(), Value::from(id));
    Value::Object(map)
}

/// Whether a value is a procedure marker (Scheme or native).
pub fn is_proc(value: &Value) -> bool {
    matches!(value, Value::Object(map)
        if map.len() == 1 && (map.contains_key(PROC_KEY) || map.contains_key(NATIVE_KEY)))
}

/// Lua's `tostring`, which the activity templates were written against.
pub fn to_display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "nil".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Type name used in validation messages.
pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "void",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) if is_proc(value) => "function",
        Value::Object(_) => "hash",
    }
}

/// Nested lookup by keys.
pub fn get<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |v, k| v.as_object()?.get(*k))
        .filter(|v| !v.is_null())
}

/// Sets a nested value, creating intermediate objects.
pub fn set(value: &mut Value, path: &[&str], new: Value) {
    let Some((last, parents)) = path.split_last() else {
        *value = new;
        return;
    };
    let mut cur = value;
    for key in parents {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        cur = cur
            .as_object_mut()
            .unwrap()
            .entry(key.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !cur.is_object() {
        *cur = Value::Object(Map::new());
    }
    cur.as_object_mut().unwrap().insert(last.to_string(), new);
}

/// `vim.tbl_deep_extend('force', base, other)`: objects are merged, other
/// values replaced. Procedure markers are replaced like any other value.
pub fn deep_merge(base: &mut Value, other: &Value) {
    match (base, other) {
        (Value::Object(base), Value::Object(entries)) if !is_proc(other) => {
            for (k, v) in entries {
                match base.get_mut(k) {
                    Some(existing @ Value::Object(_))
                        if !is_proc(existing)
                            && v.is_object()
                            && !is_proc(v) =>
                    {
                        deep_merge(existing, v)
                    }
                    _ => {
                        base.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (base, other) => *base = other.clone(),
    }
}

pub fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

pub fn non_empty(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deep_merge_replaces_scalars_and_merges_objects() {
        let mut base =
            json!({"a": {"b": 1, "c": 2}, "l": [1, 2], "p": {"x": 1}});
        deep_merge(
            &mut base,
            &json!({"a": {"b": 3}, "l": [3], "p": {"__cord_proc": 4}}),
        );
        assert_eq!(
            base,
            json!({"a": {"b": 3, "c": 2}, "l": [3], "p": {"__cord_proc": 4}})
        );
    }

    #[test]
    fn display_matches_lua_tostring() {
        assert_eq!(to_display(&json!(null)), "nil");
        assert_eq!(to_display(&json!(12)), "12");
        assert_eq!(to_display(&json!(true)), "true");
        assert_eq!(to_display(&json!("x")), "x");
    }

    #[test]
    fn nested_get_and_set() {
        let mut v = json!({});
        set(&mut v, &["a", "b"], json!(1));
        assert_eq!(get(&v, &["a", "b"]), Some(&json!(1)));
        assert_eq!(get(&v, &["a", "c"]), None);
    }
}
