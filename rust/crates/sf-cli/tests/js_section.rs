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
