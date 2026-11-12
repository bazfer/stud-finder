//! End-to-end: run the `stud-finder` binary over a tempdir git repo
//! and confirm the output table carries the new `churn` + `coupling`
//! columns introduced by sf-history.

use std::process::Command;

#[test]
fn cli_output_includes_churn_and_coupling_columns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
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
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "commit.gpgsign", "false"]);

    for i in 0..6 {
        std::fs::write(
            repo.join("a.rb"),
            format!("class A\n  def go\n    B.m{i}\n  end\nend\n"),
        )
        .unwrap();
        std::fs::write(
            repo.join("b.rb"),
            format!("class B\n  def m{i}\n    1\n  end\nend\n"),
        )
        .unwrap();
        run(&["add", "a.rb", "b.rb"]);
        run(&["commit", "-q", "-m", &format!("co {i}")]);
    }

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(repo).output().expect("stud-finder");
    assert!(
        out.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("churn"), "header missing churn: {stdout}");
    assert!(
        stdout.contains("coupling"),
        "header missing coupling: {stdout}"
    );
    // Both files co-changed 6 times; each row should carry churn 6.
    assert!(
        stdout
            .lines()
            .any(|l| l.contains("a.rb") && l.contains("6")),
        "a.rb row should carry churn count 6: {stdout}"
    );
}
