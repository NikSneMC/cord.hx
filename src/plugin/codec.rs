//! JSON <-> msgpack conversion for the server protocol.

use serde_json::{Map, Value};

/// Converts a JSON value into msgpack. `null` object members are dropped,
/// mirroring how Lua tables with `nil` fields were encoded by `vim.mpack`.
pub fn value_to_msgpack(value: Value) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &json_to_rmpv(value))
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn json_to_rmpv(value: Value) -> rmpv::Value {
    match value {
        Value::Null => rmpv::Value::Nil,
        Value::Bool(b) => rmpv::Value::Boolean(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rmpv::Value::from(i)
            } else if let Some(u) = n.as_u64() {
                rmpv::Value::from(u)
            } else {
                rmpv::Value::F64(n.as_f64().unwrap_or_default())
            }
        }
        Value::String(s) => rmpv::Value::String(s.into()),
        Value::Array(arr) => {
            rmpv::Value::Array(arr.into_iter().map(json_to_rmpv).collect())
        }
        Value::Object(map) => rmpv::Value::Map(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (rmpv::Value::String(k.into()), json_to_rmpv(v)))
                .collect(),
        ),
    }
}

pub fn rmpv_to_json(value: rmpv::Value) -> Value {
    use rmpv::Value as M;

    match value {
        M::Nil | M::Ext(..) => Value::Null,
        M::Boolean(b) => Value::Bool(b),
        M::Integer(i) => match (i.as_i64(), i.as_u64()) {
            (Some(i), _) => Value::from(i),
            (_, Some(u)) => Value::from(u),
            _ => Value::Null,
        },
        M::F32(f) => Value::from(f as f64),
        M::F64(f) => Value::from(f),
        M::String(s) => Value::String(s.into_str().unwrap_or_default()),
        M::Binary(b) => Value::String(String::from_utf8_lossy(&b).into_owned()),
        M::Array(arr) => {
            Value::Array(arr.into_iter().map(rmpv_to_json).collect())
        }
        M::Map(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        M::String(s) => s.into_str().unwrap_or_default(),
                        other => other.to_string(),
                    };
                    (key, rmpv_to_json(v))
                })
                .collect::<Map<_, _>>(),
        ),
    }
}

/// Converts kebab-case keys to the server's snake_case field names
/// (`idle?` becomes `is_idle`).
pub fn to_wire(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let key = match k.strip_suffix('?') {
                        Some(base) => format!("is_{}", base.replace('-', "_")),
                        None => k.replace('-', "_"),
                    };
                    (key, to_wire(v))
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(to_wire).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_keys() {
        assert_eq!(
            to_wire(
                json!({"large-image": "x", "idle?": true, "buttons": [{"label-text": 1}]})
            ),
            json!({"large_image": "x", "is_idle": true, "buttons": [{"label_text": 1}]})
        );
    }

    #[test]
    fn msgpack_round_trip_drops_nulls() {
        let bytes =
            value_to_msgpack(json!({"a": 1, "b": null, "c": [true, "s"]}))
                .unwrap();
        let back = rmpv_to_json(
            rmpv::decode::read_value(&mut bytes.as_slice()).unwrap(),
        );
        assert_eq!(back, json!({"a": 1, "c": [true, "s"]}));
    }
}
