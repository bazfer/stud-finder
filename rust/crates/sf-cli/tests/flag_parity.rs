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
