# fmtx

A small, fast, dependency-light **data-format interconverter**. Convert between
**YAML**, **JSON**, **TOML**, and **CSV** from the command line — file-to-file
or as a Unix filter (stdin → stdout). No network, no API, no runtime services.

Every conversion funnels through a single universal intermediate
representation (`serde_json::Value`), so any source format can target any other
(subject to the structural constraints each format imposes — see
[the CSV / tabular caveat](#csv--tabular-caveat) and
[TOML constraints](#toml-constraints)).

## Install

```sh
cargo build --release
# binary at target/release/fmtx
```

## Usage

```
fmtx [convert] [INPUT] [OPTIONS]
```

Formats are **inferred from file extensions** when you pass an input path
and/or an `-o <file>` output path. Use `-f/--from` and `-t/--to` to set them
explicitly — required when reading stdin or writing stdout (no extension to
infer from), and useful to override a misleading extension.

### Flags

| Flag | Meaning | Default |
|------|---------|---------|
| `-f`, `--from <FORMAT>` | Source format: `json` \| `yaml` \| `toml` \| `csv` | inferred from input extension |
| `-t`, `--to <FORMAT>`   | Target format: `json` \| `yaml` \| `toml` \| `csv` | inferred from output extension |
| `-i`, `--input <FILE>`  | Input file | stdin |
| `-o`, `--output <FILE>` | Output file | stdout |
| `--pretty`              | Pretty-print (multi-line, indented) | **on (default)** |
| `--compact`             | Compact output (single line where the format allows) | off |

`yml` is accepted as an alias for `yaml`. Format names are case-insensitive.
The leading literal `convert` word is optional: `fmtx convert a.yaml -o a.json`
and `fmtx a.yaml -o a.json` are equivalent.

## Examples

Convert a file, inferring both formats from the extensions:

```sh
fmtx convert config.yaml -o config.json
fmtx config.yaml -o config.json          # 'convert' is optional
fmtx data.json -o data.toml              # JSON object -> TOML
```

Use it as a filter (stdin → stdout) with explicit formats:

```sh
cat in.yaml | fmtx -f yaml -t json       # YAML -> JSON on stdout
fmtx -f csv -t json -i rows.csv          # read a file, write JSON to stdout
fmtx -f json -t yaml --compact < in.json # compact where applicable
```

Override a misleading extension:

```sh
fmtx data.txt -f json -t yaml            # treat data.txt as JSON
```

### What conversions look like

YAML → JSON:

```sh
$ printf 'name: fmtx\nversion: 1\ntags:\n  - cli\n  - rust\n' | fmtx -f yaml -t json
{
  "name": "fmtx",
  "version": 1,
  "tags": [
    "cli",
    "rust"
  ]
}
```

JSON → TOML (note: TOML requires a table at the top level):

```sh
$ printf '{"title":"demo","count":7,"on":true}' | fmtx -f json -t toml
title = "demo"
count = 7
on = true
```

CSV → JSON (a CSV becomes an **array of objects**; the header row supplies the
keys, and every value is a string because CSV carries no type information):

```sh
$ printf 'name,age,city\nAlice,30,NYC\nBob,25,LA\n' | fmtx -f csv -t json --compact
[{"name":"Alice","age":"30","city":"NYC"},{"name":"Bob","age":"25","city":"LA"}]
```

JSON (array of objects) → CSV (the reverse):

```sh
$ printf '[{"name":"Alice","age":30},{"name":"Bob","age":25}]' | fmtx -f json -t csv
name,age
Alice,30
Bob,25
```

## CSV / tabular caveat

CSV is **not a tree format** — it is a flat table of rows and columns. `fmtx`
models a CSV file as a JSON **array of objects**:

- **CSV → anything**: the first row is the header and supplies the object keys;
  each subsequent row becomes one object. **All values are strings** — CSV has
  no notion of numbers, booleans, or null, so `age` of `30` round-trips as the
  string `"30"`, not the number `30`.
- **anything → CSV**: the input's intermediate representation **must be an
  array of objects** (a table). The union of all object keys, in first-seen
  order, becomes the header; a key missing from a given row produces an empty
  cell; scalar cell values (string/number/bool) are stringified, and `null`
  becomes an empty cell.

Converting **non-tabular data to CSV is an error** with a clear message and a
non-zero exit code. Specifically, the following cannot be represented as CSV:

- a top-level **scalar** (e.g. `42`, `"hi"`, `true`) — *"the data is a scalar
  value, not a table"*;
- a top-level **single object** (e.g. `{"a": 1}`) — *"the data is a single
  object, not a table … wrap your rows in a JSON array"*;
- an **array whose elements aren't all objects** (e.g. `[1, 2, 3]`) —
  *"array element N is not an object"*;
- a row containing a **nested array or object** value (e.g.
  `[{"a": {"deep": 1}}]`) — *"CSV cells must be scalars … nested arrays and
  objects cannot be represented"*.

An empty array (`[]`) produces an empty CSV document.

## TOML constraints

TOML imposes two structural rules that `fmtx` enforces with clear errors:

- The **top level must be a table** (a key/value map / JSON object). A
  top-level array or scalar is rejected — wrap it, e.g.
  `{ "items": [ ... ] }`.
- TOML has **no null type**. Any `null` anywhere in the data is rejected, with
  a dotted path to the offending value (e.g. `a.b`) so you can find and remove
  it.

Also note that TOML requires bare key/value pairs to precede any `[table]`
sections, so a JSON object whose keys mix scalars and nested objects will have
its **key order rearranged** on output (scalars first, then tables). The
**data is preserved** — only the serialized key order changes.

## Behavior & exit codes

- On success, `fmtx` exits `0`. Output is written to the file given by `-o`, or
  to stdout. Output is normalized to end in exactly one trailing newline (an
  empty document stays empty).
- On any error — unparseable input, an impossible conversion, a missing input
  file, an unknown format, or a missing format that couldn't be inferred —
  `fmtx` prints a clear message to **stderr** (never panics) and exits with a
  **non-zero** status.

## How it works

`serde_json::Value` is the universal intermediate representation. Each format
has a parser (`<fmt> → Value`) and a serializer (`Value → <fmt>`):

- **JSON** ↔ `Value`: native via `serde_json` (`preserve_order` keeps object
  key insertion order).
- **YAML** ↔ `Value`: via `serde_yaml`, deserializing straight into / out of
  `Value`.
- **TOML** ↔ `Value`: via `toml`, with the table-at-top-level and no-null
  checks above.
- **CSV** ↔ `Value`: custom array-of-objects mapping via the `csv` crate.

## Testing

```sh
cargo test
```

The suite covers the conversion core with unit tests (round-trip preservation
for every format pair, plus error paths) and the CLI end-to-end with
`assert_cmd` integration tests (stdin/stdout, file I/O, extension inference,
and non-zero exit on bad input).

## License

MIT — see [LICENSE](LICENSE).
