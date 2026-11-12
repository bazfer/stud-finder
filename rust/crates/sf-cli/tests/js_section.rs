//! `--coverage`-independent JS section: build a tempdir with two JS
//! files where one imports the other, run the binary, assert both a
//! `# Ruby` and a `# JavaScript` section appear and the referenced JS
//! file lands with non-zero fan_in.

use std::process::Command;

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
    // Ruby file so the Ruby header renders too.
    std::fs::write(root.join("thing.rb"), "class Thing\nend\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(root).output().expect("stud-finder");
    assert!(
        out.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Ruby"), "missing Ruby header: {stdout}");
    assert!(
        stdout.contains("# JavaScript"),
        "missing JavaScript header: {stdout}"
    );
    // util.js is referenced by app.js — in the JS section it should
    // appear with fan_in >= 1.
    let js_section = stdout
        .split_once("# JavaScript")
        .map(|(_, s)| s)
        .unwrap_or("");
    let util_line = js_section
        .lines()
        .find(|l| l.contains("util.js"))
        .expect("util.js row not found in JS section");
    // The row format is `class  score  fan_in  complexity  file`;
    // the fan_in column is the third whitespace-delimited field.
    let fields: Vec<&str> = util_line.split_whitespace().collect();
    assert!(fields.len() >= 5, "unexpected row layout: {util_line:?}");
    let fan_in: usize = fields[2].parse().expect("fan_in is numeric");
    assert!(fan_in >= 1, "util.js should have fan_in >= 1, got {fan_in}");
}

#[test]
fn same_basename_distinct_paths_do_not_collapse() {
    // Ohm P1 on PR #81: `a/index.js` and `b/index.js` have the same
    // Zeitwerk basename ("Index") but must stay distinct via
    // path-exact edges. A caller that imports only `./a/index.js`
    // must NOT lift b/index.js's fan_in.
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

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(root).output().expect("stud-finder");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let js_section = stdout.split_once("# JavaScript").map(|(_, s)| s).unwrap();
    let find_row = |path: &str| {
        js_section
            .lines()
            .find(|l| l.contains(path))
            .unwrap_or_else(|| panic!("row for {path} not found"))
    };
    let fan_in_of = |path: &str| -> usize {
        let fields: Vec<&str> = find_row(path).split_whitespace().collect();
        fields[2].parse().expect("fan_in numeric")
    };
    assert_eq!(
        fan_in_of("a/index.js"),
        1,
        "a/index.js should see the import"
    );
    assert_eq!(fan_in_of("b/index.js"), 0, "b/index.js must NOT inherit");
}

#[test]
fn non_js_extension_specifier_never_rewrites_to_js() {
    // Ohm P1 on PR #81 re-review: `./foo.json` (or any non-JS
    // extension) must NOT get rewritten to `./foo.ts`. The previous
    // fix only protected JS extensions — unprotected extensions
    // still fell into the suffix-inference branch.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("foo.ts"), "export const foo = 1;\n").unwrap();
    // No foo.json exists on disk. The caller still imports it.
    std::fs::write(
        root.join("caller.ts"),
        "import data from './foo.json';\nconsole.log(data);\n",
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(root).output().expect("stud-finder");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let js_section = stdout.split_once("# JavaScript").map(|(_, s)| s).unwrap();
    let row = js_section
        .lines()
        .find(|l| l.contains("foo.ts"))
        .expect("foo.ts row");
    let fields: Vec<&str> = row.split_whitespace().collect();
    let fan_in: usize = fields[2].parse().unwrap();
    assert_eq!(
        fan_in, 0,
        "`./foo.json` specifier must NOT be rewritten to foo.ts"
    );
}

#[test]
fn explicit_extension_does_not_fall_through_to_other_extensions() {
    // Ohm P1 on PR #81: importing `./foo.js` while a `foo.ts` also
    // exists must NOT silently resolve to foo.ts. Non-existing
    // explicit extension → no edge, no false lift.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("foo.ts"), "export const foo = 1;\n").unwrap();
    // No foo.js — the explicit specifier `./foo.js` points at
    // nothing. foo.ts must NOT inherit the edge.
    std::fs::write(
        root.join("caller.js"),
        "import { foo } from './foo.js';\nconsole.log(foo);\n",
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(root).output().expect("stud-finder");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let js_section = stdout.split_once("# JavaScript").map(|(_, s)| s).unwrap();
    let row = js_section
        .lines()
        .find(|l| l.contains("foo.ts"))
        .expect("foo.ts row");
    let fields: Vec<&str> = row.split_whitespace().collect();
    let fan_in: usize = fields[2].parse().unwrap();
    assert_eq!(fan_in, 0, "explicit .js import must NOT create edge to .ts");
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

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(root).output().expect("stud-finder");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("util.mjs"), "util.mjs should be collected");
    let js_section = stdout.split_once("# JavaScript").map(|(_, s)| s).unwrap();
    let row = js_section
        .lines()
        .find(|l| l.contains("util.mjs"))
        .expect("util.mjs row");
    let fields: Vec<&str> = row.split_whitespace().collect();
    let fan_in: usize = fields[2].parse().unwrap();
    assert!(fan_in >= 1, "util.mjs should have fan_in >= 1");
}

#[test]
fn malformed_js_is_excluded_not_imputed() {
    // Mirror of PR #78's Ruby exclusion policy for JS: a parse-failed
    // file must not be registered + must not sink neighbours. Build
    // two versions: one with a broken file, one without; the clean
    // files' scores must be identical across both runs.
    use std::process::Command;
    let make_run = |include_broken: bool| -> String {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("a.js"), "export const a = 1;\n").unwrap();
        std::fs::write(root.join("b.js"), "import { a } from './a';\n").unwrap();
        if include_broken {
            std::fs::write(root.join("broken.js"), "function f( { !!! what").unwrap();
        }
        let bin = env!("CARGO_BIN_EXE_stud-finder");
        let out = Command::new(bin).arg(root).output().expect("stud-finder");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let clean = make_run(false);
    let mixed = make_run(true);
    // Extract the "a.js" row from each JS section — the score should
    // match exactly (broken file didn't register, didn't shift pcts).
    let a_row = |stdout: &str| -> String {
        let js = stdout.split_once("# JavaScript").map(|(_, s)| s).unwrap();
        js.lines()
            .find(|l| l.contains("a.js") && !l.contains("b.js"))
            .expect("a.js row")
            .to_string()
    };
    let clean_row = a_row(&clean);
    let mixed_row = a_row(&mixed);
    let clean_fields: Vec<&str> = clean_row.split_whitespace().collect();
    let mixed_fields: Vec<&str> = mixed_row.split_whitespace().collect();
    assert_eq!(
        clean_fields[1], mixed_fields[1],
        "broken.js must not shift neighbours' scores: {clean_fields:?} vs {mixed_fields:?}"
    );
    // And broken.js itself must NOT appear in the JS section.
    assert!(
        !mixed.contains("broken.js"),
        "broken.js must be excluded from output: {mixed}"
    );
}

#[test]
fn no_js_files_keeps_only_ruby_section() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("a.rb"), "class A\nend\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_stud-finder");
    let out = Command::new(bin).arg(root).output().expect("stud-finder");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Ruby"));
    assert!(
        !stdout.contains("# JavaScript"),
        "JS header must not render without JS files: {stdout}"
    );
}
