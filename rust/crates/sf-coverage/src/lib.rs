//! Coverage readers for stud-finder: SimpleCov resultset.json, LCOV
//! `.info`, and Cobertura XML. One public entry point [`load`] picks
//! the format by extension (matching Ruby `Coverage::Detector::PARSERS`)
//! and returns per-file line-rate in `[0.0, 1.0]`.
//!
//! Parity targets (bazfer/stud-finder, main branch, 2026-11-12):
//!
//! - `lib/stud_finder/coverage/resultset.rb`: supports both
//!   `{coverage: {filename: ...}}` flat and `{suite: {coverage: ...}}`
//!   nested shapes. Line entries may be a hash `{"lines": [...]}` or
//!   just the array. Multiple suites merge via per-line max (nil
//!   stays nil only if BOTH entries are nil). Rate is
//!   `positive_count / non_nil_count`; all-nil files score 0.0.
//! - `lib/stud_finder/coverage/lcov.rb`: records split by
//!   `end_of_record`, `SF:` names the file, `LF:`/`LH:` give
//!   found/hit directly. Fall back to parsing `DA:<line>,<count>`.
//!   Exceeding LH > LF is a hard error.
//! - `lib/stud_finder/coverage/cobertura.rb`: XPath
//!   `/coverage/packages/package/classes/class` with `line-rate`
//!   already in `[0.0, 1.0]`. Rate out of range is a hard error.
//!
//! Path normalization (matching resultset.rb + lcov.rb):
//! 1. Try stripping `project_root/` prefix from the reported path.
//!    If the stripped result is in the caller's `files` set, use it.
//! 2. If the path starts with `/`, try suffix-matching: longest
//!    trailing path components that appear in `files`.
//! 3. Otherwise strip leading `./`.
//!
//! Cobertura is simpler: just strip `./`. Matches Ruby.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Per-file line-rate in `[0.0, 1.0]` keyed by caller-relative path.
/// `missing_files` lists requested files that didn't appear in the
/// report. `warnings` carries non-fatal observations (e.g. a reported
/// path that couldn't be matched to any requested file and fell
/// through to its `./`-stripped form).
#[derive(Debug, Default, Clone)]
pub struct CoverageReport {
    pub coverage_pct: HashMap<PathBuf, f64>,
    pub missing_files: Vec<PathBuf>,
    pub warnings: Vec<CoverageWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageWarning {
    /// A path was present in the coverage report but we could not map
    /// it to any requested file in `files` — it still lands in the
    /// report under its best-effort key, but the CLI should know.
    UnmatchedReportedPath(String),
}

#[derive(Debug)]
pub enum CoverageError {
    UnsupportedExtension(String),
    FileNotFound(PathBuf),
    MalformedJson(String),
    MalformedXml(String),
    MissingLineRate(String),
    InvalidLineRate(String),
    LineRateOutOfRange(String),
    LineHitsExceedLinesFound(String),
    Io(std::io::Error),
}

impl std::fmt::Display for CoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedExtension(p) => {
                write!(f, "unsupported coverage file type: {p}")
            }
            Self::FileNotFound(p) => write!(f, "coverage file not found: {}", p.display()),
            Self::MalformedJson(m) => write!(f, "malformed coverage JSON: {m}"),
            Self::MalformedXml(m) => write!(f, "malformed coverage XML: {m}"),
            Self::MissingLineRate(file) => write!(f, "missing line-rate for coverage file: {file}"),
            Self::InvalidLineRate(file) => write!(f, "invalid line-rate for coverage file: {file}"),
            Self::LineRateOutOfRange(file) => {
                write!(f, "line-rate out of range for coverage file: {file}")
            }
            Self::LineHitsExceedLinesFound(file) => {
                write!(f, "line hits exceed lines found for coverage file: {file}")
            }
            Self::Io(err) => write!(f, "I/O error reading coverage file: {err}"),
        }
    }
}

impl std::error::Error for CoverageError {}

impl From<std::io::Error> for CoverageError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::NotFound {
            Self::FileNotFound(PathBuf::new())
        } else {
            Self::Io(err)
        }
    }
}

/// Load a coverage report from `path`, treating `root` as the project
/// root for path normalization and `files` as the caller's set of
/// recognized files (used for suffix matching). Format is picked by
/// extension: `.json` → SimpleCov, `.info` → LCOV, `.xml` → Cobertura.
pub fn load(path: &Path, root: &Path, files: &[&Path]) -> Result<CoverageReport, CoverageError> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoverageError::FileNotFound(path.to_path_buf()));
        }
        Err(err) => return Err(CoverageError::Io(err)),
    };
    let file_set: HashSet<PathBuf> = files.iter().map(|p| p.to_path_buf()).collect();
    let ctx = NormalizeCtx {
        root: root.to_path_buf(),
        file_set: &file_set,
    };
    let (coverage_pct, warnings) = match ext.as_deref() {
        Some("json") => parse_resultset(&source, &ctx)?,
        Some("info") => parse_lcov(&source, &ctx)?,
        Some("xml") => parse_cobertura(&source, &ctx)?,
        _ => {
            return Err(CoverageError::UnsupportedExtension(
                path.display().to_string(),
            ));
        }
    };
    // Filter output to the caller's requested files — matches Ruby's
    // `reported.slice(*@files)` semantic. Without this, unmatched
    // report entries would still land in `coverage_pct` and could
    // activate downstream availability gates from unrelated files.
    // Fixes Ohm's P2 on PR #80.
    let file_set: HashSet<PathBuf> = files.iter().map(|p| p.to_path_buf()).collect();
    let coverage_pct: HashMap<PathBuf, f64> = coverage_pct
        .into_iter()
        .filter(|(k, _)| file_set.contains(k))
        .collect();
    let missing_files: Vec<PathBuf> = files
        .iter()
        .filter_map(|p| {
            let pb = p.to_path_buf();
            if coverage_pct.contains_key(&pb) {
                None
            } else {
                Some(pb)
            }
        })
        .collect();
    Ok(CoverageReport {
        coverage_pct,
        missing_files,
        warnings,
    })
}

struct NormalizeCtx<'a> {
    root: PathBuf,
    file_set: &'a HashSet<PathBuf>,
}

impl NormalizeCtx<'_> {
    fn normalize(&self, reported: &str) -> (PathBuf, Option<CoverageWarning>) {
        // 1. Try stripping project_root/ prefix.
        let stripped = self.strip_root(reported);
        if let Some(s) = stripped.as_ref() {
            let pb = PathBuf::from(s);
            if self.file_set.contains(&pb) {
                return (pb, None);
            }
        }

        if reported.starts_with('/') {
            // 2. Suffix match for absolute paths.
            if let Some(suffix) = self.suffix_match(reported) {
                return (PathBuf::from(suffix), None);
            }
            // 3. Fall through to stripped or ./-stripped reported.
            let key = stripped
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| reported.trim_start_matches("./").to_string());
            let warning = if !self.file_set.contains(&PathBuf::from(&key)) {
                Some(CoverageWarning::UnmatchedReportedPath(reported.to_string()))
            } else {
                None
            };
            (PathBuf::from(key), warning)
        } else {
            let key = stripped
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| reported.trim_start_matches("./").to_string());
            (PathBuf::from(key), None)
        }
    }

    fn strip_root(&self, reported: &str) -> Option<String> {
        let root_str = self.root.to_string_lossy();
        let with_slash = format!("{root_str}/");
        reported.strip_prefix(&with_slash).map(str::to_string)
    }

    fn suffix_match(&self, reported: &str) -> Option<String> {
        let parts: Vec<&str> = reported.split('/').filter(|p| !p.is_empty()).collect();
        for count in (1..=parts.len()).rev() {
            let suffix = parts[parts.len() - count..].join("/");
            if self.file_set.contains(&PathBuf::from(&suffix)) {
                return Some(suffix);
            }
        }
        None
    }
}

// --- SimpleCov resultset.json ---

fn parse_resultset(
    source: &str,
    ctx: &NormalizeCtx<'_>,
) -> Result<(HashMap<PathBuf, f64>, Vec<CoverageWarning>), CoverageError> {
    let data: serde_json::Value =
        serde_json::from_str(source).map_err(|e| CoverageError::MalformedJson(e.to_string()))?;

    let coverage_payloads: Vec<&serde_json::Map<String, serde_json::Value>> =
        match data.get("coverage").and_then(|v| v.as_object()) {
            Some(flat) => vec![flat],
            None => data
                .as_object()
                .map(|root_obj| {
                    root_obj
                        .values()
                        .filter_map(|suite| suite.get("coverage").and_then(|c| c.as_object()))
                        .collect()
                })
                .unwrap_or_default(),
        };

    // Normalize filenames INSIDE the merge loop so aliased reports
    // (e.g. `/abs/lib/a.rb` from one suite + `lib/a.rb` from another)
    // merge per-line on the same normalized key. Matches Ruby
    // `Resultset#parse_report` which keys `merged` by normalize_filename.
    // Fixes Ohm's P1 on PR #80 (alias collision / nondeterministic
    // overwrite). nil stays nil only if BOTH entries are nil.
    let mut merged: HashMap<PathBuf, Vec<Option<i64>>> = HashMap::new();
    let mut warnings: Vec<CoverageWarning> = Vec::new();
    for coverage in &coverage_payloads {
        for (filename, details) in coverage.iter() {
            let Some(new_lines) = extract_lines(details) else {
                continue;
            };
            let (key, warning) = ctx.normalize(filename);
            if let Some(w) = warning {
                warnings.push(w);
            }
            merged
                .entry(key)
                .and_modify(|previous| merge_lines(previous, &new_lines))
                .or_insert(new_lines);
        }
    }

    let coverage_pct: HashMap<PathBuf, f64> = merged
        .iter()
        .map(|(key, lines)| (key.clone(), line_rate(lines)))
        .collect();
    Ok((coverage_pct, warnings))
}

fn extract_lines(details: &serde_json::Value) -> Option<Vec<Option<i64>>> {
    let array = details
        .as_array()
        .cloned()
        .or_else(|| details.get("lines").and_then(|v| v.as_array()).cloned())?;
    Some(
        array
            .into_iter()
            .map(|v| match v {
                serde_json::Value::Null => None,
                serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
                _ => None,
            })
            .collect(),
    )
}

fn merge_lines(previous: &mut Vec<Option<i64>>, new: &[Option<i64>]) {
    let len = previous.len().max(new.len());
    previous.resize(len, None);
    for (i, slot) in previous.iter_mut().enumerate() {
        let a = *slot;
        let b = new.get(i).copied().unwrap_or(None);
        *slot = match (a, b) {
            (None, None) => None,
            (Some(x), None) => Some(x),
            (None, Some(y)) => Some(y),
            (Some(x), Some(y)) => Some(x.max(y)),
        };
    }
}

fn line_rate(lines: &[Option<i64>]) -> f64 {
    let executable: Vec<i64> = lines.iter().filter_map(|v| *v).collect();
    if executable.is_empty() {
        return 0.0;
    }
    let hit = executable.iter().filter(|v| **v > 0).count();
    hit as f64 / executable.len() as f64
}

// --- LCOV ---

fn parse_lcov(
    source: &str,
    ctx: &NormalizeCtx<'_>,
) -> Result<(HashMap<PathBuf, f64>, Vec<CoverageWarning>), CoverageError> {
    let mut coverage_pct: HashMap<PathBuf, f64> = HashMap::new();
    let mut warnings: Vec<CoverageWarning> = Vec::new();
    // Split on `end_of_record` on its own line (any surrounding whitespace).
    // Simpler: split on lines, accumulate until we see `end_of_record`.
    let mut current: Vec<&str> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_end();
        if trimmed == "end_of_record" {
            process_lcov_record(&current, ctx, &mut coverage_pct, &mut warnings)?;
            current.clear();
        } else {
            current.push(line);
        }
    }
    // Any trailing record without end_of_record still gets processed.
    if !current.is_empty() {
        process_lcov_record(&current, ctx, &mut coverage_pct, &mut warnings)?;
    }
    Ok((coverage_pct, warnings))
}

fn process_lcov_record(
    lines: &[&str],
    ctx: &NormalizeCtx<'_>,
    out: &mut HashMap<PathBuf, f64>,
    warnings: &mut Vec<CoverageWarning>,
) -> Result<(), CoverageError> {
    let mut filename: Option<&str> = None;
    let mut lf: Option<usize> = None;
    let mut lh: Option<usize> = None;
    let mut da_hits: Vec<u64> = Vec::new();

    for line in lines {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("SF:") {
            filename = Some(rest);
        } else if let Some(rest) = line.strip_prefix("LF:") {
            lf = rest.parse::<usize>().ok();
        } else if let Some(rest) = line.strip_prefix("LH:") {
            lh = rest.parse::<usize>().ok();
        } else if let Some(rest) = line.strip_prefix("DA:") {
            // `DA:<line>,<count>[,<checksum>]` — accept 2- or 3-field
            // form. The old `split_once(',')` version left `count` as
            // `"<count>,<checksum>"` and failed `u64::parse`, silently
            // dropping any record that carried a checksum. Fixes
            // Ohm's P2 on PR #80. Validate `line` too.
            let mut parts = rest.splitn(3, ',');
            let line_no = parts.next();
            let count = parts.next();
            if let (Some(l), Some(c)) = (line_no, count) {
                if l.parse::<u64>().is_ok() {
                    if let Ok(cnt) = c.parse::<u64>() {
                        da_hits.push(cnt);
                    }
                }
            }
        }
    }

    let Some(filename) = filename else {
        return Ok(());
    };
    if filename.is_empty() {
        return Ok(());
    }

    let (found, hit) = match (lf, lh) {
        (Some(f), Some(h)) => (f, h),
        _ => {
            let f = da_hits.len();
            let h = da_hits.iter().filter(|c| **c > 0).count();
            (f, h)
        }
    };

    if hit > found {
        return Err(CoverageError::LineHitsExceedLinesFound(
            filename.to_string(),
        ));
    }
    let rate = if found == 0 {
        0.0
    } else {
        hit as f64 / found as f64
    };
    let (key, warning) = ctx.normalize(filename);
    if let Some(w) = warning {
        warnings.push(w);
    }
    out.insert(key, rate);
    Ok(())
}

// --- Cobertura XML ---

fn parse_cobertura(
    source: &str,
    _ctx: &NormalizeCtx<'_>,
) -> Result<(HashMap<PathBuf, f64>, Vec<CoverageWarning>), CoverageError> {
    let doc = roxmltree::Document::parse(source)
        .map_err(|e| CoverageError::MalformedXml(e.to_string()))?;
    let mut coverage_pct: HashMap<PathBuf, f64> = HashMap::new();
    let warnings: Vec<CoverageWarning> = Vec::new();

    for class in doc
        .root_element()
        .descendants()
        .filter(|n| n.has_tag_name("class"))
    {
        let filename = match class.attribute("filename") {
            Some(f) if !f.is_empty() => f,
            _ => continue,
        };
        let raw = class
            .attribute("line-rate")
            .ok_or_else(|| CoverageError::MissingLineRate(filename.to_string()))?;
        if raw.is_empty() {
            return Err(CoverageError::MissingLineRate(filename.to_string()));
        }
        let rate: f64 = raw
            .parse()
            .map_err(|_| CoverageError::InvalidLineRate(filename.to_string()))?;
        if !(0.0..=1.0).contains(&rate) {
            return Err(CoverageError::LineRateOutOfRange(filename.to_string()));
        }
        // Cobertura: just strip leading `./` per Ruby.
        let key = filename.strip_prefix("./").unwrap_or(filename).to_string();
        coverage_pct.insert(PathBuf::from(key), rate);
    }
    Ok((coverage_pct, warnings))
}

#[cfg(test)]
mod tests;
