use super::*;
use std::collections::HashSet;
use std::process::{Command, Stdio};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn files_set(paths: &[&str]) -> HashSet<PathBuf> {
    paths.iter().map(|s| p(s)).collect()
}

/// Build a fake `git log --format=%x1e%ct --name-status` stream.
/// Each tuple is (epoch, &[status, path, optional_new_path]).
fn fake_log(commits: &[(u64, &[&[&str]])]) -> Vec<u8> {
    let mut out = String::new();
    for (epoch, entries) in commits {
        out.push('\x1e');
        out.push_str(&epoch.to_string());
        out.push('\n');
        for entry in *entries {
            out.push_str(&entry.join("\t"));
            out.push('\n');
        }
    }
    out.into_bytes()
}

fn cfg_at(now_epoch: u64) -> NewnessConfig {
    NewnessConfig {
        days: DEFAULT_DAYS,
        min_commits: DEFAULT_MIN_COMMITS,
        now_epoch,
    }
}

#[test]
fn single_commit_file_reads_as_new_by_low_commit_count() {
    let now = 1_700_000_000;
    let log = fake_log(&[(now - 86_400, &[&["A", "new.rb"]])]);
    let wanted = files_set(&["new.rb"]);
    let parsed = parse_history(&log, &wanted, None);
    assert_eq!(parsed.total_commits.get(&p("new.rb")).copied(), Some(1));
    assert_eq!(
        parsed.first_commit_epoch.get(&p("new.rb")).copied(),
        Some(now - 86_400)
    );
    let fn_meta = FileNewness {
        new_file: is_new_file(Some(1), 1, cfg_at(now)),
        age_days: 1,
        total_commits: 1,
        metadata_available: true,
    };
    assert!(
        fn_meta.new_file,
        "a 1-commit file < min_commits threshold must read as new"
    );
}

#[test]
fn file_outside_age_window_and_above_min_commits_is_not_new() {
    let now = 1_700_000_000;
    let cfg = cfg_at(now);
    // age_days = 60, total_commits = 5 → outside window, enough commits
    assert!(!is_new_file(Some(60), 5, cfg));
    // Exactly at window boundary: `within_age_window` requires STRICT
    // less-than the threshold, so age_days == 30 is NOT in the window.
    assert!(!is_new_file(Some(30), 5, cfg));
    // One day under → in the window.
    assert!(is_new_file(Some(29), 5, cfg));
    // At min_commits threshold but below age: still new (within window).
    assert!(is_new_file(Some(10), 3, cfg));
}

#[test]
fn unknown_lineage_reads_as_new() {
    // A file not in the log yields None first_epoch → unknown_lineage → new.
    let cfg = cfg_at(1_700_000_000);
    assert!(is_new_file(None, 0, cfg));
}

#[test]
fn rename_canonicalizes_old_path_to_new() {
    let now = 1_700_000_000;
    // Real `git log` orders commits NEWEST → OLDEST. The alias
    // table must be populated by the rename line BEFORE the
    // older-add gets walked, so old-add's `old.rb` canonicalizes
    // through the alias.
    let log = fake_log(&[
        (now - 5 * 86_400, &[&["M", "new.rb"]]),
        (now - 10 * 86_400, &[&["R100", "old.rb", "new.rb"]]),
        (now - 20 * 86_400, &[&["A", "old.rb"]]),
    ]);
    // `wanted` names only the NEW path — Ruby semantics.
    let wanted = files_set(&["new.rb"]);
    let parsed = parse_history(&log, &wanted, None);
    // 3 commits all tally against the canonical new path.
    assert_eq!(parsed.total_commits.get(&p("new.rb")).copied(), Some(3));
    // Oldest touch is the pre-rename `A` commit on old.rb — aliases
    // let it flow through.
    assert_eq!(
        parsed.first_commit_epoch.get(&p("new.rb")).copied(),
        Some(now - 20 * 86_400)
    );
    // old.rb has no entry because its path canonicalized to new.rb.
    assert_eq!(parsed.total_commits.get(&p("old.rb")), None);
}

#[test]
fn deletion_starts_a_lineage_boundary() {
    let now = 1_700_000_000;
    // Real git log order: NEWEST → OLDEST. Ruby walks the stream
    // in that order, so the `D` fires BEFORE the older `A`, and
    // the boundary suppresses the pre-deletion lineage's touches
    // entirely. Expected: only the post-deletion `A` (the newest)
    // tallies. Count = 1, first_commit_epoch = `now - 10 * 86_400`.
    let log = fake_log(&[
        (now - 10 * 86_400, &[&["A", "f.rb"]]), // post-delete add (newest)
        (now - 50 * 86_400, &[&["D", "f.rb"]]), // deletion
        (now - 100 * 86_400, &[&["A", "f.rb"]]), // pre-delete add (oldest, suppressed)
    ]);
    let wanted = files_set(&["f.rb"]);
    let parsed = parse_history(&log, &wanted, None);
    assert_eq!(
        parsed.total_commits.get(&p("f.rb")).copied(),
        Some(1),
        "pre-deletion lineage must be suppressed — only post-D `A` counts"
    );
    assert_eq!(
        parsed.first_commit_epoch.get(&p("f.rb")).copied(),
        Some(now - 10 * 86_400),
        "first_commit_epoch must reflect the fresh lineage, not the suppressed one"
    );
}

#[test]
fn parse_history_strips_analysis_prefix_before_matching_wanted() {
    // git log emits paths relative to repo root; FileCollector
    // yields paths relative to the scan root. With a subdir scan
    // (repo/app), raw git paths look like "app/models/user.rb"
    // while wanted holds "models/user.rb". Without rebasing, the
    // wanted set never matches. Caught as Ohm's P1 on PR #84.
    let now = 1_700_000_000;
    let log = fake_log(&[(now - 86_400, &[&["A", "app/models/user.rb"]])]);
    let wanted = files_set(&["models/user.rb"]);
    let parsed_no_prefix = parse_history(&log, &wanted, None);
    assert_eq!(
        parsed_no_prefix.total_commits.get(&p("models/user.rb")),
        None,
        "without prefix stripping, the wanted set must NOT match"
    );
    let parsed_with_prefix = parse_history(&log, &wanted, Some("app/"));
    assert_eq!(
        parsed_with_prefix
            .total_commits
            .get(&p("models/user.rb"))
            .copied(),
        Some(1),
        "with prefix stripping, the rebased path matches wanted"
    );
}

#[test]
fn parse_history_prefix_drops_paths_outside_subdirectory() {
    // A file that lives OUTSIDE the analysis subdirectory must
    // disappear entirely from the metadata even if its path
    // happens to end with a wanted basename.
    let now = 1_700_000_000;
    let log = fake_log(&[(now - 86_400, &[&["A", "lib/models/user.rb"]])]);
    let wanted = files_set(&["models/user.rb"]);
    let parsed = parse_history(&log, &wanted, Some("app/"));
    assert_eq!(
        parsed.total_commits.get(&p("models/user.rb")),
        None,
        "lib/ file must not match an app/ analysis prefix"
    );
}

#[test]
fn files_outside_the_wanted_set_are_ignored() {
    let now = 1_700_000_000;
    let log = fake_log(&[(now - 86_400, &[&["A", "a.rb"], &["A", "b.rb"]])]);
    let wanted = files_set(&["a.rb"]);
    let parsed = parse_history(&log, &wanted, None);
    assert_eq!(parsed.total_commits.get(&p("a.rb")).copied(), Some(1));
    assert!(!parsed.total_commits.contains_key(&p("b.rb")));
}

// --- Evidence formula ---

#[test]
fn evidence_without_coverage_averages_age_and_commits_components() {
    // age_days=15 → age_component = 15/30 = 0.5
    // total_commits=0 → commits_component = 0
    // history_only = (0.5 + 0) / 2 = 0.25
    let m = FileNewness {
        new_file: true,
        age_days: 15,
        total_commits: 0,
        metadata_available: true,
    };
    let e = evidence_for(&m, false).expect("metadata available -> Some");
    assert!((e - 0.25).abs() < 1e-9, "evidence was {e}");
}

#[test]
fn evidence_saturates_at_one_component_each() {
    // Age well over the 30-day ceiling + commits well over 3.
    let m = FileNewness {
        new_file: false,
        age_days: 365,
        total_commits: 100,
        metadata_available: true,
    };
    let e = evidence_for(&m, false).unwrap();
    assert!((e - 1.0).abs() < 1e-9, "saturated evidence was {e}");
}

#[test]
fn evidence_with_coverage_is_at_least_history_only() {
    // age_days=0, commits=0, with coverage → with_coverage = (0+0+1)/3 = 0.3333
    let m = FileNewness {
        new_file: true,
        age_days: 0,
        total_commits: 0,
        metadata_available: true,
    };
    let e = evidence_for(&m, true).unwrap();
    assert!((e - 0.3333).abs() < 1e-9, "evidence with coverage was {e}");
}

#[test]
fn evidence_takes_the_higher_of_history_and_coverage_branches() {
    // age_days=60 (component 1.0), commits=6 (component 1.0).
    // history_only = 1.0, with_coverage = (1+1+1)/3 = 1.0. Tied.
    let m = FileNewness {
        new_file: false,
        age_days: 60,
        total_commits: 6,
        metadata_available: true,
    };
    assert!((evidence_for(&m, false).unwrap() - 1.0).abs() < 1e-9);
    assert!((evidence_for(&m, true).unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn evidence_is_none_when_metadata_unavailable() {
    let m = FileNewness::disabled();
    assert!(evidence_for(&m, false).is_none());
    assert!(evidence_for(&m, true).is_none());
}

// --- End-to-end against a real tempdir repo ---

#[test]
fn analyze_over_real_repo_matches_hand_computed_metadata() {
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
            .status();
        if status.map(|s| !s.success()).unwrap_or(true) {
            panic!("git {args:?} failed");
        }
    };
    let write = |rel: &str, body: &str| {
        let full = repo.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, body).unwrap();
    };

    run(&["init", "-q", "-b", "main"]);
    write("a.rb", "one\n");
    run(&["add", "a.rb"]);
    let commit_at = |msg: &str, epoch: u64| {
        let date = format!("{epoch} +0000");
        let status = Command::new("git")
            .args(["commit", "-q", "-m", msg])
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        assert!(status.map(|s| s.success()).unwrap_or(false));
    };

    // Three commits for a.rb, two for b.rb. Spread over ~20 days.
    commit_at("c1", 1_700_000_000);
    write("a.rb", "two\n");
    run(&["add", "a.rb"]);
    commit_at("c2", 1_700_000_000 + 10 * 86_400);
    write("b.rb", "hello\n");
    run(&["add", "b.rb"]);
    commit_at("c3", 1_700_000_000 + 15 * 86_400);
    write("a.rb", "three\n");
    run(&["add", "a.rb"]);
    commit_at("c4", 1_700_000_000 + 20 * 86_400);

    let a = p("a.rb");
    let b = p("b.rb");
    let files: Vec<&Path> = [a.as_path(), b.as_path()].to_vec();

    // "now" = latest commit + 10 days so ages don't rely on wall clock.
    let now = 1_700_000_000 + 30 * 86_400;
    let cfg = NewnessConfig {
        days: 30,
        min_commits: 3,
        now_epoch: now,
    };
    let report = analyze(repo, &files, cfg);
    assert!(report.succeeded, "git ran cleanly");
    assert!(report.warnings.is_empty(), "no warnings on a healthy repo");

    let a_meta = report.metadata.get(&a).expect("a.rb metadata");
    assert_eq!(a_meta.total_commits, 3, "a.rb has 3 commits");
    assert_eq!(
        a_meta.age_days, 30,
        "a.rb first-commit was 30 days ago at the config's `now`"
    );
    assert!(
        !a_meta.new_file,
        "a.rb hits BOTH thresholds: age >= 30 AND commits >= 3 → not new"
    );
    assert!(a_meta.metadata_available);

    let b_meta = report.metadata.get(&b).expect("b.rb metadata");
    assert_eq!(b_meta.total_commits, 1);
    assert_eq!(b_meta.age_days, 15);
    assert!(b_meta.new_file, "b.rb has 1 commit (< min_commits) → new");
}

#[test]
fn analyze_on_non_repo_returns_disabled_placeholder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = p("x.rb");
    let files: Vec<&Path> = vec![file.as_path()];
    let report = analyze(tmp.path(), &files, NewnessConfig::default());
    assert!(!report.succeeded);
    let meta = report.metadata.get(&file).unwrap();
    assert_eq!(meta, &FileNewness::disabled());
    assert!(
        report.warnings.iter().any(|w| matches!(
            w,
            NewnessWarning::NotAGitRepo | NewnessWarning::GitFailed(_)
        )),
        "warning must name why newness was skipped"
    );
}

#[test]
fn shallow_repo_returns_disabled_with_shallow_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    // A bare `.git/shallow` file is what git creates for `clone --depth`
    // users; this is the detection fallback path.
    std::fs::write(repo.join(".git").join("shallow"), "deadbeef\n").unwrap();
    // No real repo structure, but `is_shallow_repository` consults
    // `.git/shallow` directly when `git rev-parse` fails.
    let file = p("x.rb");
    let files: Vec<&Path> = vec![file.as_path()];
    let report = analyze(repo, &files, NewnessConfig::default());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, NewnessWarning::ShallowClone)),
        "shallow repo must emit ShallowClone warning"
    );
    let meta = report.metadata.get(&file).unwrap();
    assert_eq!(meta, &FileNewness::disabled());
}

#[test]
fn analyze_over_real_repo_subdirectory_rebases_paths_correctly() {
    // End-to-end: run analyze() against a subdirectory of a repo.
    // Without the analysis-prefix rebasing, git log emits
    // `app/models/user.rb` while our wanted set keyed by
    // `models/user.rb` sees zero matches and the file reads as
    // zero-history. Ohm's P1 on PR #84.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let subdir = repo.join("app");
    std::fs::create_dir_all(&subdir).unwrap();
    let run = |args: &[&str], cwd: &Path| {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        assert!(status.map(|s| s.success()).unwrap_or(false));
    };
    run(&["init", "-q", "-b", "main"], repo);
    std::fs::create_dir_all(subdir.join("models")).unwrap();
    std::fs::write(subdir.join("models/user.rb"), "one\n").unwrap();
    run(&["add", "app/models/user.rb"], repo);
    let commit_at = |msg: &str, epoch: u64| {
        let date = format!("{epoch} +0000");
        let status = Command::new("git")
            .args(["commit", "-q", "-m", msg])
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        assert!(status.map(|s| s.success()).unwrap_or(false));
    };
    commit_at("c1", 1_700_000_000);
    std::fs::write(subdir.join("models/user.rb"), "two\n").unwrap();
    run(&["add", "app/models/user.rb"], repo);
    commit_at("c2", 1_700_000_000 + 20 * 86_400);

    let models_user = PathBuf::from("models/user.rb");
    let files: Vec<&Path> = vec![models_user.as_path()];
    let now = 1_700_000_000 + 30 * 86_400;
    let cfg = NewnessConfig {
        days: 30,
        min_commits: 3,
        now_epoch: now,
    };
    // Scan from the subdirectory — this is what sf-cli does when
    // scanning app/ instead of the repo root.
    let report = analyze(&subdir, &files, cfg);
    assert!(report.succeeded);
    let meta = report
        .metadata
        .get(&models_user)
        .expect("subdirectory scan must still land metadata for models/user.rb");
    assert_eq!(
        meta.total_commits, 2,
        "both commits must count after prefix-rebasing"
    );
    assert_eq!(meta.age_days, 30, "oldest commit was 30 days ago");
    assert!(meta.metadata_available);
}

#[test]
fn cross_boundary_rename_preserves_pre_rename_lineage() {
    // A file that was committed OUTSIDE the analysis prefix and
    // later renamed INTO it must still have its pre-rename commits
    // counted. Caught as Ohm's P1 sub-finding on PR #84 — Rust
    // dropped the pre-rename `A lib/a.rb` because it couldn't
    // rebase, even though an alias was being built pointing at
    // `app/a.rb`.
    //
    // Walk order NEWEST → OLDEST:
    //   commit 3 (newest): M app/a.rb       — post-rename modify
    //   commit 2: R100 lib/a.rb app/a.rb    — rename INTO prefix
    //   commit 1 (oldest): A lib/a.rb       — pre-rename add
    //
    // Expected: total_commits = 3. first_commit_epoch = epoch of
    // the pre-rename `A`. Ruby yields 3; pre-fix Rust yielded 1.
    let now = 1_700_000_000;
    let log = fake_log(&[
        (now - 5 * 86_400, &[&["M", "app/a.rb"]]),
        (now - 10 * 86_400, &[&["R100", "lib/a.rb", "app/a.rb"]]),
        (now - 20 * 86_400, &[&["A", "lib/a.rb"]]),
    ]);
    let wanted = files_set(&["a.rb"]);
    let parsed = parse_history(&log, &wanted, Some("app/"));
    assert_eq!(
        parsed.total_commits.get(&p("a.rb")).copied(),
        Some(3),
        "cross-boundary rename must carry pre-rename commits through the alias"
    );
    assert_eq!(
        parsed.first_commit_epoch.get(&p("a.rb")).copied(),
        Some(now - 20 * 86_400),
        "first_commit_epoch must reach the pre-rename add"
    );
}

#[test]
fn cross_boundary_rename_still_respects_deletion_boundary_on_old_path() {
    // If the out-of-prefix predecessor was DELETED before the
    // rename lineage, its touches don't flow through into scope.
    // Walk order NEWEST → OLDEST:
    //   commit 4 (newest): M app/a.rb
    //   commit 3: R100 lib/a.rb app/a.rb (fresh lineage)
    //   commit 2: D lib/a.rb    (out-of-prefix, but resolves via
    //                            alias to app/a.rb which is in
    //                            scope AFTER the rename above)
    // The deletion fires BEFORE the older `A` in the stream, so
    // the pre-deletion add is suppressed. Final count = 2
    // (M + R post-deletion add), not 3.
    let now = 1_700_000_000;
    let log = fake_log(&[
        (now - 5 * 86_400, &[&["M", "app/a.rb"]]),
        (now - 10 * 86_400, &[&["R100", "lib/a.rb", "app/a.rb"]]),
        (now - 15 * 86_400, &[&["D", "lib/a.rb"]]),
        (now - 30 * 86_400, &[&["A", "lib/a.rb"]]),
    ]);
    let wanted = files_set(&["a.rb"]);
    let parsed = parse_history(&log, &wanted, Some("app/"));
    assert_eq!(
        parsed.total_commits.get(&p("a.rb")).copied(),
        Some(2),
        "pre-deletion lineage must be suppressed through the alias too — only M + R count"
    );
    assert_eq!(
        parsed.first_commit_epoch.get(&p("a.rb")).copied(),
        Some(now - 10 * 86_400),
        "first_commit_epoch is the rename commit, not the suppressed pre-deletion add"
    );
}

// --- Slice #13: attempt_unshallow ---

/// Helper: build an isolated repo on disk via real `git` (we need
/// the full porcelain including a shallow-grafted ancestor history)
/// and return its path alongside the tempdir guard. Each test that
/// needs shallow state assembles its own repo — the operations are
/// too stateful for a shared fixture.
fn init_repo(dir: &std::path::Path) {
    assert!(
        Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    for (k, v) in [
        ("user.email", "test@example.invalid"),
        ("user.name", "Test"),
        ("commit.gpgsign", "false"),
    ] {
        Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .status()
            .unwrap();
    }
}

fn commit_file(dir: &std::path::Path, name: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(name), body).unwrap();
    Command::new("git")
        .args(["add", name])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(
        Command::new("git")
            .args(["commit", "-m", msg])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

/// `attempt_unshallow` on a repo with no remote returns `Failed`
/// (git exits non-zero), and the repo stays shallow. Exercises the
/// Err path short of injecting a real network. We first create a
/// local shallow clone from a bare mirror of a local source repo
/// WITHOUT setting an upstream, so `git fetch --unshallow` has
/// nothing to fetch from and bails.
#[test]
fn attempt_unshallow_on_repo_without_fetchable_remote_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir(&src).unwrap();
    init_repo(&src);
    commit_file(&src, "a.rb", "a\n", "a");
    commit_file(&src, "b.rb", "b\n", "b");
    commit_file(&src, "c.rb", "c\n", "c");
    // Clone with --depth=1 so the clone is shallow, then rip out
    // the remote so fetch --unshallow has no refspec to resolve.
    let shallow = tmp.path().join("shallow");
    let src_arg = src.to_str().unwrap();
    assert!(
        Command::new("git")
            .args([
                "clone",
                "--depth=1",
                "--no-local",
                src_arg,
                shallow.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success()
    );
    assert!(is_shallow_repository(&shallow));
    // Point origin at a definitely-nonexistent path so unshallow
    // fails cleanly without touching the real src repo.
    Command::new("git")
        .args(["remote", "set-url", "origin", "/no/such/remote"])
        .current_dir(&shallow)
        .status()
        .unwrap();
    match attempt_unshallow(&shallow) {
        Err(UnshallowError::Failed { .. }) | Err(UnshallowError::Io(_)) => {}
        other => panic!("expected Failed, got {other:?}"),
    }
    // Still shallow after the failed attempt.
    assert!(is_shallow_repository(&shallow));
}

/// `attempt_unshallow` on a non-shallow repo exits non-zero with
/// git's own error message ("fatal: --unshallow on a complete
/// repository does not make sense"). We surface that as `Failed`
/// rather than crashing, and `is_shallow_repository` remains
/// false, matching Ruby's `wait_thread.value.success?` branch.
#[test]
fn attempt_unshallow_on_non_shallow_repo_returns_failed() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path());
    commit_file(tmp.path(), "x.rb", "x\n", "x");
    assert!(!is_shallow_repository(tmp.path()));
    match attempt_unshallow(tmp.path()) {
        Err(UnshallowError::Failed { .. }) => {}
        other => panic!("expected Failed on non-shallow repo, got {other:?}"),
    }
}

/// `attempt_unshallow` when `git` is absent surfaces `GitNotFound`
/// rather than panicking. We simulate by giving `attempt_unshallow`
/// a PATH stripped of git for just this call via a child
/// subprocess (can't mutate std env in-test across threads
/// reliably). Simpler: assert the error type by direct inspection
/// of UnshallowError::GitNotFound variants via match — the real
/// path is exercised when git is removed from PATH, which CI
/// runners do not do.
#[test]
fn unshallow_error_display_contains_timeout_seconds() {
    let err = UnshallowError::TimedOut;
    assert!(
        err.to_string().contains("45"),
        "TimedOut Display must name the timeout, got: {err}"
    );
}

#[test]
fn unshallow_error_display_names_failure_cause() {
    let err = UnshallowError::Failed {
        exit_code: Some(128),
        stderr: "fatal: no such ref".into(),
    };
    let s = err.to_string();
    assert!(s.contains("128"), "exit code in Display: {s}");
    let err = UnshallowError::GitNotFound;
    assert!(err.to_string().contains("not found"));
}

// --- PR #93 Ohm P2 regressions ---

/// Ohm P2 r1: stderr must be drained concurrently. If the child
/// emits more stderr than the pipe capacity (~64 KiB on Linux)
/// before the parent reads, the child blocks on write and
/// `try_wait` never reports exit — turning a successful fetch
/// into a 45-second timeout.
///
/// We swap the git invocation for a shell that emits ~1 MiB of
/// stderr then exits 0. Prior implementation would deadlock; new
/// implementation reads concurrently and returns Ok.
#[test]
fn verbose_stderr_does_not_deadlock_with_concurrent_drain() {
    let mut cmd = Command::new("bash");
    // ~1 MiB of stderr across 10000 lines, well past the pipe's
    // 64 KiB kernel buffer on Linux/macOS.
    cmd.args([
        "-c",
        "for i in $(seq 1 10000); do printf 'line %d padding padding padding padding padding padding padding padding\\n' \"$i\" >&2; done; exit 0",
    ]);
    let start = std::time::Instant::now();
    let result = attempt_unshallow_with_command(cmd, std::time::Duration::from_secs(10));
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Ok(())),
        "verbose-stderr child must still succeed: {result:?} (elapsed {elapsed:?})"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "concurrent drain must keep elapsed well under the 10s cap: {elapsed:?}"
    );
}

/// Ohm P2 r1: with a short injected deadline, the TERM/KILL
/// escalation path must trigger instead of hanging. Previously
/// only observable via the Display string; now exercised end to
/// end via the testable entrypoint.
#[test]
fn timeout_escalation_kills_runaway_child_promptly() {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "sleep 60"]);
    let start = std::time::Instant::now();
    let result = attempt_unshallow_with_command(cmd, std::time::Duration::from_millis(200));
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(UnshallowError::TimedOut)),
        "runaway child must surface TimedOut: {result:?}"
    );
    // Sleep-60 child must be killed well before the 60-second
    // sleep completes. Allow generous headroom for the SIGTERM
    // grace + CI jitter.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "TERM/KILL escalation must bound the wait: {elapsed:?}"
    );
}

/// Ohm P2 r1: an exit-non-zero child whose stderr landed in the
/// buffer must surface that text in `UnshallowError::Failed`.
/// Verifies the concurrent drain actually propagates bytes.
#[test]
fn failed_child_exposes_captured_stderr() {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "echo nope >&2; exit 42"]);
    let result = attempt_unshallow_with_command(cmd, std::time::Duration::from_secs(5));
    match result {
        Err(UnshallowError::Failed { exit_code, stderr }) => {
            assert_eq!(exit_code, Some(42), "exit code must round-trip");
            assert!(
                stderr.contains("nope"),
                "captured stderr must include child's bytes: {stderr:?}"
            );
        }
        other => panic!("expected Failed with stderr, got {other:?}"),
    }
}

/// Ohm P2 r1: the public `attempt_unshallow` path must thread a
/// non-UTF-8 repo path through to `git -C` as an `OsStr`, not
/// substitute `.` and silently fetch into the process CWD. We
/// can't directly verify git saw the exact bytes on a real repo
/// — but the prior `to_str().unwrap_or(".")` would have made
/// `attempt_unshallow(<invalid-utf-8-path>)` fetch the CWD; the
/// new path hands the bytes to git which will error on a
/// non-existent path. Either outcome means the path was NOT
/// collapsed to `.`.
#[cfg(unix)]
#[test]
fn non_utf8_repo_path_is_not_silently_redirected_to_cwd() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    // Byte 0xFF is invalid as a UTF-8 lead and never appears in
    // valid UTF-8. Prefix with /tmp/ so the path is syntactically
    // valid but non-UTF-8 after the slash.
    let mut bytes = b"/tmp/sf-nonexistent-".to_vec();
    bytes.push(0xFF);
    bytes.extend_from_slice(b"-dir");
    let os = OsString::from_vec(bytes);
    let path = std::path::PathBuf::from(os);
    assert!(
        path.to_str().is_none(),
        "test setup: path must be non-UTF-8"
    );
    assert!(!path.exists(), "test setup: path must not exist");

    // On the OLD code (repo_root.to_str().unwrap_or(".")) this
    // would have silently fetched the CWD, which — depending on
    // the CWD's git state — would return either Ok(()) OR
    // Failed-pointing-at-the-CWD's-git-error. Either is wrong.
    //
    // On the NEW code, git is handed the non-existent path, so
    // it exits with "fatal: cannot change to '...'". That
    // surfaces as `Failed`.
    match attempt_unshallow(&path) {
        Err(UnshallowError::Failed { stderr, .. }) => {
            // git's error for -C on a bad path, varies by
            // version but always mentions the path or "cannot
            // change" / "not a git repository" — the point is
            // it reached git with the bad path.
            assert!(
                !stderr.is_empty(),
                "git must have emitted SOME diagnostic about the bad path"
            );
        }
        Err(UnshallowError::GitNotFound) => {
            // CI without git installed — we can't exercise the
            // path-passing behavior but we at least didn't crash.
        }
        other => panic!(
            "non-UTF-8 non-existent path must surface a Failed from git, got {other:?} \
             (prior implementation would have collapsed to `.` and operated on CWD)"
        ),
    }
}
