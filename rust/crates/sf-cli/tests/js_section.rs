//! JS section end-to-end tests. Driven through `--output csv` so row
//! parsing stays trivial regardless of the markdown / table layout
//! changes — the CSV contract is pinned to Ruby `RESULT_COLUMNS` and
//! unchanged across slices.
//!
//! The CSV column layout we read from here is
//! `rank,language,file,score,evidence,class,new_file,age_days,
//!  escalation,fan_in,fan_in_pct,fan_out,...`
//!
//! so `fan_in` lives at column index 9 and `file` at index 2.

use std::process::Command;

fn stud_finder_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stud-finder")
}

/// Run the CLI in CSV mode and return the raw stdout. Panics on
/// failure — test fixtures should always succeed.
fn run_csv(root: &std::path::Path) -> String {
    let out = Command::new(stud_finder_bin())
        .arg(root)
        .arg("--output")
        .arg("csv")
        .output()
        .expect("stud-finder");
    assert!(
        out.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Parse CSV, return (header, data rows) as Vec<Vec<String>>. Simple
/// splitter — no multi-line quoted cells in these fixtures.
fn parse_csv(stdout: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = stdout.lines();
    let header = lines
        .next()
        .expect("CSV header")
        .split(',')
        .map(str::to_string)
        .collect();
    let rows = lines
        .filter(|l| !l.is_empty())
        .map(|l| l.split(',').map(str::to_string).collect())
        .collect();
    (header, rows)
}

fn row_for(rows: &[Vec<String>], path_suffix: &str) -> Option<Vec<String>> {
    rows.iter()
        .find(|r| r.get(2).map(|f| f.contains(path_suffix)).unwrap_or(false))
        .cloned()
}

fn fan_in_of(rows: &[Vec<String>], path_suffix: &str) -> usize {
    row_for(rows, path_suffix)
        .unwrap_or_else(|| panic!("row for {path_suffix} not found"))
        .get(9)
        .expect("fan_in column 9")
        .parse()
        .expect("fan_in numeric")
}

fn language_of(rows: &[Vec<String>], path_suffix: &str) -> String {
    row_for(rows, path_suffix)
        .unwrap_or_else(|| panic!("row for {path_suffix} not found"))
        .get(1)
        .expect("language column 1")
        .clone()
}

#[test]
fn js_section_renders_with_fan_in_from_relative_imports() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    std::fs::write(
        root.join("util.js"),
        "export function greet(name) { return 'hi ' + name; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app.js"),
        "import { greet } from './util';\nconsole.log(greet('world'));\n",
    )
    .unwrap();
    std::fs::write(root.join("thing.rb"), "class Thing\nend\n").unwrap();

    let stdout = run_csv(root);
    let (_header, rows) = parse_csv(&stdout);
    // Both sections present.
    assert!(
        rows.iter().any(|r| r.get(1).map(String::as_str) == Some("ruby")),
        "no ruby rows: {stdout}"
    );
    assert!(
        rows.iter().any(|r| matches!(
            r.get(1).map(String::as_str),
            Some("javascript") | Some("typescript")
        )),
        "no js rows: {stdout}"
    );
    assert!(
        fan_in_of(&rows, "util.js") >= 1,
        "util.js should have fan_in >= 1"
    );
}

#[test]
fn same_basename_distinct_paths_do_not_collapse() {
    // Ohm P1 on PR #81: `a/index.js` and `b/index.js` must stay
    // distinct via path-exact edges. A caller that imports only
    // `./a/index.js` must NOT lift b/index.js's fan_in.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    std::fs::write(root.join("a/index.js"), "export const a = 1;\n").unwrap();
    std::fs::write(root.join("b/index.js"), "export const b = 2;\n").unwrap();
    std::fs::write(
        root.join("caller.js"),
        "import { a } from './a/index.js';\nconsole.log(a);\n",
    )
    .unwrap();

    let stdout = run_csv(root);
    let (_, rows) = parse_csv(&stdout);
    assert_eq!(
        fan_in_of(&rows, "a/index.js"),
        1,
        "a/index.js should see the import"
    );
    assert_eq!(
        fan_in_of(&rows, "b/index.js"),
        0,
        "b/index.js must NOT inherit"
    );
}

#[test]
fn non_js_extension_specifier_never_rewrites_to_js() {
    // Ohm P1 on PR #81 re-review: `./foo.json` must NOT get rewritten
    // to `./foo.ts`. The previous fix only protected JS extensions.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("foo.ts"), "export const foo = 1;\n").unwrap();
    std::fs::write(
        root.join("caller.ts"),
        "import data from './foo.json';\nconsole.log(data);\n",
    )
    .unwrap();

    let stdout = run_csv(root);
    let (_, rows) = parse_csv(&stdout);
    assert_eq!(
        fan_in_of(&rows, "foo.ts"),
        0,
        "`./foo.json` specifier must NOT be rewritten to foo.ts"
    );
}

#[test]
fn explicit_extension_does_not_fall_through_to_other_extensions() {
    // Ohm P1 on PR #81: importing `./foo.js` while a `foo.ts` also
    // exists must NOT silently resolve to foo.ts.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("foo.ts"), "export const foo = 1;\n").unwrap();
    std::fs::write(
        root.join("caller.js"),
        "import { foo } from './foo.js';\nconsole.log(foo);\n",
    )
    .unwrap();

    let stdout = run_csv(root);
    let (_, rows) = parse_csv(&stdout);
    assert_eq!(
        fan_in_of(&rows, "foo.ts"),
        0,
        "explicit .js import must NOT create edge to .ts"
    );
}

#[test]
fn mjs_extension_is_collected_and_resolvable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("util.mjs"), "export const x = 1;\n").unwrap();
    std::fs::write(
        root.join("app.mjs"),
        "import { x } from './util';\nconsole.log(x);\n",
    )
    .unwrap();

    let stdout = run_csv(root);
    let (_, rows) = parse_csv(&stdout);
    assert!(
        fan_in_of(&rows, "util.mjs") >= 1,
        "util.mjs should have fan_in >= 1"
    );
}

#[test]
fn malformed_js_is_excluded_not_imputed() {
    // Parse-failed JS file must not be registered + must not sink
    // neighbours. Score for the clean file stays identical across
    // broken-vs-not runs.
    let make_run = |include_broken: bool| -> String {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("a.js"), "export const a = 1;\n").unwrap();
        std::fs::write(root.join("b.js"), "import { a } from './a';\n").unwrap();
        if include_broken {
            std::fs::write(root.join("broken.js"), "function f( { !!! what").unwrap();
        }
        run_csv(root)
    };
    let clean = make_run(false);
    let mixed = make_run(true);
    let clean_rows = parse_csv(&clean).1;
    let mixed_rows = parse_csv(&mixed).1;
    // Compare the `score` column (index 3) of a.js — broken didn't
    // participate in the percentile pool, so a.js's score should be
    // identical.
    let a_score = |rows: &[Vec<String>]| -> String {
        row_for(rows, "a.js")
            .expect("a.js row")
            .get(3)
            .expect("score col")
            .clone()
    };
    assert_eq!(
        a_score(&clean_rows),
        a_score(&mixed_rows),
        "broken.js must not shift a.js score"
    );
    assert!(
        row_for(&mixed_rows, "broken.js").is_none(),
        "broken.js must be excluded"
    );
}

#[test]
fn no_js_files_keeps_only_ruby_section() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("a.rb"), "class A\nend\n").unwrap();

    let stdout = run_csv(root);
    let (_, rows) = parse_csv(&stdout);
    assert!(
        rows.iter().all(|r| r.get(1).map(String::as_str) == Some("ruby")),
        "all rows should be ruby: {stdout}"
    );
    assert_eq!(language_of(&rows, "a.rb"), "ruby");
}
