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
fn output_json_emits_ruby_array_with_full_row_shape() {
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
    let ruby = v.get("ruby").expect("ruby key").as_array().expect("array");
    assert!(ruby.len() >= 2, "expected 2+ rows: {ruby:?}");
    // Every row must carry the full shape.
    let expected_keys = [
        "path",
        "class",
        "composite",
        "fan_in",
        "fan_out",
        "complexity",
        "churn_commits",
        "churn_lines",
        "max_coupling",
        "max_coupling_partner",
        "coverage",
        "fan_in_pct",
        "fan_out_pct",
        "complexity_pct",
        "churn_pct",
        "coupling_pct",
        "coverage_pct",
        "evidence",
    ];
    for row in ruby {
        for k in &expected_keys {
            assert!(
                row.get(k).is_some(),
                "json row missing field `{k}`: {row:?}"
            );
        }
    }
}

#[test]
fn output_csv_emits_header_and_rows_per_section() {
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
    assert!(stdout.starts_with("# Ruby\n"), "no Ruby header: {stdout}");
    let after = stdout.strip_prefix("# Ruby\n").unwrap();
    let mut lines = after.lines();
    let header = lines.next().expect("header line");
    assert!(
        header.contains("class,score,fan_in"),
        "header shape wrong: {header}"
    );
    let row_count = lines.filter(|l| !l.is_empty()).count();
    assert!(row_count >= 2, "expected 2+ data rows: {stdout}");
}

#[test]
fn output_markdown_emits_pipe_table() {
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
    assert!(stdout.starts_with("# Ruby"));
    assert!(
        stdout.contains("| class | score |"),
        "pipe-table header missing: {stdout}"
    );
    assert!(
        stdout.contains("| --- | --- |"),
        "pipe-table separator missing: {stdout}"
    );
    // At least one data row with pipes.
    let data_row_count = stdout
        .lines()
        .filter(|l| l.starts_with("| ") && !l.contains("---") && !l.contains("class |"))
        .count();
    assert!(data_row_count >= 2, "expected 2+ data rows: {stdout}");
}

#[test]
fn top_limits_rows_per_section() {
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    // Table format for a tight shape; count data rows.
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--top")
        .arg("1")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // After the Ruby header and the column-header line, there should
    // be exactly 1 data row ending with `.rb`.
    let data_rows: Vec<&str> = stdout.lines().filter(|l| l.ends_with(".rb")).collect();
    assert_eq!(data_rows.len(), 1, "--top 1 should keep 1 row: {stdout}");
}

#[test]
fn top_zero_keeps_all_rows() {
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let baseline = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .output()
        .unwrap();
    let topped = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--top")
        .arg("0")
        .output()
        .unwrap();
    let a = String::from_utf8_lossy(&baseline.stdout)
        .lines()
        .filter(|l| l.ends_with(".rb"))
        .count();
    let b = String::from_utf8_lossy(&topped.stdout)
        .lines()
        .filter(|l| l.ends_with(".rb"))
        .count();
    assert_eq!(a, b, "--top 0 must keep all rows");
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
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let file_lines: Vec<&str> = stdout.lines().filter(|l| l.ends_with(".rb")).collect();
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
fn diff_base_on_non_git_dir_degrades_to_unfiltered_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    write_ruby_mini_repo(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--diff-base")
        .arg("main")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let data_rows: usize = stdout.lines().filter(|l| l.ends_with(".rb")).count();
    assert_eq!(
        data_rows, 3,
        "non-git-repo should fall back to unfiltered: {stdout}"
    );
    assert!(
        stderr.contains("--diff-base"),
        "expected warning on stderr: {stderr}"
    );
}
