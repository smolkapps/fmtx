//! Integration tests for the `fmtx` CLI binary.
//!
//! These drive the real compiled binary via `assert_cmd`, exercising the
//! actual argument-marshalling / stdin / stdout / file-I/O / exit-code paths
//! that unit tests on the conversion core cannot reach.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

/// Build a fresh `Command` for the `fmtx` binary under test.
fn fmtx() -> Command {
    Command::cargo_bin("fmtx").expect("binary `fmtx` should build")
}

/// A unique temp dir for a test, created under the OS temp dir. Returned path
/// is cleaned up by the OS eventually; tests don't depend on cleanup.
fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    dir.push(format!("fmtx-it-{}-{}", tag, nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// stdin -> stdout, explicit formats
// ---------------------------------------------------------------------------

#[test]
fn yaml_to_json_via_stdin_stdout() {
    fmtx()
        .args(["-f", "yaml", "-t", "json"])
        .write_stdin("name: fmtx\nversion: 1\nflags:\n  - a\n  - b\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"fmtx\""))
        .stdout(predicate::str::contains("\"version\": 1"))
        .stdout(predicate::str::contains("\"a\""));
}

#[test]
fn compact_flag_produces_single_line_json() {
    fmtx()
        .args(["-f", "yaml", "-t", "json", "--compact"])
        .write_stdin("a: 1\nb: 2\n")
        .assert()
        .success()
        // Compact JSON of {a:1,b:2} plus a single trailing newline.
        .stdout(predicate::eq("{\"a\":1,\"b\":2}\n"));
}

#[test]
fn pretty_is_the_default() {
    fmtx()
        .args(["-f", "json", "-t", "json"])
        .write_stdin(r#"{"a":1,"b":2}"#)
        .assert()
        .success()
        // Pretty output is multi-line (contains a newline before EOF newline).
        .stdout(predicate::str::contains("{\n"));
}

#[test]
fn csv_to_json_array_of_objects_via_stdin() {
    fmtx()
        .args(["-f", "csv", "-t", "json", "--compact"])
        .write_stdin("name,age\nAlice,30\nBob,25\n")
        .assert()
        .success()
        .stdout(predicate::eq(
            "[{\"name\":\"Alice\",\"age\":\"30\"},{\"name\":\"Bob\",\"age\":\"25\"}]\n",
        ));
}

#[test]
fn json_to_csv_via_stdin() {
    fmtx()
        .args(["-f", "json", "-t", "csv"])
        .write_stdin(r#"[{"name":"Alice","age":30},{"name":"Bob","age":25}]"#)
        .assert()
        .success()
        .stdout(predicate::eq("name,age\nAlice,30\nBob,25\n"));
}

// ---------------------------------------------------------------------------
// File I/O with extension inference (the `convert input -o output` shape)
// ---------------------------------------------------------------------------

#[test]
fn convert_file_infers_formats_from_extensions() {
    let dir = tmp_dir("infer");
    let input = dir.join("config.yaml");
    let output = dir.join("config.json");
    fs::write(&input, "service: web\nport: 8080\nenabled: true\n").unwrap();

    fmtx()
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .assert()
        .success();

    let written = fs::read_to_string(&output).unwrap();
    assert!(written.contains("\"service\": \"web\""), "got: {}", written);
    assert!(written.contains("\"port\": 8080"), "got: {}", written);
    assert!(written.contains("\"enabled\": true"), "got: {}", written);
}

#[test]
fn convert_without_literal_convert_word_also_works() {
    // The leading `convert` token is optional; a bare path works too.
    let dir = tmp_dir("noconvert");
    let input = dir.join("in.json");
    let output = dir.join("out.yaml");
    fs::write(&input, r#"{"k":"v","n":3}"#).unwrap();

    fmtx().arg(&input).arg("-o").arg(&output).assert().success();

    let written = fs::read_to_string(&output).unwrap();
    assert!(written.contains("k: v"), "got: {}", written);
    assert!(written.contains("n: 3"), "got: {}", written);
}

#[test]
fn input_flag_reads_file_writes_stdout() {
    let dir = tmp_dir("inputflag");
    let input = dir.join("rows.csv");
    fs::write(&input, "k,v\nfoo,1\nbar,2\n").unwrap();

    fmtx()
        .args(["-t", "json", "--compact", "-i"])
        .arg(&input)
        .assert()
        .success()
        .stdout(predicate::eq(
            "[{\"k\":\"foo\",\"v\":\"1\"},{\"k\":\"bar\",\"v\":\"2\"}]\n",
        ));
}

#[test]
fn explicit_format_overrides_extension() {
    // File is named .txt but we force --from json / --to yaml.
    let dir = tmp_dir("override");
    let input = dir.join("data.txt");
    fs::write(&input, r#"{"hello":"world"}"#).unwrap();

    fmtx()
        .arg(&input)
        .args(["-f", "json", "-t", "yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello: world"));
}

// ---------------------------------------------------------------------------
// Round-trip preservation through the CLI (yaml -> json -> yaml)
// ---------------------------------------------------------------------------

#[test]
fn cli_roundtrip_yaml_json_yaml_preserves_data() {
    let dir = tmp_dir("roundtrip");
    let y1 = dir.join("a.yaml");
    let j = dir.join("b.json");
    let y2 = dir.join("c.yaml");

    let original = "title: demo\nitems:\n  - x\n  - y\nmeta:\n  count: 2\n  ok: true\n";
    fs::write(&y1, original).unwrap();

    fmtx().arg(&y1).arg("-o").arg(&j).assert().success();
    fmtx().arg(&j).arg("-o").arg(&y2).assert().success();

    // Compare semantically by converting both YAMLs to compact JSON via the CLI.
    let json_from_y1 = fmtx()
        .arg(&y1)
        .args(["-t", "json", "--compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json_from_y2 = fmtx()
        .arg(&y2)
        .args(["-t", "json", "--compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_from_y1, json_from_y2);
}

#[test]
fn cli_roundtrip_json_toml_json_preserves_data() {
    let dir = tmp_dir("tomlrt");
    let j1 = dir.join("a.json");
    let t = dir.join("b.toml");

    let original = r#"{"title":"demo","count":7,"ratio":2.5,"on":true,"tags":["a","b"],"owner":{"name":"m","admin":false}}"#;
    fs::write(&j1, original).unwrap();

    fmtx().arg(&j1).arg("-o").arg(&t).assert().success();

    let back = fmtx()
        .arg(&t)
        .args(["-t", "json", "--compact"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let back_str = String::from_utf8(back).unwrap();

    // TOML legitimately reorders keys (bare key/value pairs must precede
    // `[table]` sections), so a byte-for-byte JSON comparison would be wrong.
    // "Preserves data" means structural equality: parse both into
    // serde_json::Value and compare order-independently.
    let original_value: serde_json::Value = serde_json::from_str(original).unwrap();
    let back_value: serde_json::Value = serde_json::from_str(back_str.trim()).unwrap();
    assert_eq!(original_value, back_value);
}

#[test]
fn cli_roundtrip_csv_json_csv_preserves_tabular_sample() {
    let dir = tmp_dir("csvrt");
    let c1 = dir.join("a.csv");
    let j = dir.join("b.json");
    let c2 = dir.join("c.csv");

    let original = "name,age,city\nAlice,30,NYC\nBob,25,LA\n";
    fs::write(&c1, original).unwrap();

    fmtx().arg(&c1).arg("-o").arg(&j).assert().success();
    fmtx().arg(&j).arg("-o").arg(&c2).assert().success();

    let final_csv = fs::read_to_string(&c2).unwrap();
    assert_eq!(final_csv, original);
}

// ---------------------------------------------------------------------------
// Error paths: clear message to stderr + non-zero exit, no panic
// ---------------------------------------------------------------------------

#[test]
fn scalar_to_csv_fails_nonzero_with_message() {
    fmtx()
        .args(["-f", "json", "-t", "csv"])
        .write_stdin("42")
        .assert()
        .failure()
        .stderr(predicate::str::contains("CSV"))
        .stderr(predicate::str::contains("scalar"));
}

#[test]
fn lone_object_to_csv_fails_nonzero() {
    fmtx()
        .args(["-f", "json", "-t", "csv"])
        .write_stdin(r#"{"a":1}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("array of objects"));
}

#[test]
fn malformed_json_fails_nonzero() {
    fmtx()
        .args(["-f", "json", "-t", "yaml"])
        .write_stdin("{ this is : not json ]")
        .assert()
        .failure()
        .stderr(predicate::str::contains("parse"));
}

#[test]
fn top_level_array_to_toml_fails_nonzero() {
    fmtx()
        .args(["-f", "json", "-t", "toml"])
        .write_stdin("[1,2,3]")
        .assert()
        .failure()
        .stderr(predicate::str::contains("TOML"))
        .stderr(predicate::str::contains("top-level array"));
}

#[test]
fn null_to_toml_fails_nonzero() {
    fmtx()
        .args(["-f", "json", "-t", "toml"])
        .write_stdin(r#"{"x": null}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("null"));
}

#[test]
fn stdin_without_from_flag_fails() {
    // No -f and no input path -> cannot infer source format.
    fmtx()
        .args(["-t", "json"])
        .write_stdin("a: 1\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("source format"));
}

#[test]
fn stdout_without_to_flag_fails() {
    // Input path has an extension but no -t and stdout target -> cannot infer.
    let dir = tmp_dir("noto");
    let input = dir.join("a.yaml");
    fs::write(&input, "a: 1\n").unwrap();

    fmtx()
        .arg(&input)
        .assert()
        .failure()
        .stderr(predicate::str::contains("target format"));
}

#[test]
fn unknown_format_flag_fails() {
    fmtx()
        .args(["-f", "xml", "-t", "json"])
        .write_stdin("<a/>")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown format"));
}

#[test]
fn nonexistent_input_file_fails() {
    fmtx()
        .args(["-t", "json"])
        .arg("/nonexistent/path/to/file.yaml")
        .assert()
        .failure()
        .stderr(predicate::str::contains("read"));
}

#[test]
fn nested_value_to_csv_fails_nonzero() {
    fmtx()
        .args(["-f", "json", "-t", "csv"])
        .write_stdin(r#"[{"a": {"deep": 1}}]"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("nested"));
}

#[test]
fn array_of_scalars_to_csv_fails_nonzero() {
    fmtx()
        .args(["-f", "json", "-t", "csv"])
        .write_stdin("[1,2,3]")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not an object"));
}

// ---------------------------------------------------------------------------
// --help / --version smoke tests
// ---------------------------------------------------------------------------

#[test]
fn help_shows_usage_and_examples() {
    fmtx()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Convert between YAML, JSON, TOML, and CSV",
        ))
        .stdout(predicate::str::contains("EXAMPLES"));
}

#[test]
fn version_prints() {
    fmtx()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("fmtx"));
}
