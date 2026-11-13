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

// --- Cleanup slice #12: Ruby error-text + --help + -- terminator ---

/// Ruby `OptionParser` prints a banner + 8-space-indented option
/// list on `--help`, and exits 0. Verified byte-for-byte against
/// `ruby -Ilib bin/stud-finder gate --help` on ruby:3.3.13-slim.
#[test]
fn gate_help_long_matches_ruby_banner() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success(), "--help must exit 0");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout,
        "Usage: stud-finder gate [--input FILE] [--enforce]\n        \
         --input FILE                 Read stud-finder JSON output from FILE\n        \
         --enforce                    Exit non-zero when gate findings are present\n"
    );
}

#[test]
fn gate_help_short_flag_equivalent_to_long() {
    let short = Command::new(stud_finder_bin())
        .args(["gate", "-h"])
        .output()
        .unwrap();
    let long = Command::new(stud_finder_bin())
        .args(["gate", "--help"])
        .output()
        .unwrap();
    assert_eq!(short.stdout, long.stdout);
    assert!(short.status.success() && long.status.success());
}

/// Ruby OptionParser raises InvalidOption → `invalid option: --foo`
/// with exit code 1 (via the CLI rescue returning 1).
#[test]
fn gate_unknown_flag_matches_ruby_invalid_option() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "--foo"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid option: --foo"),
        "expected `invalid option: --foo`: {stderr}"
    );
}

/// Ruby `ValidationError` message: `Error: unexpected arguments: ...`,
/// with ALL remaining argv entries joined by space.
#[test]
fn gate_positional_args_report_all_joined() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "garbage", "trash"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: unexpected arguments: garbage trash"),
        "expected both args joined: {stderr}"
    );
}

/// `--` is the Ruby-standard "stop parsing options" sentinel. Ruby
/// OptionParser consumes it and leaves everything after as
/// positional, which then trips the `unless @argv.empty?` check.
#[test]
fn gate_double_dash_terminator_pushes_rest_as_positional() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "--", "extra", "args"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: unexpected arguments: extra args"),
        "args after `--` must surface as positional: {stderr}"
    );
}

#[test]
fn gate_missing_input_value_matches_ruby_missing_argument() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "--input"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("missing argument: --input"),
        "expected `missing argument: --input`: {stderr}"
    );
}

#[test]
fn gate_enforce_with_inline_value_matches_ruby_needless_argument() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "--enforce=foo"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("needless argument: --enforce=foo"),
        "expected `needless argument: --enforce=foo`: {stderr}"
    );
}

#[test]
fn gate_missing_input_file_matches_ruby_enoent() {
    let out = Command::new(stud_finder_bin())
        .args(["gate", "--input", "/definitely/not/a/path.json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: input file not found: /definitely/not/a/path.json"),
        "expected Ruby ENOENT wording: {stderr}"
    );
}

/// Ruby `Gate.markdown(...)` returns a string ending in exactly
/// ONE `\n`. The CLI `print!`s it without a trailing newline, so
/// the final stdout byte is `\n` (not `\n\n`). The old
/// `push_str("\n</details>\n\n")` tail produced an extra blank
/// line (Ohm's non-blocking note after PR #91).
#[test]
fn gate_markdown_output_ends_with_single_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("report.json");
    std::fs::write(&input, SAMPLE).unwrap();
    let out = Command::new(stud_finder_bin())
        .arg("gate")
        .arg("--input")
        .arg(&input)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = out.stdout;
    assert!(
        stdout.ends_with(b"</details>\n") && !stdout.ends_with(b"</details>\n\n"),
        "stdout must end with `</details>\\n` exactly, got last 32 bytes = {:?}",
        String::from_utf8_lossy(&stdout[stdout.len().saturating_sub(32)..])
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
