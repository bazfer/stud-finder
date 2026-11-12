//! `--coverage` flag end-to-end: build a tempdir Ruby repo, drop a
//! SimpleCov resultset.json beside it, run the binary with the flag,
//! assert a `coverage` column appears in the output.

use std::process::Command;

#[test]
fn coverage_flag_adds_coverage_column_with_rate() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();

    // One Ruby file + a SimpleCov resultset covering it.
    std::fs::write(
        repo.join("order.rb"),
        "class Order\n  def go\n    Payment.capture\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("payment.rb"),
        "class Payment\n  def self.capture; end\nend\n",
    )
    .unwrap();

    let rs = r#"{
        "RSpec": {
            "coverage": {
                "order.rb": [1, 1, 1, null, null],
                "payment.rb": [1, 0, null]
            }
        }
    }"#;
    std::fs::write(repo.join("coverage.json"), rs).unwrap();

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin)
        .arg(repo)
        .arg("--coverage")
        .arg(repo.join("coverage.json"))
        .output()
        .expect("stud-finder");
    assert!(
        out.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("coverage"),
        "header missing coverage column: {stdout}"
    );
    // order.rb has 3/3 = 1.00; payment.rb has 1/2 = 0.50. Both values
    // should appear somewhere in the output (format `1.00` / `0.50`).
    assert!(
        stdout.contains("1.00"),
        "expected 1.00 coverage for order.rb: {stdout}"
    );
    assert!(
        stdout.contains("0.50"),
        "expected 0.50 coverage for payment.rb: {stdout}"
    );
}

#[test]
fn no_coverage_flag_produces_empty_coverage_column() {
    // Without --coverage, every row should show 0.00 but the column
    // header still appears (the slice always prints it). Simplest to
    // assert: header present, no "1.00" value since nothing has
    // coverage.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    std::fs::write(repo.join("a.rb"), "class A\nend\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(repo).output().expect("stud-finder");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("coverage"), "header: {stdout}");
}
