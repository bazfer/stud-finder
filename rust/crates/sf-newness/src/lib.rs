//! Per-file newness signals for stud-finder.
//!
//! Ports Ruby `lib/stud_finder/newness.rb`. For every file in the
//! scored set, computes:
//!
//! - `age_days` — whole days since the file was first committed (by
//!   COMMITTER time, not author time, so cherry-picks or imports
//!   read as new in this repo). `0` when the file has no history.
//! - `total_commits` — non-merge commits touching this file after
//!   rename canonicalization and deletion-lineage boundaries.
//! - `new_file` — `true` when the file is within the recency window
//!   (age_days < `days`) OR below the minimum-commits threshold OR
//!   has unknown lineage (no history record at all).
//! - `metadata_available` — `true` on a successful git run,
//!   regardless of whether a specific file had any commits. `false`
//!   when the run failed, the repo is shallow, or newness was
//!   explicitly disabled.
//!
//! Rename handling matches the Ruby `parse_history` loop: `R` lines
//! create an alias from the old path to the new (canonical) path so
//! further touches resolve to a single file; `D` lines mark the
//! deleted path as a lineage boundary so an unrelated file that
//! later takes the same path doesn't inherit the deleted lineage.
//!
//! Shallow-clone detection runs before the log parse because a
//! shallow clone silently truncates history and would mark every
//! file "new". `analyze_shallow` returns metadata with
//! `metadata_available: false` plus a `ShallowClone` warning so the
//! caller can decide whether to attempt `git fetch --unshallow` or
//! disable newness.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default recency window in days. Matches Ruby
/// `Newness::DEFAULT_DAYS`.
pub const DEFAULT_DAYS: u64 = 30;
/// Default minimum commits before a file stops being "new". Matches
/// Ruby `Newness::DEFAULT_MIN_COMMITS`.
pub const DEFAULT_MIN_COMMITS: usize = 3;
const SECONDS_PER_DAY: u64 = 86_400;

/// Per-file newness record. `age_days` and `total_commits` are `0`
/// when the file has no history. `metadata_available` is the signal
/// consumers check to decide whether the record is informative
/// versus a "newness disabled or unavailable" placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNewness {
    pub new_file: bool,
    pub age_days: u64,
    pub total_commits: usize,
    pub metadata_available: bool,
}

impl FileNewness {
    /// The placeholder record newness-disabled runs hand back.
    /// Matches Ruby `disabled_metadata`.
    pub fn disabled() -> Self {
        Self {
            new_file: false,
            age_days: 0,
            total_commits: 0,
            metadata_available: false,
        }
    }
}

/// Non-fatal observations surfaced to the caller (usually printed
/// to stderr). The `ShallowClone` variant is load-bearing — a
/// shallow clone would make every file look new, so the caller
/// should either disable newness or unshallow before trusting the
/// results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewnessWarning {
    ShallowClone,
    NotAGitRepo,
    GitFailed(String),
    GitNotFound,
}

/// Result of a newness analysis pass.
///
/// On success, `metadata` holds one entry per file in the input
/// slice (missing files from the log still land as zero-history
/// records with `metadata_available: true`). On any failure path —
/// shallow clone, non-repo, git error — every file lands as the
/// `disabled()` placeholder and `warnings` names the reason.
#[derive(Debug, Clone, Default)]
pub struct NewnessReport {
    pub metadata: HashMap<PathBuf, FileNewness>,
    pub warnings: Vec<NewnessWarning>,
    pub succeeded: bool,
}

/// Configurable parameters for a newness analysis. Defaults match
/// the Ruby CLI's defaults.
#[derive(Debug, Clone, Copy)]
pub struct NewnessConfig {
    pub days: u64,
    pub min_commits: usize,
    /// Seconds since the Unix epoch, used as "now" for age
    /// computation. Tests override this so expected age_days don't
    /// drift with wall-clock. In production callers pass the real
    /// current time.
    pub now_epoch: u64,
}

impl Default for NewnessConfig {
    fn default() -> Self {
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            days: DEFAULT_DAYS,
            min_commits: DEFAULT_MIN_COMMITS,
            now_epoch,
        }
    }
}

/// Walk the committer-time git log for `repo_root`, restrict to
/// `files`, and emit per-file newness.
///
/// Guards: if the repo is shallow, returns the "disabled" report
/// with a `ShallowClone` warning (no log parse — a shallow log
/// would lie about newness). If git is missing or fails, returns
/// the disabled report with an appropriate warning.
pub fn analyze(repo_root: &Path, files: &[&Path], config: NewnessConfig) -> NewnessReport {
    if is_shallow_repository(repo_root) {
        return disabled_report(files, NewnessWarning::ShallowClone);
    }

    // When scanning a SUBDIRECTORY of the repository (e.g.
    // `/repo/app`), git log emits paths relative to the repo root
    // (`app/models/user.rb`), but the file-collector hands us paths
    // relative to the scan root (`models/user.rb`). Without
    // stripping that prefix from log paths, the wanted set never
    // matches and every file reads as zero-history. Ruby Newness
    // handles this via `rebase_to_analysis_root`. We compute the
    // prefix once and hand it to `parse_history`.
    //
    // `--relative` on git log would seem simpler, but mixing it
    // with `--find-renames` + `--name-status` has edge cases on
    // cross-subdirectory renames. We mirror Ruby's manual rebase
    // instead.
    let analysis_prefix = analysis_root_prefix(repo_root);

    let output = match Command::new("git")
        .args([
            "-C",
            repo_root.to_str().unwrap_or("."),
            "-c",
            "core.quotepath=off",
            "log",
            "--format=%x1e%ct",
            "--name-status",
            "--find-renames",
            "--diff-filter=ACMRD",
            "--no-merges",
        ])
        .output()
    {
        Ok(out) => out,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return disabled_report(files, NewnessWarning::GitNotFound);
        }
        Err(err) => return disabled_report(files, NewnessWarning::GitFailed(err.to_string())),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let warning = if stderr.contains("not a git repository") {
            NewnessWarning::NotAGitRepo
        } else {
            NewnessWarning::GitFailed(stderr.trim().to_string())
        };
        return disabled_report(files, warning);
    }

    let file_set: HashSet<PathBuf> = files.iter().map(|p| p.to_path_buf()).collect();
    let history = parse_history(&output.stdout, &file_set, analysis_prefix.as_deref());

    let metadata = files
        .iter()
        .map(|rel| {
            let path = rel.to_path_buf();
            let first_epoch = history.first_commit_epoch.get(&path).copied();
            let total_commits = history.total_commits.get(&path).copied().unwrap_or(0);
            let age_days = age_days(first_epoch, config.now_epoch);
            let is_new = is_new_file(age_days, total_commits, config);
            (
                path,
                FileNewness {
                    new_file: is_new,
                    age_days: age_days.unwrap_or(0),
                    total_commits,
                    metadata_available: true,
                },
            )
        })
        .collect();

    NewnessReport {
        metadata,
        warnings: Vec::new(),
        succeeded: true,
    }
}

/// Report where every requested file falls back to the disabled
/// placeholder — used on shallow, non-repo, or git failure.
fn disabled_report(files: &[&Path], warning: NewnessWarning) -> NewnessReport {
    let metadata = files
        .iter()
        .map(|rel| (rel.to_path_buf(), FileNewness::disabled()))
        .collect();
    NewnessReport {
        metadata,
        warnings: vec![warning],
        succeeded: false,
    }
}

/// Return the repo-relative prefix for `repo_root` when it's a
/// SUBDIRECTORY of the enclosing git toplevel. `Some("app/")` when
/// the scan root is `/repo/app`. `None` when the scan root IS the
/// toplevel (no rebasing needed) or git refuses to answer.
///
/// Matches Ruby `Newness#analysis_root_prefix`.
pub fn analysis_root_prefix(repo_root: &Path) -> Option<String> {
    let repo_arg = repo_root.to_str()?;
    let output = Command::new("git")
        .args(["-C", repo_arg, "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let toplevel_raw = String::from_utf8(output.stdout).ok()?;
    let toplevel = PathBuf::from(toplevel_raw.trim());
    let toplevel = std::fs::canonicalize(&toplevel).ok()?;
    let analysis = std::fs::canonicalize(repo_root).ok()?;
    if analysis == toplevel {
        return None;
    }
    let rel = analysis.strip_prefix(&toplevel).ok()?;
    let s = rel.to_str()?;
    if s.is_empty() || s == "." || s.starts_with("..") {
        return None;
    }
    Some(format!("{s}/"))
}

/// Ask git whether `repo_root` is a shallow clone. Falls back to
/// probing `.git/shallow` directly if the subprocess can't run.
/// Matches Ruby `shallow_repository?`.
pub fn is_shallow_repository(repo_root: &Path) -> bool {
    let repo_arg = repo_root.to_str().unwrap_or(".");
    match Command::new("git")
        .args(["-C", repo_arg, "rev-parse", "--is-shallow-repository"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.trim() == "true"
        }
        _ => repo_root.join(".git").join("shallow").exists(),
    }
}

struct ParsedHistory {
    first_commit_epoch: HashMap<PathBuf, u64>,
    total_commits: HashMap<PathBuf, usize>,
}

/// Parse `git log --format=%x1e%ct --name-status` output.
///
/// Each commit starts with `\x1e<unix-epoch>\n`, followed by zero
/// or more `<status>\t<path>` (or `R<score>\t<old>\t<new>`) lines.
/// We walk per-commit, canonicalize paths through renames, respect
/// deletion boundaries, and tally per-file `first_commit_epoch` +
/// `total_commits`.
///
/// `analysis_prefix` is the repo-relative prefix that identifies
/// the subdirectory being scanned (e.g. `Some("app/")` when the
/// scan root is `/repo/app`). Paths that don't start with the
/// prefix are dropped entirely; paths that do have the prefix
/// stripped before being compared against `wanted`. Matches Ruby
/// `rebase_to_analysis_root`.
fn parse_history(
    stdout: &[u8],
    wanted: &HashSet<PathBuf>,
    analysis_prefix: Option<&str>,
) -> ParsedHistory {
    let mut first_commit_epoch: HashMap<PathBuf, u64> = HashMap::new();
    let mut total_commits: HashMap<PathBuf, usize> = HashMap::new();
    let mut aliases: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut lineage_boundaries: HashSet<PathBuf> = HashSet::new();

    let text = match std::str::from_utf8(stdout) {
        Ok(s) => s,
        Err(_) => {
            return ParsedHistory {
                first_commit_epoch,
                total_commits,
            };
        }
    };

    let mut current_epoch: Option<u64> = None;
    let mut touched: HashSet<PathBuf> = HashSet::new();

    let flush = |epoch: Option<u64>,
                 touched: &mut HashSet<PathBuf>,
                 first: &mut HashMap<PathBuf, u64>,
                 totals: &mut HashMap<PathBuf, usize>| {
        if let Some(epoch) = epoch {
            for file in touched.iter() {
                *totals.entry(file.clone()).or_insert(0) += 1;
                let entry = first.entry(file.clone()).or_insert(u64::MAX);
                if epoch < *entry {
                    *entry = epoch;
                }
            }
        }
        touched.clear();
    };

    // Local helper closure: rebase a git-emitted path to the analysis
    // root. Returns `None` when the path lies outside the subdirectory
    // being scanned (and should be ignored entirely).
    let rebase = |raw: &str| -> Option<PathBuf> {
        match analysis_prefix {
            None => Some(PathBuf::from(raw)),
            Some(prefix) => raw.strip_prefix(prefix).map(PathBuf::from),
        }
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('\x1e') {
            flush(
                current_epoch,
                &mut touched,
                &mut first_commit_epoch,
                &mut total_commits,
            );
            current_epoch = rest.parse::<u64>().ok();
            continue;
        }
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or("");
        if status.starts_with('D') {
            // Deletion: mark the deleted path as a lineage boundary
            // so a later same-path file doesn't inherit this
            // lineage.
            if let Some(path) = parts.next() {
                let rebased = match rebase(path) {
                    Some(p) => p,
                    None => continue,
                };
                if let Some(canonical) = canonical_path(&rebased, &aliases) {
                    if wanted.contains(&canonical) {
                        lineage_boundaries.insert(rebased);
                    }
                }
            }
            continue;
        }
        if status.starts_with('R') || status.starts_with('C') {
            // Rename / copy: status is `R<score>` or `C<score>`,
            // followed by old path + new path. The NEW path is the
            // active one going forward.
            let raw_old = match parts.next() {
                Some(p) => p,
                None => continue,
            };
            let raw_new = match parts.next() {
                Some(p) => p,
                None => continue,
            };
            // Rebase both; if the new path is outside the analysis
            // subdirectory, skip the record (the file is being moved
            // out of our view). If only the old path is outside,
            // still track the new one — it just has no prior
            // in-scope lineage.
            let new_path = match rebase(raw_new) {
                Some(p) => p,
                None => continue,
            };
            let old_path = rebase(raw_old);
            let canonical_new = canonical_path(&new_path, &aliases).unwrap_or(new_path.clone());
            let old_boundary_hit = old_path
                .as_ref()
                .map(|p| lineage_boundaries.contains(p))
                .unwrap_or(false);
            if wanted.contains(&canonical_new)
                && !old_boundary_hit
                && !lineage_boundaries.contains(&new_path)
            {
                touched.insert(canonical_new.clone());
                if let Some(old) = old_path {
                    aliases.insert(old, canonical_new);
                }
            }
            continue;
        }
        // Add / modify / else (A, M, T, …): single path follows.
        if let Some(path) = parts.next() {
            let rebased = match rebase(path) {
                Some(p) => p,
                None => continue,
            };
            if lineage_boundaries.contains(&rebased) {
                continue;
            }
            if let Some(canonical) = canonical_path(&rebased, &aliases) {
                if wanted.contains(&canonical) {
                    touched.insert(canonical);
                }
            }
        }
    }
    flush(
        current_epoch,
        &mut touched,
        &mut first_commit_epoch,
        &mut total_commits,
    );

    ParsedHistory {
        first_commit_epoch,
        total_commits,
    }
}

fn canonical_path(path: &Path, aliases: &HashMap<PathBuf, PathBuf>) -> Option<PathBuf> {
    let mut canonical = path.to_path_buf();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    while let Some(next) = aliases.get(&canonical) {
        if !seen.insert(canonical.clone()) {
            break; // cycle guard
        }
        canonical = next.clone();
    }
    Some(canonical)
}

fn age_days(first_epoch: Option<u64>, now: u64) -> Option<u64> {
    let first = first_epoch?;
    if now <= first {
        return Some(0);
    }
    Some((now - first) / SECONDS_PER_DAY)
}

fn is_new_file(age_days: Option<u64>, total_commits: usize, cfg: NewnessConfig) -> bool {
    // Unknown lineage (no history record) always reads as new —
    // matches Ruby `unknown_lineage || within_age_window ||
    // low_commit_count`.
    let unknown_lineage = age_days.is_none();
    let within_age_window = cfg.days > 0 && age_days.map(|d| d < cfg.days).unwrap_or(false);
    let low_commit_count = cfg.min_commits > 0 && total_commits < cfg.min_commits;
    unknown_lineage || within_age_window || low_commit_count
}

/// Evidence confidence in `[0.0, 1.0]`, matching Ruby
/// `Newness.evidence`. `None` when the metadata record is
/// unavailable (shallow / disabled / failed run). `explicit_coverage`
/// is `true` when the file has a coverage entry in the loaded
/// coverage report — raises the ceiling via the third component.
pub fn evidence_for(metadata: &FileNewness, explicit_coverage: bool) -> Option<f64> {
    if !metadata.metadata_available {
        return None;
    }
    let age_component = (metadata.age_days as f64 / 30.0).min(1.0);
    let commits_component = (metadata.total_commits as f64 / 3.0).min(1.0);
    let history_only = (age_component + commits_component) / 2.0;
    let candidate = if explicit_coverage {
        let with_coverage = (age_component + commits_component + 1.0) / 3.0;
        history_only.max(with_coverage)
    } else {
        history_only
    };
    Some(round4(candidate))
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests;
