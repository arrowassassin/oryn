//! Parse `llvm-cov export --format=text` JSON into per-file executed line sets.
//!
//! Schema (verified against llvm-cov export 3.1.0, rustc 1.96): each
//! `data[].functions[]` has `filenames` and `regions`, where a region is the
//! 8-tuple `[line_start, col_start, line_end, col_end, execution_count,
//! file_id, expanded_file_id, kind]`. `kind == 0` is a real Code region; a line
//! is "executed" by this run if some Code region with `execution_count > 0`
//! covers it. Generic instantiations union naturally (a line is covered if any
//! instantiation executed it).

use crate::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const KIND_CODE: u64 = 0;

/// Map of source filename → set of executed (1-based) line numbers.
pub type Executed = BTreeMap<String, BTreeSet<usize>>;

/// Parse an `llvm-cov export` JSON document into executed lines per file.
///
/// # Errors
/// Returns an error if the bytes are not valid JSON.
pub fn parse_export(json: &[u8]) -> Result<Executed> {
    let v: Value = serde_json::from_slice(json)?;
    let mut out: Executed = BTreeMap::new();

    for data in arr(&v["data"]) {
        for func in arr(&data["functions"]) {
            let filenames: Vec<&str> = arr(&func["filenames"])
                .iter()
                .map(|f| f.as_str().unwrap_or_default())
                .collect();
            for region in arr(&func["regions"]) {
                let Some(r) = region.as_array() else { continue };
                if r.len() < 8 {
                    continue;
                }
                let (Some(l0), Some(l1), Some(count), Some(file_id), Some(kind)) = (
                    r[0].as_u64(),
                    r[2].as_u64(),
                    r[4].as_u64(),
                    r[5].as_u64(),
                    r[7].as_u64(),
                ) else {
                    continue;
                };
                if kind != KIND_CODE || count == 0 {
                    continue;
                }
                if let Some(name) = filenames.get(file_id as usize) {
                    let set = out.entry((*name).to_string()).or_default();
                    for line in l0..=l1 {
                        set.insert(line as usize);
                    }
                }
            }
        }
    }
    Ok(out)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"{
      "version": "3.1.0",
      "type": "llvm.coverage.json.export",
      "data": [{
        "functions": [{
          "name": "_RNvCs_3lib3add",
          "count": 1,
          "filenames": ["/repo/src/lib.rs"],
          "regions": [
            [3, 1, 4, 20, 1, 0, 0, 0],
            [6, 1, 6, 10, 0, 0, 0, 0],
            [8, 1, 8, 5, 7, 0, 0, 4]
          ]
        }],
        "totals": {}
      }]
    }"#;

    #[test]
    fn extracts_executed_code_lines() {
        let cov = parse_export(SAMPLE).unwrap();
        let lines = &cov["/repo/src/lib.rs"];
        // lines 3,4 executed (count 1, kind Code); line 6 not (count 0);
        // line 8 ignored (kind 4 = Branch, not Code).
        assert!(lines.contains(&3) && lines.contains(&4));
        assert!(!lines.contains(&6));
        assert!(!lines.contains(&8));
    }

    #[test]
    fn empty_export() {
        let cov = parse_export(br#"{"data":[]}"#).unwrap();
        assert!(cov.is_empty());
    }
}
