//! End-to-end: `--output json|csv|markdown`, `--top`, `--diff-base`.
//! Each test builds a tempdir Ruby mini-repo (optionally + git) and
//! verifies the flag's shape, not exact strings.

use std::process::Command;

fn stud_finder_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stud-finder")
}

fn write_ruby_mini_repo(root: &std::path::Path) {
    std::fs::write(
        root.join("order.rb"),
        "class Order\n  def process(x)\n    Payment.capture if x\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(
        root.join("payment.rb"),
        "class Payment\n  def self.capture; end\nend\n",
    )
    .unwrap();
    std::fs::write(
        root.join("user.rb"),
        "class User\n  def ship\n    Order.new.process(true)\n  end\nend\n",
    )
    .unwrap();
}

#[test]
fn output_json_emits_ruby_parity_root_and_full_row_shape() {
    // PR #85 (sf-output-parity slice #10a): JSON root is
    // `{meta, warnings, ruby, javascript}` — ALL four keys always
    // present. Per-row field names match Ruby `json_file`:
    // `score` not `composite`, `class` not `classification`.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    // Ruby-parity root keys.
    for k in ["meta", "warnings", "ruby", "javascript"] {
        assert!(v.get(k).is_some(), "JSON root missing `{k}`: {stdout}");
    }
    let meta = v.get("meta").unwrap().as_object().unwrap();
    assert_eq!(meta.get("schema_version"), Some(&serde_json::json!(1)));
    assert!(meta.contains_key("repo"));
    assert!(meta.contains_key("analyzed_at"));
    assert!(meta.contains_key("churn_days"));
    assert!(meta.contains_key("file_count"));

    let ruby = v.get("ruby").unwrap().as_array().unwrap();
    assert!(ruby.len() >= 2, "expected 2+ rows: {ruby:?}");

    // Ruby-shape row fields. Note: `composite` must NOT be present
    // (it's `score` in the Ruby contract); `classification` must
    // NOT be present (it's `class`).
    let required = [
        "rank",
        "language",
        "path",
        "score",
        "evidence",
        "class",
        "new_file",
        "age_days",
        "escalation",
        "fan_in",
        "fan_in_pct",
        "fan_out",
        "fan_out_pct",
        "instability",
        "instability_pct",
        "complexity",
        "complexity_pct",
        "churn_commits",
        "churn_lines",
        "churn_pct",
        "loc",
        "loc_pct",
        "max_coupling",
        "max_coupling_partner",
        "coupling_partners",
        "coupling_pct",
        "coverage",
    ];
    let forbidden = ["composite", "classification"];
    for row in ruby {
        for k in &required {
            assert!(row.get(k).is_some(), "row missing `{k}`: {row:?}");
        }
        for k in &forbidden {
            assert!(
                row.get(k).is_none(),
                "row must NOT carry pre-parity key `{k}`: {row:?}"
            );
        }
        assert_eq!(row.get("language"), Some(&serde_json::json!("ruby")));
    }
}

#[test]
fn output_csv_emits_single_combined_stream_with_ruby_result_columns_header() {
    // PR #85: CSV is now a single combined ruby+javascript stream
    // with Ruby's RESULT_COLUMNS header (starts with
    // rank,language,file,score,...). The old `# Ruby` / `# JavaScript`
    // split is gone.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("# Ruby"),
        "CSV must not include the old `# Ruby` section header: {stdout}"
    );
    let mut lines = stdout.lines();
    let header = lines.next().expect("header");
    assert!(
        header.starts_with("rank,language,file,score,evidence,class"),
        "header does not match Ruby RESULT_COLUMNS: {header}"
    );
    let data_rows: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert!(data_rows.len() >= 2, "expected 2+ data rows: {stdout}");
    // Every data row carries the language column as the second cell.
    for row in &data_rows {
        let second = row.split(',').nth(1).unwrap_or("");
        assert!(
            second == "ruby" || second == "javascript",
            "second cell must be the language: {row}"
        );
    }
}

#[test]
fn output_markdown_emits_ruby_parity_shape() {
    // Slice #10b: markdown now matches Ruby `emit_markdown` —
    // `## stud-finder — YYYY-MM-DD` title, `> formula` blockquote,
    // `### Ruby` + `### JavaScript/TypeScript` section tables with
    // the Ruby `MARKDOWN_COLUMNS` 22-column subset, closing with
    // the fan_in disclaimer.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("markdown")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("## stud-finder — "),
        "markdown should start with Ruby-parity title: {stdout}"
    );
    assert!(
        stdout.contains("> 4-factor. Churn window: 180 days."),
        "formula + churn blockquote missing: {stdout}"
    );
    assert!(
        stdout.contains("### Ruby"),
        "Ruby section header missing: {stdout}"
    );
    assert!(
        stdout.contains("### JavaScript/TypeScript"),
        "JS/TS section header missing: {stdout}"
    );
    assert!(
        stdout.contains("| rank | language | file | score |"),
        "22-column MARKDOWN_COLUMNS header missing: {stdout}"
    );
    assert!(
        stdout.contains("*fan_in is a static approximation"),
        "fan_in disclaimer footer missing: {stdout}"
    );
    // Count Ruby section data rows — lines inside the Ruby table
    // with a `.rb` path cell.
    let data_row_count = stdout
        .lines()
        .filter(|l| l.starts_with("| ") && l.contains(".rb") && !l.contains("---"))
        .count();
    assert!(data_row_count >= 2, "expected 2+ Ruby data rows: {stdout}");
}

#[test]
fn top_limits_rows_per_section() {
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    // CSV format for a tight shape — each data row is one line with
    // language in the second column. Header gets filtered out.
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--top")
        .arg("1")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let _header = lines.next();
    let data_rows: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert_eq!(data_rows.len(), 1, "--top 1 should keep 1 row: {stdout}");
}

#[test]
fn top_zero_is_rejected_matching_ruby() {
    // Ruby `--top 0` prints `--top must be positive` and exits
    // non-zero. We match that so automation that passes 0 by mistake
    // fails loudly rather than getting the "unlimited" surprise.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--top")
        .arg("0")
        .output()
        .unwrap();
    assert!(!out.status.success(), "--top 0 must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--top must be positive"),
        "expected `--top must be positive` on stderr: {stderr}"
    );
}

#[test]
fn markdown_escapes_pipes_and_newlines_in_paths() {
    // Non-blocking from Ohm's PR #82 review: a path containing `|`
    // or newlines must not break the pipe-table row. Updated for
    // slice #10b: header is now `| rank | language | file | ...`
    // (22 cols — the Ruby MARKDOWN_COLUMNS subset).
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("weird|dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("order.rb"), "class Order\nend\n").unwrap();
    std::fs::write(tmp.path().join("other.rb"), "class Other\nend\n").unwrap();

    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("markdown")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let header_line = stdout
        .lines()
        .find(|l| l.starts_with("| rank |"))
        .expect("header");
    let header_pipe_count = header_line.matches('|').count();
    let data_rows: Vec<&str> = stdout
        .lines()
        .filter(|l| l.ends_with(" |") && l.contains(".rb") && !l.contains("---"))
        .collect();
    assert!(
        data_rows.len() >= 2,
        "test fixture must produce at least 2 data rows: {stdout}"
    );
    for row in &data_rows {
        assert_eq!(
            row.matches('|').count(),
            header_pipe_count,
            "row pipe count differs from header; unescaped `|` in path? row={row:?}"
        );
    }
    assert!(
        stdout.contains("weird&#124;dir"),
        "expected HTML-entity-escaped pipe in path: {stdout}"
    );
}

#[test]
fn csv_escapes_pipe_and_comma_in_paths() {
    // RFC 4180 double-quoted + inner-quote doubled.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("a,b");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("order.rb"), "class Order\nend\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(r#""a,b/order.rb""#),
        "expected RFC4180-quoted path in CSV: {stdout}"
    );
}

#[test]
fn absent_top_keeps_all_rows() {
    // CSV for a tight shape — each data row is one line.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let baseline = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(baseline.status.success());
    let stdout = String::from_utf8_lossy(&baseline.stdout);
    let mut lines = stdout.lines();
    let _header = lines.next();
    let row_count = lines.filter(|l| !l.is_empty()).count();
    assert_eq!(row_count, 3, "baseline should emit all 3 Ruby files");
}

#[test]
fn diff_base_filters_to_changed_files_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);

    // Baseline commit: 3 Ruby files.
    write_ruby_mini_repo(root);
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "base"]);

    // Branch off, touch only order.rb.
    run_git(&["checkout", "-b", "feature"]);
    std::fs::write(
        root.join("order.rb"),
        "class Order\n  def process(x)\n    Payment.capture if x\n    :changed\n  end\nend\n",
    )
    .unwrap();
    run_git(&["commit", "-qam", "touch order"]);

    let out = Command::new(stud_finder_bin())
        .arg(root)
        .arg("--diff-base")
        .arg("main")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let _header = lines.next();
    let file_lines: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert_eq!(
        file_lines.len(),
        1,
        "expected exactly 1 changed file row: {stdout}"
    );
    assert!(
        file_lines[0].contains("order.rb"),
        "expected order.rb row: {}",
        file_lines[0]
    );
}

#[test]
fn diff_base_on_non_git_dir_exits_nonzero() {
    // Matches Ruby `Diff` behavior — a non-repo path with
    // --diff-base is a hard failure, not a silent "no filter".
    // Automation relying on PR-only output should fail loudly.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--diff-base")
        .arg("main")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "non-git --diff-base must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--diff-base failed"),
        "expected failure message on stderr: {stderr}"
    );
}

#[test]
fn diff_base_bad_ref_exits_nonzero() {
    // Bad ref against a real repo — should fail, not fall through.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let run_git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    write_ruby_mini_repo(root);
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "x"]);

    let out = Command::new(stud_finder_bin())
        .arg(root)
        .arg("--diff-base")
        .arg("not-a-real-ref-nope")
        .output()
        .unwrap();
    assert!(!out.status.success(), "bad ref must exit non-zero");
}

#[test]
fn diff_base_nested_scan_root_resolves_paths_correctly() {
    // Ohm P1 on PR #82: when the scan root is a subdirectory of the
    // repo, scored rows use subdir-relative paths while
    // `git diff --name-only` (without --relative) emits repo-root
    // paths. The two don't match, so every changed row would be
    // dropped. Fixed by passing --relative to git, so the diff
    // paths come back in the same coordinate system.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success());
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);

    std::fs::create_dir_all(repo.join("lib/app")).unwrap();
    write_ruby_mini_repo(&repo.join("lib/app"));
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "base"]);

    run_git(&["checkout", "-b", "feature"]);
    std::fs::write(
        repo.join("lib/app/order.rb"),
        "class Order\n  def process(x)\n    Payment.capture if x\n    :changed\n  end\nend\n",
    )
    .unwrap();
    run_git(&["commit", "-qam", "touch order"]);

    // Scan the subdir, not the repo root. Scored rows are keyed by
    // `order.rb` (relative to lib/app). Without --relative, git
    // would emit `lib/app/order.rb` and the filter would drop it.
    let out = Command::new(stud_finder_bin())
        .arg(repo.join("lib/app"))
        .arg("--diff-base")
        .arg("main")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "nested-scan --diff-base must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let _header = lines.next();
    let file_lines: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert_eq!(
        file_lines.len(),
        1,
        "expected exactly 1 changed file in nested scan: {stdout}"
    );
    assert!(file_lines[0].contains("order.rb"));
}

#[test]
fn top_alone_does_not_set_meta_filtered() {
    // Ohm P2 on PR #86: Ruby `meta.filtered` is set ONLY when
    // `filter_set` is populated (diff-base / --only). `--top` is a
    // truncation knob, not a filter, so meta.filtered must stay
    // absent when --top is the only flag set.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("json")
        .arg("--top")
        .arg("1")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let meta = v.get("meta").unwrap().as_object().unwrap();
    assert!(
        meta.get("filtered").is_none(),
        "--top alone must NOT set meta.filtered: {meta:?}"
    );
}

#[test]
fn diff_base_does_set_meta_filtered() {
    // Companion to top_alone_does_not_set_meta_filtered: with a
    // real diff-base filter in play, meta.filtered MUST be present
    // and true.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success());
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    write_ruby_mini_repo(root);
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "base"]);
    run_git(&["checkout", "-b", "feature"]);
    std::fs::write(
        root.join("order.rb"),
        "class Order\n  def process(x)\n    Payment.capture if x\n    :changed\n  end\nend\n",
    )
    .unwrap();
    run_git(&["commit", "-qam", "touch order"]);

    let out = Command::new(stud_finder_bin())
        .arg(root)
        .arg("--output")
        .arg("json")
        .arg("--diff-base")
        .arg("main")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let meta = v.get("meta").unwrap().as_object().unwrap();
    assert_eq!(
        meta.get("filtered"),
        Some(&serde_json::json!(true)),
        "--diff-base must set meta.filtered: {meta:?}"
    );
    assert_eq!(
        meta.get("diff_base"),
        Some(&serde_json::json!("main")),
        "meta.diff_base must carry the ref: {meta:?}"
    );
}

#[test]
fn version_flag_prints_version_and_exits_zero() {
    // Ruby `--version` prints `StudFinder::VERSION` and exits 0.
    // The Rust port carries the same const; must match Ruby's
    // `lib/stud_finder/version.rb`.
    let out = Command::new(stud_finder_bin())
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success(), "--version must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0.7.0"),
        "--version must print VERSION: {stdout}"
    );
}

#[test]
fn deprecated_coverage_alias_emits_warning_and_still_loads() {
    // Ruby `--coverage PATH` is a deprecated alias for
    // `--ruby-coverage`. It must emit the `coverage_flag_deprecated`
    // warning (stderr + meta.warnings) and otherwise behave
    // identically.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    // Minimal SimpleCov resultset.json so the coverage reader
    // doesn't blow up; contents don't need to match any file.
    let cov_path = tmp.path().join("resultset.json");
    std::fs::write(&cov_path, r#"{"StudFinderTest": {"coverage": {}}}"#).unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--coverage")
        .arg(&cov_path)
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("coverage_flag_deprecated"),
        "deprecated --coverage must emit stderr warning: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let warnings = v.get("warnings").unwrap().as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.get("code") == Some(&serde_json::json!("coverage_flag_deprecated"))),
        "meta.warnings must carry coverage_flag_deprecated: {warnings:?}"
    );
}

#[test]
fn churn_days_flag_flows_into_meta_and_history_window() {
    // `--churn-days N` must land in meta.churn_days and also drive
    // the history window used by sf-history. A smoke-level check
    // that the flag isn't getting silently dropped.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--churn-days")
        .arg("30")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(
        v["meta"]["churn_days"],
        serde_json::json!(30),
        "churn-days must flow into meta: {}",
        v["meta"]
    );
}

#[test]
fn exclude_flag_drops_matching_paths_from_scored_set() {
    // `--exclude GLOB` is repeatable and applied after the built-in
    // default-rule globs. An excluded path should NOT appear in the
    // scored row set.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app")).unwrap();
    std::fs::write(
        tmp.path().join("app/keep.rb"),
        "class Keep\n  def one; end\nend\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("app/drop.rb"),
        "class Drop\n  def one; end\nend\n",
    )
    .unwrap();
    // Need a third file so FileCollector's small-set guard doesn't
    // trip (Rust port doesn't enforce the 5-file minimum yet, but
    // keep tests robust).
    std::fs::write(tmp.path().join("app/third.rb"), "class Third\nend\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--exclude")
        .arg("app/drop.rb")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("app/drop.rb"),
        "excluded path must not appear in output: {stdout}"
    );
    assert!(
        stdout.contains("app/keep.rb"),
        "non-excluded path must still appear: {stdout}"
    );
}

#[test]
fn default_exclude_globs_match_ruby_defaults() {
    // Ruby `FileCollector::DEFAULT_EXCLUDES` drops these paths
    // before any user `--exclude` runs. The Rust port must mirror
    // the same defaults; vendor/foo.rb should land in
    // default_excluded_count and NOT in the scored set.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("vendor")).unwrap();
    std::fs::create_dir_all(tmp.path().join("spec")).unwrap();
    std::fs::write(tmp.path().join("vendor/library.rb"), "class Lib\nend\n").unwrap();
    std::fs::write(tmp.path().join("spec/foo_spec.rb"), "describe 'x'\n").unwrap();
    std::fs::write(tmp.path().join("app.rb"), "class App\nend\n").unwrap();
    std::fs::write(tmp.path().join("another.rb"), "class Another\nend\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("vendor/library.rb"),
        "vendor/** must be default-excluded: {stdout}"
    );
    assert!(
        !stdout.contains("spec/foo_spec.rb"),
        "spec/** must be default-excluded: {stdout}"
    );
    assert!(stdout.contains("app.rb"));
}

#[test]
fn only_flag_narrows_to_matching_paths_and_sets_meta() {
    // Ruby `--only paths` emits only those rows but still scores
    // the full repo. meta.only_paths must carry the list and
    // meta.filtered must be true.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--only")
        .arg("order.rb")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v["meta"]["filtered"], serde_json::json!(true));
    assert_eq!(
        v["meta"]["only_paths"],
        serde_json::json!(["order.rb"]),
        "meta.only_paths must list the --only paths: {}",
        v["meta"]
    );
    let rows = v["ruby"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "--only should narrow to one row");
    assert_eq!(rows[0]["path"], serde_json::json!("order.rb"));
}

#[test]
fn exclude_single_star_does_not_cross_directory_separator() {
    // Ohm P1 on PR #87: Ruby `File.fnmatch` with FNM_PATHNAME treats
    // `*` as "match anything but /". A pattern like `lib/*.rb` must
    // NOT match `lib/deep/keep.rb`. Prior revision used
    // MatchOptions::new() (literal_separator=false) and silently
    // removed nested production files.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("lib/deep")).unwrap();
    std::fs::write(tmp.path().join("lib/top.rb"), "class Top\nend\n").unwrap();
    std::fs::write(
        tmp.path().join("lib/deep/keep.rb"),
        "class Keep\n  def run; end\nend\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("app.rb"), "class App\nend\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--exclude")
        .arg("lib/*.rb")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("lib/top.rb"),
        "lib/top.rb must be excluded by lib/*.rb: {stdout}"
    );
    assert!(
        stdout.contains("lib/deep/keep.rb"),
        "lib/deep/keep.rb must NOT be matched by lib/*.rb (FNM_PATHNAME): {stdout}"
    );
}

#[test]
fn diff_base_and_only_are_mutually_exclusive() {
    // Ohm P2 on PR #87: Ruby `validate_filter_options!` rejects
    // --diff-base combined with --only. Rust must do the same.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--diff-base")
        .arg("main")
        .arg("--only")
        .arg("order.rb")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "combined --diff-base + --only must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mutually exclusive"),
        "expected mutually-exclusive error on stderr: {stderr}"
    );
}

#[test]
fn churn_days_zero_is_rejected() {
    // Ohm P2 on PR #87: Ruby raises `--churn-days must be positive`
    // on `<= 0`. Rust must match.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--churn-days")
        .arg("0")
        .output()
        .unwrap();
    assert!(!out.status.success(), "--churn-days 0 must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--churn-days must be positive"),
        "expected positive-required error on stderr: {stderr}"
    );
}

#[test]
fn coverage_flag_deprecated_warning_always_emits_when_coverage_passed() {
    // Ohm P2 on PR #87: Ruby emits `coverage_flag_deprecated` every
    // time `--coverage` is parsed. When BOTH --coverage and
    // --ruby-coverage are given we reject with exit 2 (Rust can't
    // observe argv order, so refusing the ambiguity is safer than
    // silently picking the wrong path).
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let cov = tmp.path().join("cov.json");
    std::fs::write(&cov, r#"{"X": {"coverage": {}}}"#).unwrap();

    // Standalone --coverage: warning + success.
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--coverage")
        .arg(&cov)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "standalone --coverage must succeed");
    assert!(stderr.contains("coverage_flag_deprecated"));

    // Both --ruby-coverage AND --coverage: hard reject.
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--ruby-coverage")
        .arg(&cov)
        .arg("--coverage")
        .arg(&cov)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "combined --ruby-coverage + --coverage must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("coverage_flag_deprecated"),
        "deprecated warning must still emit when both are given: {stderr}"
    );
    assert!(
        stderr.contains("mutually exclusive"),
        "expected mutually-exclusive error on stderr: {stderr}"
    );
}

#[test]
fn auto_generated_sentinel_matches_ruby_regex() {
    // Ohm P2 on PR #87: Ruby's regex is `/\A\s*#\s*This file is
    // auto-generated/i` — zero-or-more whitespace after `#`,
    // case-insensitive. All of these should land in
    // default_excluded_count (not emitted).
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("no_space.rb"),
        "#This file is auto-generated\nclass X\nend\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("many_spaces.rb"),
        "#   This file is auto-generated\nclass Y\nend\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("case.rb"),
        "# THIS FILE IS AUTO-GENERATED\nclass Z\nend\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("keep.rb"), "class Keep\nend\n").unwrap();
    std::fs::write(tmp.path().join("another.rb"), "class Another\nend\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for excluded in ["no_space.rb", "many_spaces.rb", "case.rb"] {
        assert!(
            !stdout.contains(excluded),
            "{excluded} should be auto-generated-excluded: {stdout}"
        );
    }
    assert!(
        stdout.contains("keep.rb"),
        "non-sentinel file must survive: {stdout}"
    );
}

#[test]
fn only_paths_rebase_to_scan_root_in_nested_scan() {
    // Ohm P2 on PR #87: a scan of REPO/lib with `--only lib/foo.rb`
    // (repo-root-relative) must rebase so the filter compares
    // against the row's scan-root-relative path `foo.rb`. Ruby does
    // this via `rebase_to_analysis_root`.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success());
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(repo.join("lib/app")).unwrap();
    write_ruby_mini_repo(&repo.join("lib/app"));
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "base"]);

    // Scan lib/app, pass repo-root-relative `lib/app/order.rb` —
    // rebase should strip `lib/app/` and compare against `order.rb`.
    let out = Command::new(stud_finder_bin())
        .arg(repo.join("lib/app"))
        .arg("--only")
        .arg("lib/app/order.rb")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let _header = lines.next();
    let rows: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert_eq!(
        rows.len(),
        1,
        "nested-scan --only must rebase and emit 1 row: {stdout}"
    );
    assert!(rows[0].contains("order.rb"));
}

#[test]
fn only_empty_list_emits_zero_rows_with_diff_filter_empty_warning() {
    // Ohm non-blocking on PR #87: Ruby accepts `--only ""` as an
    // empty filter and emits zero rows with `diff_filter_empty`.
    // Match that rather than rejecting at parse time.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--only")
        .arg("")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "empty --only must succeed (not reject): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v["ruby"].as_array().unwrap().len(), 0);
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.get("code") == Some(&serde_json::json!("diff_filter_empty"))),
        "empty --only must emit diff_filter_empty warning: {warnings:?}"
    );
}

#[test]
fn auto_generated_check_does_not_crash_on_unicode_header() {
    // Ohm P1 on PR #87 round 2: slicing `rest[..SENTINEL.len()]`
    // panics if byte 27 falls mid-character. A valid source whose
    // first non-blank line starts with non-ASCII chars (`é`, etc.)
    // must not crash the whole scan.
    let tmp = tempfile::tempdir().unwrap();
    // 20 `é` chars = 40 bytes. Earlier revision crashed slicing at
    // byte 27.
    let eacute = "é".repeat(20);
    let header = format!("# {eacute}\nclass Unicode\nend\n");
    std::fs::write(tmp.path().join("unicode.rb"), header).unwrap();
    std::fs::write(tmp.path().join("plain.rb"), "class Plain\nend\n").unwrap();
    std::fs::write(tmp.path().join("third.rb"), "class Third\nend\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "non-ASCII header must not crash: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("unicode.rb"),
        "unicode.rb should be scored, not sentinel-excluded: {stdout}"
    );
}

#[test]
fn rebase_filter_paths_strips_scan_root_prefix_exactly_once() {
    // Ohm P2 on PR #87 round 2: `trim_start_matches` strips the
    // prefix REPEATEDLY, so a nested dir like `lib/lib/foo.rb`
    // under scan root `lib/` collapsed to `foo.rb` instead of
    // `lib/foo.rb`. Use `strip_prefix` once.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success());
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(repo.join("lib/lib")).unwrap();
    std::fs::write(
        repo.join("lib/lib/foo.rb"),
        "class Foo\n  def one; end\nend\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("lib/other.rb"),
        "class Other\n  def one; end\nend\n",
    )
    .unwrap();
    std::fs::write(repo.join("lib/third.rb"), "class Third\nend\n").unwrap();
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "base"]);

    // Scan lib/, filter `lib/lib/foo.rb` → rebased to `lib/foo.rb`,
    // which is the scan-root-relative key for the file.
    let out = Command::new(stud_finder_bin())
        .arg(repo.join("lib"))
        .arg("--only")
        .arg("lib/lib/foo.rb")
        .arg("--output")
        .arg("csv")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let _header = lines.next();
    let rows: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert_eq!(
        rows.len(),
        1,
        "nested repeated-prefix --only must match exactly once: {stdout}"
    );
    assert!(
        rows[0].contains("lib/foo.rb"),
        "row must be the lib/foo.rb file: {}",
        rows[0]
    );
}

#[test]
fn meta_only_paths_emits_original_user_input_not_rebased() {
    // Ohm P2 on PR #87 round 2: Ruby `json_meta` emits
    // `@options[:only_paths]` unchanged — the rebased list is for
    // filter matching only. A user who passes `--only lib/foo.rb`
    // in a nested scan must see `lib/foo.rb` back in meta, not the
    // internal `foo.rb` rebased form.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success());
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(repo.join("lib/app")).unwrap();
    write_ruby_mini_repo(&repo.join("lib/app"));
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "base"]);

    let out = Command::new(stud_finder_bin())
        .arg(repo.join("lib/app"))
        .arg("--only")
        .arg("lib/app/order.rb")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(
        v["meta"]["only_paths"],
        serde_json::json!(["lib/app/order.rb"]),
        "meta.only_paths must carry the user-supplied, un-rebased paths: {}",
        v["meta"]
    );
    // AND the row must have been matched after internal rebasing.
    let rows = v["ruby"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["path"], serde_json::json!("order.rb"));
}

#[test]
fn branch_threshold_must_be_less_than_trunk_threshold() {
    // Ruby `Scorer#validate!` raises `branch-threshold must be
    // strictly less than trunk-threshold.` when the order is wrong.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--trunk-threshold")
        .arg("50")
        .arg("--branch-threshold")
        .arg("60")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "invalid thresholds must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("branch-threshold must be strictly less than trunk-threshold"),
        "expected Ruby threshold error: {stderr}"
    );
}

#[test]
fn trunk_threshold_above_100_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--trunk-threshold")
        .arg("101")
        .output()
        .unwrap();
    assert!(!out.status.success(), "threshold > 100 must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("between 0 and 100"),
        "expected range error: {stderr}"
    );
}

#[test]
fn no_newness_flag_flips_new_file_to_false_everywhere() {
    // With `--no-newness`, sf-newness::analyze is skipped and every
    // row should carry `new_file: false` with `age_days: 0`.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--no-newness")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let rows = v["ruby"].as_array().unwrap();
    for r in rows {
        assert_eq!(
            r["new_file"],
            serde_json::json!(false),
            "--no-newness must flip new_file to false: {r:?}"
        );
    }
}

#[test]
fn min_files_below_threshold_emits_small_repo_warning() {
    // A 3-file repo with --min-files=20 should emit the `small_repo`
    // warning in both stderr and meta.warnings.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.get("code") == Some(&serde_json::json!("small_repo"))),
        "meta.warnings must carry small_repo at 3 files < default --min-files 20: {warnings:?}"
    );
}

#[test]
fn custom_min_files_suppresses_small_repo_warning() {
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--min-files")
        .arg("2")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        !warnings
            .iter()
            .any(|w| w.get("code") == Some(&serde_json::json!("small_repo"))),
        "--min-files below analyzed count must NOT emit small_repo: {warnings:?}"
    );
}

#[test]
fn weights_sum_to_one_across_four_base_signals_matching_ruby() {
    // Ohm non-blocking fix on PR #86: Ruby `normalize_weights` keeps
    // all four BASE_WEIGHT_KEYS active regardless of which signals
    // are populated. Previously Rust dropped absent signals from the
    // denominator, inflating remaining weights. A tiny 3-file repo
    // with no git history has no churn + no coupling; the four base
    // weights must still sum to ≈1.0 after renormalization.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let w = v["meta"]["weights"].as_object().unwrap();
    let sum = w["fan_in"].as_f64().unwrap()
        + w["fan_out"].as_f64().unwrap()
        + w["complexity"].as_f64().unwrap()
        + w["churn"].as_f64().unwrap();
    assert!(
        (sum - 1.0).abs() < 1e-3,
        "four BASE_WEIGHT_KEYS should renorm to ≈1.0 even with absent signals; got {sum} from {w:?}"
    );
}

#[test]
fn new_file_options_accept_zero_matching_ruby() {
    // Ohm P2 on PR #88: Ruby validate_newness_options! explicitly
    // accepts zero for --new-file-days and --new-file-min-commits
    // (cli.rb:378-383). Zero disables that half of the recency
    // check. Prior revision rejected zero as "value must be positive".
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    for flag in ["--new-file-days", "--new-file-min-commits"] {
        let out = Command::new(stud_finder_bin())
            .arg(tmp.path())
            .arg(flag)
            .arg("0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{flag} 0 must succeed (Ruby accepts): stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn no_newness_flag_emits_null_evidence_not_placeholder() {
    // Ohm P2 on PR #88: with --no-newness, newness metadata is
    // unavailable so Ruby `Newness.evidence(...)` returns nil.
    // JSON must emit `evidence: null`, not the scorer's placeholder
    // 1.0, to avoid claiming maximum confidence we don't have.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--no-newness")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    for r in v["ruby"].as_array().unwrap() {
        assert_eq!(
            r["evidence"],
            serde_json::Value::Null,
            "--no-newness must emit null evidence, not placeholder: {r:?}"
        );
        assert_eq!(r["new_file"], serde_json::json!(false));
    }
}

#[test]
fn small_repo_warns_per_section_not_combined() {
    // Ohm P2 on PR #88: Ruby score_group adds small_repo when ANY
    // non-empty scored group is below --min-files. Prior revision
    // used combined count, so a 2 Ruby + 2 JS repo with --min-files
    // 3 suppressed the warning though both sections are small.
    let tmp = tempfile::tempdir().unwrap();
    // 2 Ruby files
    std::fs::write(tmp.path().join("a.rb"), "class A\nend\n").unwrap();
    std::fs::write(tmp.path().join("b.rb"), "class B\nend\n").unwrap();
    // 2 JS files
    std::fs::write(
        tmp.path().join("app.js"),
        "import { x } from './util';\nconsole.log(x);\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("util.js"), "export const x = 1;\n").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--min-files")
        .arg("3")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.get("code") == Some(&serde_json::json!("small_repo"))),
        "mixed-language small-per-section must trip small_repo even if combined count ≥ threshold: {warnings:?}"
    );
}

#[test]
fn coupling_threshold_above_one_is_rejected() {
    // Ruby `parse_weights`-style range check: --coupling-threshold
    // must be in [0.0, 1.0].
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--coupling-threshold")
        .arg("1.5")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "range-violated threshold must exit non-zero"
    );
}

#[test]
fn coupling_min_commits_zero_is_rejected() {
    // Must be positive (a 0 threshold would never gate anything).
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--coupling-min-commits")
        .arg("0")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "--coupling-min-commits 0 must exit non-zero"
    );
}

#[test]
fn coupling_max_commit_files_zero_means_unlimited() {
    // Ohm P2 on PR #89 round 2: Ruby documents `0 = unlimited`
    // for this flag (cli.rb:287-290). Must accept zero and skip
    // the bulk-commit cap entirely — not reject, not treat as
    // literal cap (which would skip every nonempty commit).
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--coupling-max-commit-files")
        .arg("0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--coupling-max-commit-files 0 must be accepted as unlimited: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("bulk commits"),
        "0 cap must disable bulk-commit filtering: {stderr}"
    );
}

#[test]
fn no_rails_inference_is_accepted_for_parity() {
    // `--no-rails-inference` is a no-op in the Rust port today
    // (no Rails inference yet) but must be accepted so automation
    // targeting Ruby doesn't break.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--no-rails-inference")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--no-rails-inference must succeed as parity stub: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn js_timeout_is_accepted_for_parity() {
    // `--js-timeout N` is a no-op (no dependency-cruiser shell-out
    // in the Rust port) but must be accepted.
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--js-timeout")
        .arg("120")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--js-timeout must succeed as parity stub"
    );
}

#[test]
fn js_only_repo_uses_js_section_weights_not_ruby_zeroes() {
    // Ohm P2 on PR #86: with no Ruby files + at least one JS file,
    // meta.weights must come from the JS section (nonzero complexity)
    // rather than from the Ruby section (all zero). Matches Ruby
    // `analysis.ruby.weights || analysis.javascript.weights`.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("app.js"),
        "export function greet(name) { return 'hi ' + name; }\n",
    )
    .unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let weights = v["meta"]["weights"].as_object().expect("weights object");
    let complexity = weights["complexity"].as_f64().expect("complexity f64");
    assert!(
        complexity > 0.0,
        "JS-only repo must report nonzero complexity weight; got {weights:?}"
    );
}
