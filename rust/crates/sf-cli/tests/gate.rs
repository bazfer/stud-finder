//! End-to-end for the `stud-finder gate` subcommand. Pipes a
//! minimal JSON fixture into the binary via --input (and stdin)
//! and asserts markdown shape + exit codes.

use std::io::Write;
use std::process::{Command, Stdio};

fn stud_finder_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stud-finder")
}

const SAMPLE: &str = r#"{
    "meta": {},
    "warnings": [],
    "ruby": [
        {"path": "a.rb", "class": "trunk", "score": 0.9, "evidence": 1.0, "new_file": false, "escalation": ""},
        {"path": "b.rb", "class": "branch", "score": 0.8, "evidence": 0.4, "new_file": false, "escalation": ""},
        {"path": "c.rb", "class": "branch", "score": 0.2, "evidence": 0.9, "new_file": true, "escalation": "trunk_adjacent"}
    ],
    "javascript": []
}"#;

#[test]
fn gate_reads_from_input_file_and_emits_markdown() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("report.json");
    std::fs::write(&input, SAMPLE).unwrap();
    let out = Command::new(stud_finder_bin())
        .arg("gate")
        .arg("--input")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "observation mode must exit 0: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("## Stud Finder gate\n"));
    assert!(stdout.contains("**Mode:** observation"));
    assert!(stdout.contains("**Summary:** 3 findings"));
    for check in [
        "trunk_touched",
        "low_evidence_high_score",
        "newness_trunk_adjacent",
    ] {
        assert!(
            stdout.contains(&format!("`{check}`")),
            "missing check {check}: {stdout}"
        );
    }
}

#[test]
fn gate_enforce_flag_exits_one_on_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("report.json");
    std::fs::write(&input, SAMPLE).unwrap();
    let out = Command::new(stud_finder_bin())
        .arg("gate")
        .arg("--input")
        .arg(&input)
        .arg("--enforce")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "--enforce must exit 1 when findings exist"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("**Mode:** enforce"));
}

#[test]
fn gate_enforce_without_findings_exits_zero() {
    let json = r#"{"ruby": [{"path": "x.rb", "class": "leaf", "score": 0.1, "evidence": 1.0}], "javascript": []}"#;
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("clean.json");
    std::fs::write(&input, json).unwrap();
    let out = Command::new(stud_finder_bin())
        .arg("gate")
        .arg("--input")
        .arg(&input)
        .arg("--enforce")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--enforce with zero findings must exit 0: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("**Summary:** 0 findings"));
}

#[test]
fn gate_reads_from_stdin_when_no_input_flag() {
    let mut child = Command::new(stud_finder_bin())
        .arg("gate")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(SAMPLE.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stdin pipe should succeed: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Stud Finder gate"));
}

#[test]
fn gate_malformed_json_exits_nonzero_with_ruby_error() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("bad.json");
    std::fs::write(&input, "{not json").unwrap();
    let out = Command::new(stud_finder_bin())
        .arg("gate")
        .arg("--input")
        .arg(&input)
        .output()
        .unwrap();
    assert!(!out.status.success(), "malformed JSON must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid JSON input"),
        "expected Ruby invalid-JSON error: {stderr}"
    );
}

#[test]
fn gate_scan_json_roundtrips_through_gate() {
    // End-to-end round-trip: run `scan --output json` on a tiny
    // Ruby repo, pipe the output through `stud-finder gate`.
    // Proves our own emitted JSON parses cleanly in the gate
    // crate (the whole point of this slice).
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("order.rb"),
        "class Order\n  def process(x); Payment.capture if x; end\nend\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("payment.rb"),
        "class Payment\n  def self.capture; end\nend\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("user.rb"),
        "class User\n  def ship\n    Order.new.process(true)\n  end\nend\n",
    )
    .unwrap();
    // scan → json
    let scan = Command::new(stud_finder_bin())
        .arg(tmp.path())
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(scan.status.success());
    // gate from stdin
    let mut child = Command::new(stud_finder_bin())
        .arg("gate")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&scan.stdout)
        .unwrap();
    let gate_out = child.wait_with_output().unwrap();
    assert!(
        gate_out.status.success(),
        "gate on our own scan output must succeed: stderr: {}",
        String::from_utf8_lossy(&gate_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&gate_out.stdout);
    assert!(stdout.starts_with("## Stud Finder gate"));
}
