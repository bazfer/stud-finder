//! End-to-end for `--no-auto-unshallow` and the auto-unshallow
//! behavior on a real shallow clone. Builds a tiny source repo,
//! clones it with `--depth=1`, then drives `stud-finder` over the
//! shallow clone to verify:
//!
//! - `--no-auto-unshallow` emits only `shallow_clone_newness_disabled`
//! - default behavior (auto-unshallow enabled) runs the fetch; on a
//!   repo where fetch can't succeed, BOTH warnings appear
//! - `--no-newness` is a hard short-circuit: no shallow check at all
//!
//! Matches Ruby `attempt_unshallow` at `lib/stud_finder/cli.rb:573`.

use std::process::{Command, Stdio};

fn stud_finder_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stud-finder")
}

fn init_source_repo(src: &std::path::Path) {
    assert!(
        Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(src)
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
            .current_dir(src)
            .status()
            .unwrap();
    }
    // Three tiny Ruby files + two commits each so we have something
    // to score once newness is active.
    for (name, body) in [
        ("a.rb", "class A\n  def one; end\nend\n"),
        ("b.rb", "class B\n  def two; end\nend\n"),
        ("c.rb", "class C\n  def three; end\nend\n"),
    ] {
        std::fs::write(src.join(name), body).unwrap();
    }
    Command::new("git")
        .args(["add", "."])
        .current_dir(src)
        .status()
        .unwrap();
    assert!(
        Command::new("git")
            .args(["commit", "-m", "seed"])
            .current_dir(src)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    for (name, body) in [
        (
            "a.rb",
            "class A\n  def one; two; end\n  def two; end\nend\n",
        ),
        ("b.rb", "class B\n  def two; end\nend\n"),
    ] {
        std::fs::write(src.join(name), body).unwrap();
    }
    Command::new("git")
        .args(["add", "."])
        .current_dir(src)
        .status()
        .unwrap();
    assert!(
        Command::new("git")
            .args(["commit", "-m", "touch"])
            .current_dir(src)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

/// Shallow-clone a source repo, then break its remote so a
/// subsequent `git fetch --unshallow` can't succeed. Returns the
/// shallow path.
fn make_shallow_with_unfetchable_remote(root: &std::path::Path) -> std::path::PathBuf {
    let src = root.join("src");
    std::fs::create_dir(&src).unwrap();
    init_source_repo(&src);
    let shallow = root.join("shallow");
    assert!(
        Command::new("git")
            .args([
                "clone",
                "--depth=1",
                "--no-local",
                src.to_str().unwrap(),
                shallow.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    Command::new("git")
        .args(["remote", "set-url", "origin", "/no/such/remote"])
        .current_dir(&shallow)
        .status()
        .unwrap();
    shallow
}

/// `--no-auto-unshallow` on a shallow clone: Ruby emits ONLY
/// `shallow_clone_newness_disabled` + proceeds with disabled newness.
#[test]
fn no_auto_unshallow_emits_single_warning_on_shallow() {
    let tmp = tempfile::tempdir().unwrap();
    let shallow = make_shallow_with_unfetchable_remote(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(&shallow)
        .arg("--no-auto-unshallow")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit 0 on shallow + --no-auto-unshallow: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stderr.contains("shallow git clone detected"),
        "expected shallow-disabled stderr: {stderr}"
    );
    assert!(
        !stderr.contains("auto-unshallow failed"),
        "--no-auto-unshallow must NOT attempt unshallow: {stderr}"
    );
    let payload: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let warnings = payload["warnings"].as_array().expect("warnings array");
    let codes: Vec<&str> = warnings.iter().filter_map(|w| w["code"].as_str()).collect();
    assert!(
        codes.contains(&"shallow_clone_newness_disabled"),
        "expected shallow_clone_newness_disabled in warnings, got {codes:?}"
    );
    assert!(
        !codes.contains(&"shallow_clone_unshallow_failed"),
        "--no-auto-unshallow must NOT emit unshallow_failed, got {codes:?}"
    );
}

/// Default behavior: on a shallow clone with a broken remote,
/// stud-finder attempts `git fetch --unshallow`, fails, and emits
/// BOTH warnings. The scan still completes (exit 0) with disabled
/// newness.
#[test]
fn default_attempts_unshallow_and_emits_both_warnings_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let shallow = make_shallow_with_unfetchable_remote(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(&shallow)
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit 0 even when unshallow fails: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("auto-unshallow: fetching full history"),
        "expected auto-unshallow progress line: {stderr}"
    );
    assert!(
        stderr.contains("auto-unshallow failed"),
        "expected auto-unshallow failure line: {stderr}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let codes: Vec<&str> = payload["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert!(
        codes.contains(&"shallow_clone_newness_disabled"),
        "expected newness_disabled warning: {codes:?}"
    );
    assert!(
        codes.contains(&"shallow_clone_unshallow_failed"),
        "expected unshallow_failed warning: {codes:?}"
    );
}

/// `--no-newness` short-circuits the shallow check entirely — no
/// progress line, no warnings. Ruby's `apply_newness` guard reads
/// `@options[:newness] && Newness.shallow_repository?`, so disabling
/// newness never consults shallow state.
#[test]
fn no_newness_skips_shallow_check_entirely() {
    let tmp = tempfile::tempdir().unwrap();
    let shallow = make_shallow_with_unfetchable_remote(tmp.path());
    let out = Command::new(stud_finder_bin())
        .arg(&shallow)
        .arg("--no-newness")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("shallow git clone"),
        "--no-newness must suppress shallow warning: {stderr}"
    );
    assert!(
        !stderr.contains("auto-unshallow"),
        "--no-newness must not run auto-unshallow: {stderr}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let codes: Vec<&str> = payload["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert!(
        !codes.contains(&"shallow_clone_newness_disabled")
            && !codes.contains(&"shallow_clone_unshallow_failed"),
        "no shallow warnings under --no-newness, got {codes:?}"
    );
}

/// Both sections (Ruby + JS) share the same shallow handling —
/// Ruby's `apply_newness` runs once per scan, not once per
/// language. Without this, we'd emit the shallow warning TWICE on
/// a mixed-language repo (once for the Ruby section's analyze,
/// once for JS). Verify we emit it only once.
#[test]
fn shallow_warning_is_emitted_only_once_for_mixed_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir(&src).unwrap();
    init_source_repo(&src);
    // Add a tiny JS file + commit so the scan has a JS section.
    std::fs::write(src.join("x.js"), "export const x = 1;\n").unwrap();
    Command::new("git")
        .args(["add", "x.js"])
        .current_dir(&src)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "js"])
        .current_dir(&src)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    let shallow = tmp.path().join("shallow");
    assert!(
        Command::new("git")
            .args([
                "clone",
                "--depth=1",
                "--no-local",
                src.to_str().unwrap(),
                shallow.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    Command::new("git")
        .args(["remote", "set-url", "origin", "/no/such/remote"])
        .current_dir(&shallow)
        .status()
        .unwrap();
    let out = Command::new(stud_finder_bin())
        .arg(&shallow)
        .arg("--no-auto-unshallow")
        .arg("--output")
        .arg("json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let dup_count = payload["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"].as_str() == Some("shallow_clone_newness_disabled"))
        .count();
    assert_eq!(
        dup_count, 1,
        "shallow_clone_newness_disabled must appear exactly once across sections"
    );
}
