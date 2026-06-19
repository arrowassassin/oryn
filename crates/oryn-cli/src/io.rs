//! Small JSON/JSONL loaders shared by the subcommands.

use anyhow::{Context, Result};
use oryn_core::contam::Document;
use oryn_core::eval::{EvalItem, EvalRun};
use std::path::Path;

/// Read a file as either JSONL (one object per line) or a JSON array.
fn read_records<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let trimmed = raw.trim_start();
    if trimmed.starts_with('[') {
        Ok(serde_json::from_str(&raw)
            .with_context(|| format!("parsing JSON array {}", path.display()))?)
    } else {
        let mut out = Vec::new();
        for (n, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let rec: T = serde_json::from_str(line)
                .with_context(|| format!("{}:{}: invalid JSON", path.display(), n + 1))?;
            out.push(rec);
        }
        Ok(out)
    }
}

/// Load a corpus / eval document set (`{"id","text"}` records).
pub fn load_documents(path: &Path) -> Result<Vec<Document>> {
    read_records(path)
}

/// Load an eval run (`{"id","score"}` records). The run name defaults to the
/// file stem unless overridden.
pub fn load_run(path: &Path, name: Option<&str>) -> Result<EvalRun> {
    let items: Vec<EvalItem> = read_records(path)?;
    let name = name.map(str::to_string).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("run")
            .to_string()
    });
    Ok(EvalRun::new(name, items))
}

/// Read a JSON array of strings (repeated generations for determinism analysis).
pub fn load_strings(path: &Path) -> Result<Vec<String>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let trimmed = raw.trim_start();
    if trimmed.starts_with('[') {
        Ok(serde_json::from_str(&raw)?)
    } else {
        // One generation per line.
        Ok(raw.lines().map(str::to_string).collect())
    }
}

/// Write `value` as pretty JSON to `out` (or stdout when `None`).
pub fn write_json<T: serde::Serialize>(value: &T, out: Option<&Path>) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    match out {
        Some(p) => {
            std::fs::write(p, json.as_bytes())
                .with_context(|| format!("writing {}", p.display()))?;
            eprintln!("wrote {}", p.display());
        }
        None => println!("{json}"),
    }
    Ok(())
}
