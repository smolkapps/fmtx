//! Format enum: the set of data formats `fmtx` can convert between, plus
//! helpers to parse a format from a `--from`/`--to` flag string or infer one
//! from a file-extension.

use anyhow::{bail, Result};
use std::fmt;
use std::path::Path;

/// A supported data format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Yaml,
    Toml,
    Csv,
}

impl Format {
    /// Parse a format from an explicit flag value (case-insensitive).
    /// Accepts the canonical names plus the common `yml` alias for YAML.
    pub fn parse(s: &str) -> Result<Format> {
        match s.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Format::Json),
            "yaml" | "yml" => Ok(Format::Yaml),
            "toml" => Ok(Format::Toml),
            "csv" => Ok(Format::Csv),
            other => bail!(
                "unknown format '{}' (expected one of: json, yaml, toml, csv)",
                other
            ),
        }
    }

    /// Infer a format from a file path's extension. Returns `None` when the
    /// path has no extension or an extension we don't recognize.
    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "json" => Some(Format::Json),
            "yaml" | "yml" => Some(Format::Yaml),
            "toml" => Some(Format::Toml),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }

    /// The canonical lowercase name of this format.
    pub fn name(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Toml => "toml",
            Format::Csv => "csv",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_canonical_names() {
        assert_eq!(Format::parse("json").unwrap(), Format::Json);
        assert_eq!(Format::parse("yaml").unwrap(), Format::Yaml);
        assert_eq!(Format::parse("toml").unwrap(), Format::Toml);
        assert_eq!(Format::parse("csv").unwrap(), Format::Csv);
    }

    #[test]
    fn parse_is_case_insensitive_and_trims() {
        assert_eq!(Format::parse("JSON").unwrap(), Format::Json);
        assert_eq!(Format::parse("  Yaml  ").unwrap(), Format::Yaml);
    }

    #[test]
    fn parse_yml_alias() {
        assert_eq!(Format::parse("yml").unwrap(), Format::Yaml);
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(Format::parse("xml").is_err());
    }

    #[test]
    fn infer_from_extension() {
        assert_eq!(Format::from_path(Path::new("a.json")), Some(Format::Json));
        assert_eq!(Format::from_path(Path::new("a.yaml")), Some(Format::Yaml));
        assert_eq!(Format::from_path(Path::new("a.yml")), Some(Format::Yaml));
        assert_eq!(Format::from_path(Path::new("a.toml")), Some(Format::Toml));
        assert_eq!(Format::from_path(Path::new("a.csv")), Some(Format::Csv));
        assert_eq!(Format::from_path(Path::new("a.txt")), None);
        assert_eq!(Format::from_path(Path::new("noext")), None);
    }
}
