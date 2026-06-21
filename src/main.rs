//! `fmtx` — a data-format interconverter for YAML, JSON, TOML, and CSV.
//!
//! Usage shapes:
//!   fmtx convert input.yaml -o output.json   # infer formats from extensions
//!   cat data.yaml | fmtx -f yaml -t json     # explicit formats over stdin/stdout
//!
//! See `README.md` for the full reference and the CSV/tabular caveat.

mod convert;
mod format;

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::Parser;

use crate::format::Format;

/// A data-format interconverter (YAML <-> JSON <-> TOML <-> CSV).
///
/// Formats are inferred from file extensions when you pass a positional input
/// path and/or `-o <file>`; use `-f/--from` and `-t/--to` to set them
/// explicitly (required when reading stdin or writing stdout, which have no
/// extension to infer from).
#[derive(Parser, Debug)]
#[command(
    name = "fmtx",
    version,
    about = "Convert between YAML, JSON, TOML, and CSV.",
    long_about = None,
    after_help = "EXAMPLES:\n  \
        fmtx convert config.yaml -o config.json     convert a file, formats inferred\n  \
        fmtx convert data.json -o data.toml          JSON object -> TOML\n  \
        cat in.yaml | fmtx -f yaml -t json           stdin -> stdout, explicit formats\n  \
        fmtx -f csv -t json -i rows.csv              read a file, write stdout\n  \
        fmtx convert in.json -o out.csv              array-of-objects -> CSV\n\n\
        NOTE: CSV maps to a JSON array of objects (header row = keys). Converting\n  \
        non-tabular data (a scalar, a lone object, or rows with nested values) to\n  \
        CSV is an error."
)]
struct Cli {
    /// Positional arguments: an optional leading literal `convert` verb,
    /// followed by an optional input path. So both `fmtx convert in.yaml ...`
    /// and `fmtx in.yaml ...` are accepted. When a path is given, the source
    /// format is inferred from its extension (unless `-f/--from` overrides it).
    #[arg(value_name = "[convert] [INPUT]", num_args = 0..=2)]
    positionals: Vec<String>,

    /// Source format (json|yaml|toml|csv). Overrides extension inference.
    /// Required when reading from stdin.
    #[arg(short = 'f', long = "from", value_name = "FORMAT")]
    from: Option<String>,

    /// Target format (json|yaml|toml|csv). Overrides extension inference.
    /// Required when writing to stdout.
    #[arg(short = 't', long = "to", value_name = "FORMAT")]
    to: Option<String>,

    /// Input file (alternative to the positional path). Defaults to stdin.
    #[arg(short = 'i', long = "input", value_name = "FILE")]
    input: Option<PathBuf>,

    /// Output file. Defaults to stdout.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Pretty-print output (multi-line, indented). This is the default.
    #[arg(long = "pretty", conflicts_with = "compact")]
    pretty: bool,

    /// Compact output (single line where the format allows).
    #[arg(long = "compact")]
    compact: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Print the full anyhow chain to stderr, no panic, non-zero exit.
            eprintln!("fmtx: error: {:#}", e);
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    // Interpret the positionals: an optional leading literal `convert` verb,
    // then an optional input path. Drop one leading `convert` token if present,
    // and whatever remains (at most one token) is the input path.
    let mut tokens: Vec<&String> = cli.positionals.iter().collect();
    if tokens.first().map(|s| s.as_str()) == Some("convert") {
        tokens.remove(0);
    }
    if tokens.len() > 1 {
        bail!(
            "too many positional arguments: expected at most one input path \
             (got {}). Pass extra options with flags.",
            tokens.len()
        );
    }
    let positional_path: Option<PathBuf> = tokens.first().map(|s| PathBuf::from(s.as_str()));

    if positional_path.is_some() && cli.input.is_some() {
        bail!("specify the input either positionally or with -i/--input, not both");
    }
    let input_path: Option<PathBuf> = positional_path.or_else(|| cli.input.clone());

    // Resolve formats: explicit flag wins, else infer from the relevant path.
    let from = resolve_from(&cli, input_path.as_deref())?;
    let to = resolve_to(&cli, cli.output.as_deref())?;

    // pretty is the default; --compact flips it off.
    let pretty = !cli.compact;

    // Read input.
    let input_text = read_input(input_path.as_deref()).context("failed to read input")?;

    // Convert.
    let output_text = convert::convert(&input_text, from, to, pretty)?;

    // Write output.
    write_output(cli.output.as_deref(), &output_text, to).context("failed to write output")?;

    Ok(())
}

/// Determine the source format: `-f/--from` if present, else infer from the
/// input path's extension. Errors with actionable guidance if neither works.
fn resolve_from(cli: &Cli, input_path: Option<&std::path::Path>) -> Result<Format> {
    if let Some(f) = &cli.from {
        return Format::parse(f);
    }
    if let Some(p) = input_path {
        if let Some(fmt) = Format::from_path(p) {
            return Ok(fmt);
        }
        bail!(
            "could not infer the source format from '{}': unrecognized or missing \
             extension. Pass -f/--from <json|yaml|toml|csv>.",
            p.display()
        );
    }
    bail!(
        "reading from stdin requires an explicit source format: pass \
         -f/--from <json|yaml|toml|csv>."
    );
}

/// Determine the target format: `-t/--to` if present, else infer from the
/// output path's extension. Errors with actionable guidance if neither works.
fn resolve_to(cli: &Cli, output_path: Option<&std::path::Path>) -> Result<Format> {
    if let Some(t) = &cli.to {
        return Format::parse(t);
    }
    if let Some(p) = output_path {
        if let Some(fmt) = Format::from_path(p) {
            return Ok(fmt);
        }
        bail!(
            "could not infer the target format from '{}': unrecognized or missing \
             extension. Pass -t/--to <json|yaml|toml|csv>.",
            p.display()
        );
    }
    bail!(
        "writing to stdout requires an explicit target format: pass \
         -t/--to <json|yaml|toml|csv>."
    );
}

/// Read the full input from a file path, or from stdin when `path` is `None`.
fn read_input(path: Option<&std::path::Path>) -> Result<String> {
    match path {
        Some(p) => {
            fs::read_to_string(p).with_context(|| format!("could not read file '{}'", p.display()))
        }
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .context("could not read from stdin")?;
            Ok(buf)
        }
    }
}

/// Write `text` to a file path, or to stdout when `path` is `None`.
///
/// We normalize to exactly one trailing newline so files and piped output are
/// well-formed. CSV already ends in a newline; JSON/TOML/YAML get one added if
/// missing. An empty document (e.g. CSV of an empty array) stays empty.
fn write_output(path: Option<&std::path::Path>, text: &str, _to: Format) -> Result<()> {
    let mut body = text.to_string();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    match path {
        Some(p) => fs::write(p, body.as_bytes())
            .with_context(|| format!("could not write file '{}'", p.display())),
        None => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            handle
                .write_all(body.as_bytes())
                .context("could not write to stdout")?;
            handle.flush().context("could not flush stdout")?;
            Ok(())
        }
    }
}
