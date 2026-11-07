//! `insta probe`: prints the *structure* of Instagram's responses from the
//! live session (keys and types, never values), to check what insta assumes
//! about them.

use anyhow::Result;
use serde_json::Value;

use crate::ig::HttpInstagram;

/// What probe asks `HttpInstagram::raw` for.
pub const PROBE_TARGETS: &[&str] = &["whoami", "inbox", "thread"];

/// Keys that are Instagram ids (all digits) are data, not schema: such objects
/// print as `map[n] of <type>` so no id ever reaches the output.
fn is_id_key(k: &str) -> bool {
    k.len() >= 6 && k.chars().all(|c| c.is_ascii_digit())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn shape(v: &Value) -> String {
    let mut out = String::new();
    write_shape(v, 0, &mut out);
    out
}

fn write_shape(v: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    let Value::Object(map) = v else { return };
    for (k, val) in map {
        let key = if is_id_key(k) { "<id>" } else { k.as_str() };
        match val {
            Value::Object(inner) if !inner.is_empty() && inner.keys().all(|k| is_id_key(k)) => {
                if let Some(first) = inner.values().next() {
                    out.push_str(&format!("{pad}{key}: map[{}] of {}\n", inner.len(), type_name(first)));
                    write_shape(first, depth + 1, out);
                }
            }
            Value::Object(_) => {
                out.push_str(&format!("{pad}{key}: object\n"));
                write_shape(val, depth + 1, out);
            }
            Value::Array(items) => {
                let of = items.first().map(type_name).unwrap_or("?");
                out.push_str(&format!("{pad}{key}: array[{}] of {of}\n", items.len()));
                if let Some(first) = items.first() {
                    write_shape(first, depth + 1, out);
                }
            }
            other => out.push_str(&format!("{pad}{key}: {}\n", type_name(other))),
        }
    }
}

pub async fn probe(api: &HttpInstagram) -> Result<()> {
    for what in PROBE_TARGETS {
        println!("== {what}");
        match api.raw(what).await {
            Ok(v) => print!("{}", shape(&v)),
            // IgError text carries no response values.
            Err(e) => println!("error: {e}"),
        }
        println!();
    }
    Ok(())
}
