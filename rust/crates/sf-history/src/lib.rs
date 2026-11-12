//! Git-history signals for stud-finder: churn (commits + lines) and
//! temporal coupling (files that usually change together). One-shot
//! analysis — no sidecar persistence, no daemon. Shells out to `git log`
//! once and derives both signals from the same output stream.
//!
//! Parity targets (bazfer/stud-finder, main branch, 2026-11-12):
//!
//! - `lib/stud_finder/churn.rb` — `--numstat --no-merges --find-renames
//!   --diff-filter=ACDMR`, counts commits + sums added+deleted lines per
//!   file, filters files to the provided set, skips binary files
//!   (`added == '-'`).
//! - `lib/stud_finder/temporal_coupling.rb` — bulk commits (> 50 files)
//!   count toward churn but NOT toward co-change. Coupling ratio is
//!   `co_changes / min(own_changes[a], own_changes[b])` (NOT Jaccard —
//!   that's the spec's wording glossing over the Ruby semantics). Default
//!   thresholds: min_co_changes = 5, coupling_threshold = 0.30. Per-file
//!   strongest partner wins ties by highest co-change count, then
//!   alphabetical path.
//!
//! Rename following: this slice does NOT follow renames. `--find-renames`
//! is on in git but we treat the `{old => new}` notation as a single
//! "new" path. A rename inside the window is accepted as the new path's
//! churn/co-change; the old path simply doesn't appear. Matches the
//! Ruby shape exactly: both churn.rb and temporal_coupling.rb normalize
//! to the new name via `renamed_path`.
//!
//! Attribution: the shape (shell out to `git log`, stream output,
//! bulk-commit filter, per-commit file dedup, pairwise accumulation)
//! follows the pattern established in Saugat Khadka's `ripple` at commit
//! 6604d76, `crates/ripple-history/src/gitlog.rs`. The actual code and
//! semantics here match bazfer/stud-finder's Ruby implementation, not
//! ripple's; no verbatim lift.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Default churn window — matches Ruby CLI default `--churn-days 180`.
pub const DEFAULT_WINDOW_DAYS: u64 = 180;

/// Bulk-commit threshold: commits touching strictly more than this many
/// files count toward churn but not toward coupling. Matches Ruby
/// `TemporalCoupling#max_commit_files` default (50).
pub const BULK_COMMIT_THRESHOLD: usize = 50;

/// Pairs with fewer co-changes than this are dropped. Matches Ruby
/// `TemporalCoupling#min_co_changes` default.
pub const MIN_CO_CHANGES: usize = 5;

/// Pairs with coupling ratio below this are dropped. Matches Ruby
/// `TemporalCoupling#coupling_threshold` default.
pub const COUPLING_THRESHOLD: f64 = 0.30;

/// Per-file coupling result: strongest partner, the co-change ratio, and
/// the raw co-change count for that pair. Matches the loop spec tuple
/// shape `(partner, ratio, co_changes)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CouplingPartner {
    pub partner: PathBuf,
    pub ratio: f64,
    pub co_changes: usize,
}

/// Non-fatal observations emitted during analysis — surfaced on stderr
/// by the caller. `BulkCommitsSkipped` counts how many commits were
/// large enough to skip from coupling (they still count toward churn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryWarning {
    BulkCommitsSkipped {
        count: usize,
        max_commit_files: usize,
    },
    NotAGitRepo,
    GitFailed(String),
    GitNotFound,
}

/// Output of [`analyze`]. On a successful git run, `churn_commits` and
/// `churn_lines` are pre-populated with a zero entry for every file in
/// the caller's `files` slice, so a "git succeeded but nothing in the
/// window touched the selected files" result is distinguishable from
/// a git-failure result (empty maps + warning) at the caller. `coupling`
/// stays sparse — only files whose pair cleared the Ruby filters land
/// there. `history_succeeded` is the authoritative success/failure
/// flag that downstream consumers use to decide whether to treat the
/// churn signal as active.
///
/// Fixes Ohm's P2 on PR #79 about zero-change windows silently
/// disabling the churn weight.
#[derive(Debug, Default, Clone)]
pub struct HistoryReport {
    pub churn_commits: HashMap<PathBuf, usize>,
    pub churn_lines: HashMap<PathBuf, usize>,
    pub coupling: HashMap<PathBuf, CouplingPartner>,
    pub warnings: Vec<HistoryWarning>,
    /// `true` when the `git log` subprocess ran cleanly, even if no
    /// commits in the window touched any of the selected files.
    /// `false` when git was missing, failed, or the directory wasn't
    /// a git repository.
    pub history_succeeded: bool,
}

/// Walks `repo_root`'s git log back `window_days` days and returns
/// churn + coupling signals restricted to `files`. Returns an empty
/// report with a warning if the repo isn't a git repo or `git` is
/// missing.
pub fn analyze(repo_root: &Path, files: &[&Path], window_days: u64) -> HistoryReport {
    let file_set: HashSet<PathBuf> = files.iter().map(|p| p.to_path_buf()).collect();

    let since_arg = format!("--since={window_days} days ago");
    // `--relative` makes git emit paths relative to the `-C` directory
    // instead of the repository root. The CLI's file_set is keyed by
    // paths relative to the scan root (which may be a subdirectory of
    // the repo), so without this a sub-directory scan would silently
    // drop every entry. Fixes Ohm's P2 on PR #79.
    let args = [
        "-C",
        repo_root.to_str().unwrap_or("."),
        "-c",
        "core.quotepath=off",
        "log",
        &since_arg,
        "--format=%x00%H",
        "--numstat",
        "--no-merges",
        "--find-renames",
        "--diff-filter=ACDMR",
        "--relative",
    ];

    let output = match Command::new("git").args(args).stdin(Stdio::null()).output() {
        Ok(o) => o,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return HistoryReport {
                warnings: vec![HistoryWarning::GitNotFound],
                ..Default::default()
            };
        }
        Err(err) => {
            return HistoryReport {
                warnings: vec![HistoryWarning::GitFailed(err.to_string())],
                ..Default::default()
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // `fatal: not a git repository` is the common case for a
        // caller passing a non-repo path — surface it as its own
        // warning variant so the CLI can print a friendlier message.
        if stderr.contains("not a git repository") {
            return HistoryReport {
                warnings: vec![HistoryWarning::NotAGitRepo],
                ..Default::default()
            };
        }
        return HistoryReport {
            warnings: vec![HistoryWarning::GitFailed(stderr.trim().to_string())],
            ..Default::default()
        };
    }

    parse_log(&output.stdout, &file_set)
}

fn parse_log(stdout: &[u8], file_set: &HashSet<PathBuf>) -> HistoryReport {
    // Pre-populate zero churn for every requested file. A successful
    // `git log` that returned no commits touching our file set still
    // yields "every file has 0 churn" rather than "churn signal
    // unavailable" — Ruby `Churn#initial_counts` does the same.
    let mut churn_commits: HashMap<PathBuf, usize> =
        file_set.iter().map(|p| (p.clone(), 0usize)).collect();
    let mut churn_lines: HashMap<PathBuf, usize> =
        file_set.iter().map(|p| (p.clone(), 0usize)).collect();
    let mut own_changes: HashMap<PathBuf, usize> = HashMap::new();
    // Pairs stored canonically (lo, hi) to dedup both orderings.
    let mut co_matrix: HashMap<(PathBuf, PathBuf), usize> = HashMap::new();
    let mut bulk_commits_skipped: usize = 0;

    // Commit records are prefixed by a NUL from `%x00%H`. Split on NUL
    // and skip the first (empty) chunk before the first commit.
    for chunk in stdout.split(|&b| b == 0u8).skip(1) {
        if chunk.is_empty() {
            continue;
        }
        let Some((hash_line, rest)) = split_once_byte(chunk, b'\n') else {
            // A record with no body (commit with no diff) — rare under
            // --diff-filter=ACDMR but harmless; nothing to count.
            continue;
        };
        if hash_line.is_empty() {
            continue;
        }

        // Dedup within a commit: a file appearing twice in --numstat
        // (can happen with some rename shapes) must only count once.
        let mut commit_files: Vec<PathBuf> = Vec::new();
        let mut commit_seen: HashSet<PathBuf> = HashSet::new();

        for line in rest.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let Some((added, rest2)) = split_once_byte(line, b'\t') else {
                continue;
            };
            let Some((deleted, path_bytes)) = split_once_byte(rest2, b'\t') else {
                continue;
            };
            let path_str = String::from_utf8_lossy(path_bytes);
            let normalized = normalize_rename(path_str.as_ref());
            let path = PathBuf::from(normalized.as_ref());
            if !file_set.contains(&path) {
                continue;
            }

            // Binary files show `-\t-\t<path>`; count the commit toward
            // the file's commit count but not its line count. Matches
            // Ruby `churn.rb` guard `added != '-' && numeric?(deleted)`.
            let added_s = std::str::from_utf8(added).unwrap_or("-");
            let deleted_s = std::str::from_utf8(deleted).unwrap_or("-");
            let line_delta = if added_s != "-" && is_numeric(deleted_s) {
                added_s.parse::<usize>().unwrap_or(0) + deleted_s.parse::<usize>().unwrap_or(0)
            } else {
                0
            };

            if commit_seen.insert(path.clone()) {
                // `entry(...).or_insert(0)` is safe either way — the
                // key is already present (set to 0 by the
                // pre-population above) for files in file_set.
                *churn_commits.entry(path.clone()).or_insert(0) += 1;
                *churn_lines.entry(path.clone()).or_insert(0) += line_delta;
                commit_files.push(path);
            } else {
                // Same path twice: still add the extra line delta so
                // churn_lines matches what --numstat totals for that
                // file in that commit. Commit count stays at 1.
                *churn_lines.entry(path).or_insert(0) += line_delta;
            }
        }

        // Coupling: skip bulk commits (> BULK_COMMIT_THRESHOLD files
        // in commit_files restricted to the file_set). Matches Ruby
        // `append_commit` which filters by the file-set-restricted
        // count, not the total commit size.
        if commit_files.len() > BULK_COMMIT_THRESHOLD {
            bulk_commits_skipped += 1;
            continue;
        }
        if commit_files.len() < 2 {
            // No pairs possible, but still bump own_changes for the
            // lone file so a 1-file commit contributes to the own_changes
            // denominator the Ruby code uses.
            for p in &commit_files {
                *own_changes.entry(p.clone()).or_insert(0) += 1;
            }
            continue;
        }

        for p in &commit_files {
            *own_changes.entry(p.clone()).or_insert(0) += 1;
        }
        for i in 0..commit_files.len() {
            for j in (i + 1)..commit_files.len() {
                let a = &commit_files[i];
                let b = &commit_files[j];
                let key = if a <= b {
                    (a.clone(), b.clone())
                } else {
                    (b.clone(), a.clone())
                };
                *co_matrix.entry(key).or_insert(0) += 1;
            }
        }
    }

    let coupling = select_strongest_partners(&co_matrix, &own_changes);

    let mut warnings = Vec::new();
    if bulk_commits_skipped > 0 {
        warnings.push(HistoryWarning::BulkCommitsSkipped {
            count: bulk_commits_skipped,
            max_commit_files: BULK_COMMIT_THRESHOLD,
        });
    }

    HistoryReport {
        churn_commits,
        churn_lines,
        coupling,
        warnings,
        history_succeeded: true,
    }
}

/// Rounds `x` to 4 decimal places, matching the Ruby
/// `TemporalCoupling#build_pairs` boundary which stores
/// `(count.to_f / min_own).round(4)`. Applied BEFORE the threshold
/// check and BEFORE partner ranking so two raw ratios that round to
/// the same value sort and classify identically to Ruby.
fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn select_strongest_partners(
    co_matrix: &HashMap<(PathBuf, PathBuf), usize>,
    own_changes: &HashMap<PathBuf, usize>,
) -> HashMap<PathBuf, CouplingPartner> {
    // Walk through pairs and record the best partner on each endpoint.
    // Pre-order by (path, path) so ties break alphabetically on the
    // partner — BTreeMap keeps the inputs sorted so the tiebreak is
    // determined by the iteration order Rust gives us.
    let mut sorted: BTreeMap<&(PathBuf, PathBuf), &usize> = BTreeMap::new();
    for (k, v) in co_matrix {
        sorted.insert(k, v);
    }

    let mut best: HashMap<PathBuf, CouplingPartner> = HashMap::new();
    let mut consider = |file: &Path, partner: &Path, count: usize| {
        if count < MIN_CO_CHANGES {
            return;
        }
        let own_a = *own_changes.get(file).unwrap_or(&0);
        let own_b = *own_changes.get(partner).unwrap_or(&0);
        let min_own = own_a.min(own_b);
        if min_own == 0 {
            return;
        }
        let ratio = round4((count as f64) / (min_own as f64));
        if ratio < COUPLING_THRESHOLD {
            return;
        }
        let candidate = CouplingPartner {
            partner: partner.to_path_buf(),
            ratio,
            co_changes: count,
        };
        match best.get(file) {
            None => {
                best.insert(file.to_path_buf(), candidate);
            }
            Some(current) => {
                // Strict > on ratio; tie → strict > on co_changes; tie →
                // keep earlier (alphabetical, from the sorted walk above).
                if candidate.ratio > current.ratio
                    || (candidate.ratio == current.ratio
                        && candidate.co_changes > current.co_changes)
                {
                    best.insert(file.to_path_buf(), candidate);
                }
            }
        }
    };

    for ((a, b), count) in &sorted {
        consider(a, b, **count);
        consider(b, a, **count);
    }
    best
}

fn split_once_byte(buf: &[u8], needle: u8) -> Option<(&[u8], &[u8])> {
    let idx = buf.iter().position(|&b| b == needle)?;
    Some((&buf[..idx], &buf[idx + 1..]))
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Normalize git's rename notation to the "new" path. Mirrors Ruby's
/// `renamed_path`:
/// - `foo/{a => b}/baz.rb` → `foo/b/baz.rb`
/// - `old/path.rb => new/path.rb` → `new/path.rb`
/// - everything else → unchanged
fn normalize_rename(path: &str) -> std::borrow::Cow<'_, str> {
    if let Some(stripped) = strip_brace_rename(path) {
        return std::borrow::Cow::Owned(stripped);
    }
    if let Some(arrow) = path.find(" => ") {
        return std::borrow::Cow::Owned(path[arrow + 4..].to_string());
    }
    std::borrow::Cow::Borrowed(path)
}

/// Returns the normalized path when `path` is of brace-rename form
/// `prefix/{old => new}/suffix`, otherwise `None`. Split out of
/// `normalize_rename` as MSRV 1.85 does not support let-chains (those
/// stabilized in 1.88).
fn strip_brace_rename(path: &str) -> Option<String> {
    let open = path.find('{')?;
    let close_rel = path[open..].find('}')?;
    let close = open + close_rel;
    let inner = &path[open + 1..close];
    let arrow = inner.find(" => ")?;
    let new_half = &inner[arrow + 4..];
    let mut out = String::with_capacity(path.len());
    out.push_str(&path[..open]);
    out.push_str(new_half);
    out.push_str(&path[close + 1..]);
    Some(out)
}

#[cfg(test)]
mod tests;
