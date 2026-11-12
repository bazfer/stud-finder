#![allow(clippy::type_complexity)]

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One file entry (added, deleted, path) in a fake git-log commit.
type FakeEntry<'a> = (&'a str, &'a str, &'a str);
/// One fake commit: (hash, entries).
type FakeCommit<'a> = (&'a str, &'a [FakeEntry<'a>]);

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

/// Build a `git log --numstat --format=%x00%H` byte stream from a
/// deterministic list of commits. Each commit is a slice of
/// (added, deleted, path) tuples; `added` or `deleted` can be `-` to
/// simulate a binary diff line.
fn fake_log(commits: &[FakeCommit<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (hash, entries) in commits {
        out.push(0u8);
        out.extend_from_slice(hash.as_bytes());
        out.push(b'\n');
        for (a, d, path) in *entries {
            out.extend_from_slice(a.as_bytes());
            out.push(b'\t');
            out.extend_from_slice(d.as_bytes());
            out.push(b'\t');
            out.extend_from_slice(path.as_bytes());
            out.push(b'\n');
        }
    }
    out
}

fn files_set(paths: &[&str]) -> HashSet<PathBuf> {
    paths.iter().copied().map(PathBuf::from).collect()
}

#[test]
fn empty_log_produces_zero_report_with_success_flag() {
    // After the P2 fix: a successful git call with no commits still
    // yields zero entries for every requested file (pre-populated),
    // and `history_succeeded` is true so downstream can tell this
    // apart from a git failure.
    let report = parse_log(&[], &files_set(&["a.rb", "b.rb"]));
    assert_eq!(report.churn_commits.get(&p("a.rb")), Some(&0));
    assert_eq!(report.churn_commits.get(&p("b.rb")), Some(&0));
    assert_eq!(report.churn_lines.get(&p("a.rb")), Some(&0));
    assert_eq!(report.churn_lines.get(&p("b.rb")), Some(&0));
    assert!(report.coupling.is_empty());
    assert!(report.warnings.is_empty());
    assert!(report.history_succeeded);
}

#[test]
fn churn_counts_commits_and_sums_lines() {
    let log = fake_log(&[
        ("aaa", &[("3", "1", "a.rb"), ("2", "0", "b.rb")]),
        ("bbb", &[("1", "1", "a.rb")]),
        ("ccc", &[("5", "0", "c.rb")]), // not in file set
    ]);
    let report = parse_log(&log, &files_set(&["a.rb", "b.rb"]));
    assert_eq!(report.churn_commits.get(&p("a.rb")), Some(&2));
    assert_eq!(report.churn_commits.get(&p("b.rb")), Some(&1));
    assert_eq!(report.churn_commits.get(&p("c.rb")), None);
    assert_eq!(report.churn_lines.get(&p("a.rb")), Some(&(3 + 1 + 1 + 1)));
    assert_eq!(report.churn_lines.get(&p("b.rb")), Some(&2));
}

#[test]
fn binary_files_count_commits_but_not_lines() {
    // Ruby `churn.rb` guards `added != '-' && numeric?(deleted)` — the
    // commit still counts, lines don't.
    let log = fake_log(&[("aaa", &[("-", "-", "logo.png"), ("3", "1", "a.rb")])]);
    let report = parse_log(&log, &files_set(&["logo.png", "a.rb"]));
    assert_eq!(report.churn_commits.get(&p("logo.png")), Some(&1));
    assert_eq!(report.churn_lines.get(&p("logo.png")), Some(&0));
    assert_eq!(report.churn_commits.get(&p("a.rb")), Some(&1));
    assert_eq!(report.churn_lines.get(&p("a.rb")), Some(&4));
}

#[test]
fn rename_brace_normalizes_to_new_path() {
    assert_eq!(normalize_rename("foo/{a => b}/x.rb"), "foo/b/x.rb");
    assert_eq!(normalize_rename("foo/{old.rb => new.rb}"), "foo/new.rb");
    assert_eq!(normalize_rename("foo/{ => sub}/x.rb"), "foo/sub/x.rb");
    assert_eq!(normalize_rename("foo/{a => }/x.rb"), "foo//x.rb");
    assert_eq!(normalize_rename("old.rb => new.rb"), "new.rb");
    assert_eq!(normalize_rename("plain.rb"), "plain.rb");
}

#[test]
fn coupling_pair_matches_ruby_min_own_ratio() {
    // Two files co-changed in 6 commits; a.rb has 10 own_changes,
    // b.rb has 8. Ratio = 6 / min(10, 8) = 0.75. Above 0.30 threshold.
    let mut commits: Vec<(&str, Vec<(&str, &str, &str)>)> = Vec::new();
    for i in 0..6 {
        let hash = Box::leak(format!("co{i:02}").into_boxed_str());
        commits.push((
            hash as &'static str,
            vec![("1", "1", "a.rb"), ("1", "1", "b.rb")],
        ));
    }
    for i in 0..4 {
        let hash = Box::leak(format!("sa{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "a.rb")]));
    }
    for i in 0..2 {
        let hash = Box::leak(format!("sb{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "b.rb")]));
    }
    // Convert to the slice-of-slices shape fake_log wants.
    let commits_ref: Vec<(&str, &[(&str, &str, &str)])> =
        commits.iter().map(|(h, v)| (*h, v.as_slice())).collect();
    let log = fake_log(&commits_ref);

    let report = parse_log(&log, &files_set(&["a.rb", "b.rb"]));
    assert_eq!(report.churn_commits.get(&p("a.rb")), Some(&10));
    assert_eq!(report.churn_commits.get(&p("b.rb")), Some(&8));

    let a_partner = report.coupling.get(&p("a.rb")).expect("a.rb coupled");
    assert_eq!(a_partner.partner, p("b.rb"));
    assert_eq!(a_partner.co_changes, 6);
    assert!((a_partner.ratio - 0.75).abs() < 1e-9);

    let b_partner = report.coupling.get(&p("b.rb")).expect("b.rb coupled");
    assert_eq!(b_partner.partner, p("a.rb"));
    assert_eq!(b_partner.co_changes, 6);
    assert!((b_partner.ratio - 0.75).abs() < 1e-9);
}

#[test]
fn coupling_below_min_co_changes_is_dropped() {
    // 4 co-changes < MIN_CO_CHANGES (5) → drop pair even though
    // ratio would be 1.0.
    let commits: Vec<(&str, &[(&str, &str, &str)])> = vec![
        ("c01", &[("1", "1", "a.rb"), ("1", "1", "b.rb")]),
        ("c02", &[("1", "1", "a.rb"), ("1", "1", "b.rb")]),
        ("c03", &[("1", "1", "a.rb"), ("1", "1", "b.rb")]),
        ("c04", &[("1", "1", "a.rb"), ("1", "1", "b.rb")]),
    ];
    let log = fake_log(&commits);
    let report = parse_log(&log, &files_set(&["a.rb", "b.rb"]));
    assert!(report.coupling.is_empty(), "4 co-changes < 5 should drop");
}

#[test]
fn coupling_below_threshold_ratio_is_dropped() {
    // 5 co-changes but min_own is 20 → ratio = 0.25 < 0.30 threshold.
    let mut commits: Vec<(&str, Vec<(&str, &str, &str)>)> = Vec::new();
    for i in 0..5 {
        let hash = Box::leak(format!("co{i:02}").into_boxed_str());
        commits.push((
            hash as &'static str,
            vec![("1", "1", "a.rb"), ("1", "1", "b.rb")],
        ));
    }
    for i in 0..15 {
        let hash = Box::leak(format!("sa{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "a.rb")]));
    }
    for i in 0..15 {
        let hash = Box::leak(format!("sb{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "b.rb")]));
    }
    let commits_ref: Vec<(&str, &[(&str, &str, &str)])> =
        commits.iter().map(|(h, v)| (*h, v.as_slice())).collect();
    let log = fake_log(&commits_ref);
    let report = parse_log(&log, &files_set(&["a.rb", "b.rb"]));
    assert!(
        report.coupling.is_empty(),
        "ratio 5/20 = 0.25 should drop (< 0.30)"
    );
}

#[test]
fn bulk_commit_filter_skips_coupling_but_counts_churn() {
    // 51 files in one commit — beyond BULK_COMMIT_THRESHOLD (50).
    // Churn counts stay; no coupling pair is recorded from this commit.
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for i in 0..51 {
        entries.push(("1".into(), "1".into(), format!("f{i:03}.rb")));
    }
    let entry_refs: Vec<(&str, &str, &str)> = entries
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    let commits: Vec<(&str, &[(&str, &str, &str)])> = vec![("bulk", entry_refs.as_slice())];
    let log = fake_log(&commits);
    let file_names: Vec<String> = (0..51).map(|i| format!("f{i:03}.rb")).collect();
    let paths: Vec<PathBuf> = file_names.iter().map(PathBuf::from).collect();
    let file_set: HashSet<PathBuf> = paths.iter().cloned().collect();
    let report = parse_log(&log, &file_set);
    assert_eq!(report.churn_commits.len(), 51);
    for pb in &paths {
        assert_eq!(report.churn_commits.get(pb), Some(&1));
    }
    assert!(report.coupling.is_empty());
    assert_eq!(
        report.warnings,
        vec![HistoryWarning::BulkCommitsSkipped {
            count: 1,
            max_commit_files: BULK_COMMIT_THRESHOLD,
        }]
    );
}

#[test]
fn partner_strongest_by_ratio_then_cochanges() {
    // a.rb has two candidate partners:
    //   b.rb: 5 co-changes, min_own = 5 → ratio 1.0
    //   c.rb: 10 co-changes, min_own = 20 → ratio 0.5
    // b.rb wins by ratio.
    let mut commits: Vec<(&str, Vec<(&str, &str, &str)>)> = Vec::new();
    for i in 0..5 {
        let hash = Box::leak(format!("ab{i:02}").into_boxed_str());
        commits.push((
            hash as &'static str,
            vec![("1", "1", "a.rb"), ("1", "1", "b.rb")],
        ));
    }
    for i in 0..10 {
        let hash = Box::leak(format!("ac{i:02}").into_boxed_str());
        commits.push((
            hash as &'static str,
            vec![("1", "1", "a.rb"), ("1", "1", "c.rb")],
        ));
    }
    for i in 0..5 {
        let hash = Box::leak(format!("sa{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "a.rb")]));
    }
    for i in 0..10 {
        let hash = Box::leak(format!("sc{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "c.rb")]));
    }
    // a.rb: 5(ab) + 10(ac) + 5(sa) = 20 own
    // b.rb: 5(ab) = 5 own
    // c.rb: 10(ac) + 10(sc) = 20 own
    // ratio(a,b) = 5 / min(20,5) = 1.0, ratio(a,c) = 10 / min(20,20) = 0.5
    let commits_ref: Vec<(&str, &[(&str, &str, &str)])> =
        commits.iter().map(|(h, v)| (*h, v.as_slice())).collect();
    let log = fake_log(&commits_ref);
    let report = parse_log(&log, &files_set(&["a.rb", "b.rb", "c.rb"]));
    let a = report.coupling.get(&p("a.rb")).expect("a.rb coupled");
    assert_eq!(a.partner, p("b.rb"), "higher ratio (1.0) wins over 0.5");
    assert!((a.ratio - 1.0).abs() < 1e-9);
}

#[test]
fn files_outside_set_are_excluded_from_everything() {
    let log = fake_log(&[
        ("c01", &[("3", "1", "a.rb"), ("2", "0", "extern.rb")]),
        ("c02", &[("1", "1", "extern.rb")]),
    ]);
    let report = parse_log(&log, &files_set(&["a.rb"]));
    assert_eq!(report.churn_commits.get(&p("a.rb")), Some(&1));
    assert_eq!(report.churn_commits.get(&p("extern.rb")), None);
}

/// End-to-end: builds a real git repo in a tempdir, runs `analyze`,
/// asserts the churn/coupling signals match hand-computed expectations.
#[test]
fn analyze_over_real_repo_matches_hand_computed_signals() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    };

    run(&["init", "-q", "-b", "main"]);
    run(&["config", "commit.gpgsign", "false"]);

    // Writes `content` to `rel` under repo and makes a commit touching that
    // path (plus optionally one or more co-touched paths).
    let commit = |changes: &[(&str, &str)], msg: &str| {
        for (rel, content) in changes {
            let full = repo.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, content).unwrap();
            run(&["add", rel]);
        }
        run(&["commit", "-q", "-m", msg]);
    };

    // 5 co-commits touching a.rb + b.rb; 3 a-only; 2 b-only.
    for i in 0..5 {
        commit(
            &[
                ("a.rb", &format!("a v{i}\n")),
                ("b.rb", &format!("b v{i}\n")),
            ],
            &format!("co {i}"),
        );
    }
    for i in 0..3 {
        commit(&[("a.rb", &format!("a solo {i}\n"))], &format!("a {i}"));
    }
    for i in 0..2 {
        commit(&[("b.rb", &format!("b solo {i}\n"))], &format!("b {i}"));
    }
    // a.rb: 5 + 3 = 8 commits, b.rb: 5 + 2 = 7 commits.
    // coupling ratio = 5 / min(8, 7) = 5/7 ≈ 0.7142857...

    let a = PathBuf::from("a.rb");
    let b = PathBuf::from("b.rb");
    let files: Vec<&Path> = vec![a.as_path(), b.as_path()];
    let report = analyze(repo, &files, 365);

    assert!(
        report.warnings.is_empty(),
        "no warnings expected: {:?}",
        report.warnings
    );
    assert_eq!(report.churn_commits.get(&a), Some(&8));
    assert_eq!(report.churn_commits.get(&b), Some(&7));

    let a_partner = report.coupling.get(&a).expect("a coupled");
    assert_eq!(a_partner.partner, b);
    assert_eq!(a_partner.co_changes, 5);
    // Ratio is Ruby `round(4)` of 5/7 = 0.71428... → 0.7143.
    assert!(
        (a_partner.ratio - 0.7143).abs() < 1e-9,
        "ratio drift: {}",
        a_partner.ratio
    );
}

#[test]
fn analyze_on_non_repo_returns_warning_not_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let report = analyze(tmp.path(), &[], 180);
    assert!(report.churn_commits.is_empty());
    assert!(!report.history_succeeded, "failure must flip the flag");
    assert!(
        matches!(
            report.warnings.first(),
            Some(HistoryWarning::NotAGitRepo) | Some(HistoryWarning::GitFailed(_))
        ),
        "expected NotAGitRepo or GitFailed, got: {:?}",
        report.warnings
    );
}

#[test]
fn successful_zero_history_populates_zero_entries_and_flag() {
    // Ohm P2 on PR #79: a successful git log where none of the selected
    // files changed must still yield "every file has 0 churn" (so the
    // churn weight stays active downstream), not empty maps.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(status.status.success());
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "commit.gpgsign", "false"]);
    // One commit touching ONLY `other.rb`. We request `target.rb`.
    std::fs::write(repo.join("other.rb"), "other\n").unwrap();
    run(&["add", "other.rb"]);
    run(&["commit", "-q", "-m", "x"]);

    let target = PathBuf::from("target.rb");
    let report = analyze(repo, &[target.as_path()], 365);
    assert!(report.history_succeeded, "git succeeded; flag must be true");
    assert_eq!(
        report.churn_commits.get(&target),
        Some(&0),
        "zero entry must be present for success-with-no-selected-changes"
    );
    assert_eq!(report.churn_lines.get(&target), Some(&0));
}

#[test]
fn nested_subdir_scan_preserves_caller_relative_paths() {
    // Ohm P2 on PR #79: scanning a subdirectory of a repo used to drop
    // every entry because git emitted repo-root-relative paths while
    // the caller's file_set was keyed by subdir-relative paths. The
    // `--relative` flag now normalizes git output into the caller's
    // coordinate system.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let run = |cwd: &Path, args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(status.status.success());
    };
    run(repo, &["init", "-q", "-b", "main"]);
    run(repo, &["config", "commit.gpgsign", "false"]);

    let nested = repo.join("lib/models");
    std::fs::create_dir_all(&nested).unwrap();
    for i in 0..3 {
        std::fs::write(nested.join("user.rb"), format!("u{i}\n")).unwrap();
        run(repo, &["add", "lib/models/user.rb"]);
        run(repo, &["commit", "-q", "-m", &format!("c{i}")]);
    }

    // Caller scans `lib/models` as the root: they pass "user.rb" (not
    // "lib/models/user.rb") as the file key. With --relative, git emits
    // "user.rb" from that subdir, so the file_set matches.
    let user = PathBuf::from("user.rb");
    let report = analyze(&nested, &[user.as_path()], 365);
    assert!(report.history_succeeded);
    assert_eq!(
        report.churn_commits.get(&user),
        Some(&3),
        "subdir scan should see 3 commits touching user.rb: {:?}",
        report.churn_commits
    );
}

#[test]
fn coupling_ratio_rounds_to_four_decimals_matching_ruby() {
    // Ruby `temporal_coupling.rb` rounds `count / min_own` to 4 decimals
    // BEFORE the threshold check and before partner ranking. Two raw
    // ratios that round to the same value must sort and classify the
    // same way as in Ruby. Example: 5 co-changes / 7 own = 0.71428... →
    // rounded to 0.7143.
    let mut commits: Vec<(&'static str, Vec<(&str, &str, &str)>)> = Vec::new();
    for i in 0..5 {
        let hash = Box::leak(format!("co{i:02}").into_boxed_str());
        commits.push((
            hash as &'static str,
            vec![("1", "1", "a.rb"), ("1", "1", "b.rb")],
        ));
    }
    for i in 0..2 {
        let hash = Box::leak(format!("sa{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "a.rb")]));
    }
    // a: 5+2 = 7; b: 5. min = 5. ratio = 5/5 = 1.0 exactly for b,
    // 5/7 = 0.71428... → 0.7143 for a.
    let commits_ref: Vec<FakeCommit<'_>> =
        commits.iter().map(|(h, v)| (*h, v.as_slice())).collect();
    let log = fake_log(&commits_ref);
    let report = parse_log(&log, &files_set(&["a.rb", "b.rb"]));
    let a = report.coupling.get(&p("a.rb")).expect("a coupled");
    // 5 / 5 = 1.0 (a's own is 7, b's own is 5, min = 5)
    assert!(
        (a.ratio - 1.0).abs() < 1e-9,
        "a→b ratio expected 1.0 (5/min(7,5)=5/5), got {}",
        a.ratio
    );
    let b = report.coupling.get(&p("b.rb")).expect("b coupled");
    assert!((b.ratio - 1.0).abs() < 1e-9, "b→a ratio got {}", b.ratio);
}

#[test]
fn coupling_ratio_near_threshold_rounds_before_check() {
    // 5 co-changes, min_own = 17 → raw ratio = 0.29411... which is
    // BELOW 0.30 threshold. After Ruby's round(4) it's 0.2941 — still
    // below. Should drop.
    //
    // Verify our code matches: no coupling partner emitted.
    let mut commits: Vec<(&'static str, Vec<(&str, &str, &str)>)> = Vec::new();
    for i in 0..5 {
        let hash = Box::leak(format!("co{i:02}").into_boxed_str());
        commits.push((
            hash as &'static str,
            vec![("1", "1", "a.rb"), ("1", "1", "b.rb")],
        ));
    }
    for i in 0..12 {
        let hash = Box::leak(format!("sa{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "a.rb")]));
    }
    for i in 0..12 {
        let hash = Box::leak(format!("sb{i:02}").into_boxed_str());
        commits.push((hash as &'static str, vec![("1", "1", "b.rb")]));
    }
    // a: 17, b: 17, min = 17 → 5/17 = 0.2941176... → round(4) = 0.2941 < 0.30.
    let commits_ref: Vec<FakeCommit<'_>> =
        commits.iter().map(|(h, v)| (*h, v.as_slice())).collect();
    let log = fake_log(&commits_ref);
    let report = parse_log(&log, &files_set(&["a.rb", "b.rb"]));
    assert!(
        report.coupling.is_empty(),
        "5/17 = 0.2941 rounds to 0.2941 which is < 0.30 threshold, must drop"
    );
}
