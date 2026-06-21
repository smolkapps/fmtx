//! The conversion core.
//!
//! Everything funnels through [`serde_json::Value`] as the universal
//! intermediate representation (IR). Each supported format gets a pair of
//! functions: a parser `<fmt>_to_value` and a serializer `value_to_<fmt>`.
//! The public entry point [`convert`] ties them together: parse the input
//! bytes from the source format into a `Value`, then serialize that `Value`
//! into the target format.
//!
//! ## CSV is special
//! CSV is not a tree format. We model a CSV file as a JSON **array of
//! objects** where the header row supplies the keys and every value is a
//! string. Converting *to* CSV therefore requires the IR to be an array of
//! objects (a "table"); anything else is rejected with a clear error.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};

use crate::format::Format;

/// Convert `input` from `from` format into `to` format.
///
/// `pretty` controls whether the JSON / TOML output is human-readable
/// (multi-line, indented) or compact. It has no effect on CSV (always rows)
/// and limited effect on YAML (serde_yaml is always block style).
pub fn convert(input: &str, from: Format, to: Format, pretty: bool) -> Result<String> {
    let value = parse_to_value(input, from)
        .with_context(|| format!("failed to parse input as {}", from))?;
    serialize_from_value(&value, to, pretty)
        .with_context(|| format!("failed to serialize output as {}", to))
}

/// Parse the textual `input` of format `from` into the `Value` IR.
pub fn parse_to_value(input: &str, from: Format) -> Result<Value> {
    match from {
        Format::Json => json_to_value(input),
        Format::Yaml => yaml_to_value(input),
        Format::Toml => toml_to_value(input),
        Format::Csv => csv_to_value(input),
    }
}

/// Serialize a `Value` IR into the textual representation of format `to`.
pub fn serialize_from_value(value: &Value, to: Format, pretty: bool) -> Result<String> {
    match to {
        Format::Json => value_to_json(value, pretty),
        Format::Yaml => value_to_yaml(value),
        Format::Toml => value_to_toml(value, pretty),
        Format::Csv => value_to_csv(value),
    }
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn json_to_value(input: &str) -> Result<Value> {
    serde_json::from_str(input).map_err(|e| anyhow!("invalid JSON: {}", e))
}

fn value_to_json(value: &Value, pretty: bool) -> Result<String> {
    let s = if pretty {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    Ok(s)
}

// ---------------------------------------------------------------------------
// YAML
// ---------------------------------------------------------------------------

fn yaml_to_value(input: &str) -> Result<Value> {
    // serde_yaml can deserialize straight into a serde_json::Value because
    // Value implements Deserialize. YAML scalars map cleanly onto JSON
    // scalars (numbers, bools, strings, null).
    serde_yaml::from_str(input).map_err(|e| anyhow!("invalid YAML: {}", e))
}

fn value_to_yaml(value: &Value) -> Result<String> {
    serde_yaml::to_string(value).map_err(|e| anyhow!("could not encode YAML: {}", e))
}

// ---------------------------------------------------------------------------
// TOML
// ---------------------------------------------------------------------------

fn toml_to_value(input: &str) -> Result<Value> {
    // toml deserializes into a serde_json::Value. TOML's top level is always
    // a table, so this yields a JSON object. TOML datetimes become strings.
    let value: Value = toml::from_str(input).map_err(|e| anyhow!("invalid TOML: {}", e))?;
    Ok(value)
}

fn value_to_toml(value: &Value, pretty: bool) -> Result<String> {
    // TOML *requires* the top-level value to be a table (JSON object). Catch
    // the common mistakes (scalar / array at top level) with a clear message
    // before handing off to the toml encoder.
    match value {
        Value::Object(_) => {}
        Value::Array(_) => bail!(
            "cannot serialize a top-level array to TOML: TOML documents must be a \
             table (key/value map) at the top level. Wrap the array in an object, \
             e.g. {{ \"items\": [ ... ] }}."
        ),
        _ => bail!(
            "cannot serialize a top-level scalar to TOML: TOML documents must be a \
             table (key/value map) at the top level."
        ),
    }

    // TOML has no `null`. serde_json `null` values would either error or be
    // silently dropped depending on position; reject them up front so the
    // user gets a precise message instead of a confusing encoder error.
    if let Some(path) = find_null(value, String::new()) {
        bail!(
            "cannot serialize to TOML: value at '{}' is null, and TOML has no null \
             type. Remove the key or give it a concrete value.",
            if path.is_empty() { "<root>" } else { &path }
        );
    }

    let s = if pretty {
        toml::to_string_pretty(value)?
    } else {
        toml::to_string(value)?
    };
    Ok(s)
}

/// Recursively search a `Value` for the first JSON `null`, returning a dotted
/// path to it (best-effort, for the error message). Returns `None` if there is
/// no null anywhere in the tree.
fn find_null(value: &Value, path: String) -> Option<String> {
    match value {
        Value::Null => Some(path),
        Value::Object(map) => {
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", path, k)
                };
                if let Some(found) = find_null(v, child) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{}[{}]", path, i);
                if let Some(found) = find_null(v, child) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Parse CSV text into a JSON array of objects. The first row is the header
/// and supplies the keys; every subsequent row becomes one object. All values
/// are strings — CSV carries no type information.
fn csv_to_value(input: &str) -> Result<Value> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(input.as_bytes());

    let headers = reader
        .headers()
        .map_err(|e| anyhow!("invalid CSV header: {}", e))?
        .clone();

    if headers.is_empty() {
        bail!("CSV input has no header row");
    }

    let mut rows = Vec::new();
    for (line_no, record) in reader.records().enumerate() {
        let record =
            record.map_err(|e| anyhow!("invalid CSV at data row {}: {}", line_no + 1, e))?;
        let mut obj = Map::new();
        for (header, field) in headers.iter().zip(record.iter()) {
            obj.insert(header.to_string(), Value::String(field.to_string()));
        }
        rows.push(Value::Object(obj));
    }

    Ok(Value::Array(rows))
}

/// Serialize a JSON array of objects into CSV. The union of all object keys
/// (in first-seen order) forms the header. Missing keys become empty cells.
///
/// Errors clearly when the IR is not tabular: a scalar, a top-level object, or
/// an array whose elements aren't all objects cannot be represented as CSV.
fn value_to_csv(value: &Value) -> Result<String> {
    let arr = match value {
        Value::Array(arr) => arr,
        Value::Object(_) => bail!(
            "cannot convert to CSV: the data is a single object, not a table. CSV \
             output requires an array of objects (one object per row). Wrap your \
             rows in a JSON array, e.g. [ {{ ... }}, {{ ... }} ]."
        ),
        _ => bail!(
            "cannot convert to CSV: the data is a scalar value, not a table. CSV \
             output requires an array of objects (one object per row)."
        ),
    };

    if arr.is_empty() {
        // An empty table: no header, no rows. Emit an empty document.
        return Ok(String::new());
    }

    // Collect the header as the union of all keys, preserving first-seen order
    // (serde_json with preserve_order keeps insertion order within each object).
    let mut header: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (idx, item) in arr.iter().enumerate() {
        let obj = item.as_object().ok_or_else(|| {
            anyhow!(
                "cannot convert to CSV: array element {} is not an object. Every \
                 element must be an object (a row) whose keys are the columns.",
                idx
            )
        })?;
        for key in obj.keys() {
            if seen.insert(key.clone()) {
                header.push(key.clone());
            }
        }
    }

    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(vec![]);

    writer
        .write_record(&header)
        .map_err(|e| anyhow!("could not write CSV header: {}", e))?;

    for (idx, item) in arr.iter().enumerate() {
        let obj = item.as_object().unwrap(); // validated above
        let mut row = Vec::with_capacity(header.len());
        for key in &header {
            let cell = match obj.get(key) {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Number(n)) => n.to_string(),
                Some(nested @ (Value::Array(_) | Value::Object(_))) => bail!(
                    "cannot convert to CSV: row {} key '{}' holds a nested {} value. \
                     CSV cells must be scalars (string/number/bool/null); nested \
                     arrays and objects cannot be represented.",
                    idx,
                    key,
                    match nested {
                        Value::Array(_) => "array",
                        _ => "object",
                    }
                ),
            };
            row.push(cell);
        }
        writer
            .write_record(&row)
            .map_err(|e| anyhow!("could not write CSV row {}: {}", idx, e))?;
    }

    let bytes = writer
        .into_inner()
        .map_err(|e| anyhow!("could not finalize CSV: {}", e))?;
    String::from_utf8(bytes).map_err(|e| anyhow!("CSV output was not valid UTF-8: {}", e))
}

// ---------------------------------------------------------------------------
// Unit tests for the conversion core
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Parse a JSON literal into the IR for terse test setup.
    fn v(input: &str) -> Value {
        serde_json::from_str(input).unwrap()
    }

    // ---- JSON ----

    #[test]
    fn json_roundtrip_pretty_and_compact() {
        let value = v(r#"{"b":2,"a":1,"nested":{"x":[1,2,3]}}"#);
        let pretty = value_to_json(&value, true).unwrap();
        let compact = value_to_json(&value, false).unwrap();
        assert!(pretty.contains('\n'));
        assert!(!compact.contains('\n'));
        assert_eq!(json_to_value(&pretty).unwrap(), value);
        assert_eq!(json_to_value(&compact).unwrap(), value);
    }

    #[test]
    fn json_preserves_key_order() {
        // With preserve_order, keys come back in insertion order, not sorted.
        let value = v(r#"{"z":1,"a":2,"m":3}"#);
        let out = value_to_json(&value, false).unwrap();
        assert_eq!(out, r#"{"z":1,"a":2,"m":3}"#);
    }

    // ---- YAML <-> JSON ----

    #[test]
    fn yaml_json_roundtrip_preserves_data() {
        let yaml = "\
name: fmtx
version: 1
tags:
  - cli
  - rust
nested:
  enabled: true
  ratio: 3.5
  note: ~
";
        let value = yaml_to_value(yaml).unwrap();
        // Round-trip yaml -> value -> yaml -> value and compare the IR.
        let yaml2 = value_to_yaml(&value).unwrap();
        let value2 = yaml_to_value(&yaml2).unwrap();
        assert_eq!(value, value2);

        // Spot-check the parsed structure.
        assert_eq!(value["name"], json!("fmtx"));
        assert_eq!(value["version"], json!(1));
        assert_eq!(value["tags"], json!(["cli", "rust"]));
        assert_eq!(value["nested"]["enabled"], json!(true));
        assert_eq!(value["nested"]["ratio"], json!(3.5));
        assert_eq!(value["nested"]["note"], json!(null));
    }

    #[test]
    fn yaml_to_json_to_yaml_full_cycle() {
        let yaml = "a: 1\nb:\n  - x\n  - y\n";
        let json = convert(yaml, Format::Yaml, Format::Json, true).unwrap();
        let back = convert(&json, Format::Json, Format::Yaml, true).unwrap();
        assert_eq!(yaml_to_value(yaml).unwrap(), yaml_to_value(&back).unwrap());
    }

    // ---- TOML <-> JSON ----

    #[test]
    fn json_toml_roundtrip_preserves_data() {
        let value = v(r#"{
            "title": "demo",
            "count": 7,
            "ratio": 2.5,
            "enabled": true,
            "tags": ["a", "b"],
            "owner": { "name": "michael", "admin": true }
        }"#);
        let toml_str = value_to_toml(&value, true).unwrap();
        let back = toml_to_value(&toml_str).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn toml_top_level_array_is_error() {
        let value = v(r#"[1,2,3]"#);
        let err = value_to_toml(&value, true).unwrap_err();
        assert!(err.to_string().contains("top-level array"));
    }

    #[test]
    fn toml_top_level_scalar_is_error() {
        let value = v(r#"42"#);
        let err = value_to_toml(&value, true).unwrap_err();
        assert!(err.to_string().contains("top-level scalar"));
    }

    #[test]
    fn toml_null_value_is_error_with_path() {
        let value = v(r#"{"a": {"b": null}}"#);
        let err = value_to_toml(&value, true).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("null"));
        assert!(msg.contains("a.b"), "expected path a.b in: {}", msg);
    }

    // ---- CSV <-> JSON ----

    #[test]
    fn csv_json_roundtrip_small_table() {
        let csv_in = "name,age,city\nAlice,30,NYC\nBob,25,LA\n";
        let value = csv_to_value(csv_in).unwrap();
        assert_eq!(
            value,
            json!([
                {"name": "Alice", "age": "30", "city": "NYC"},
                {"name": "Bob", "age": "25", "city": "LA"}
            ])
        );
        let csv_out = value_to_csv(&value).unwrap();
        // Round-trip back through the parser to compare semantically (avoids
        // any line-ending nitpicks).
        assert_eq!(csv_to_value(&csv_out).unwrap(), value);
        // And the literal output should match the canonical form.
        assert_eq!(csv_out, "name,age,city\nAlice,30,NYC\nBob,25,LA\n");
    }

    #[test]
    fn csv_handles_quoting_and_commas() {
        let csv_in = "name,note\n\"Smith, John\",\"says \"\"hi\"\"\"\n";
        let value = csv_to_value(csv_in).unwrap();
        assert_eq!(value[0]["name"], json!("Smith, John"));
        assert_eq!(value[0]["note"], json!("says \"hi\""));
        // Re-serialize and re-parse: data must survive.
        let csv_out = value_to_csv(&value).unwrap();
        assert_eq!(csv_to_value(&csv_out).unwrap(), value);
    }

    #[test]
    fn csv_from_scalar_is_error() {
        let value = v(r#"42"#);
        let err = value_to_csv(&value).unwrap_err();
        assert!(err.to_string().contains("scalar"));
    }

    #[test]
    fn csv_from_object_is_error() {
        let value = v(r#"{"a": 1}"#);
        let err = value_to_csv(&value).unwrap_err();
        assert!(err.to_string().contains("single object"));
    }

    #[test]
    fn csv_from_array_of_scalars_is_error() {
        let value = v(r#"[1, 2, 3]"#);
        let err = value_to_csv(&value).unwrap_err();
        assert!(err.to_string().contains("not an object"));
    }

    #[test]
    fn csv_from_nested_value_is_error() {
        let value = v(r#"[{"a": {"deep": 1}}]"#);
        let err = value_to_csv(&value).unwrap_err();
        assert!(err.to_string().contains("nested"));
    }

    #[test]
    fn csv_writes_number_and_bool_cells() {
        // JSON (typed) -> CSV stringifies scalars.
        let value = v(r#"[{"n": 3, "b": true, "s": "hi"}]"#);
        let csv_out = value_to_csv(&value).unwrap();
        assert_eq!(csv_out, "n,b,s\n3,true,hi\n");
    }

    #[test]
    fn csv_union_header_and_missing_cells() {
        // Rows with differing keys: header is the union; missing -> empty.
        let value = v(r#"[{"a": "1"}, {"a": "2", "b": "3"}]"#);
        let csv_out = value_to_csv(&value).unwrap();
        assert_eq!(csv_out, "a,b\n1,\n2,3\n");
    }

    #[test]
    fn csv_empty_array_is_empty_document() {
        let value = v(r#"[]"#);
        assert_eq!(value_to_csv(&value).unwrap(), "");
    }

    // ---- malformed input ----

    #[test]
    fn malformed_json_errors() {
        assert!(parse_to_value("{not valid", Format::Json).is_err());
    }

    #[test]
    fn malformed_toml_errors() {
        assert!(parse_to_value("this is = = broken", Format::Toml).is_err());
    }

    // ---- cross-format via the public `convert` entry point ----

    #[test]
    fn convert_yaml_to_toml_roundtrips_through_json() {
        let yaml = "title: hi\ncount: 3\nflag: true\n";
        let toml_str = convert(yaml, Format::Yaml, Format::Toml, true).unwrap();
        let json = convert(&toml_str, Format::Toml, Format::Json, true).unwrap();
        let original = convert(yaml, Format::Yaml, Format::Json, true).unwrap();
        assert_eq!(v(&json), v(&original));
    }

    #[test]
    fn convert_csv_to_json_array_of_objects() {
        let csv_in = "k,v\nfoo,1\nbar,2\n";
        let json = convert(csv_in, Format::Csv, Format::Json, false).unwrap();
        assert_eq!(json, r#"[{"k":"foo","v":"1"},{"k":"bar","v":"2"}]"#);
    }
}
