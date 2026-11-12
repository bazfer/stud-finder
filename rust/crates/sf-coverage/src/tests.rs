use super::*;
use std::io::Write;

fn tmp_write(name: &str, contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    (dir, path)
}

fn files_vec(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

fn refs(paths: &[PathBuf]) -> Vec<&Path> {
    paths.iter().map(PathBuf::as_path).collect()
}

// --- SimpleCov resultset.json ---

#[test]
fn simplecov_flat_shape_rate_matches_covered_over_nonnil() {
    // Flat `{coverage: ...}` shape with per-file hash+lines.
    // Lines [1, 0, null, 2] → non-nil = 3, hit = 2 → rate = 2/3.
    let json = r#"{
        "coverage": {
            "app/models/order.rb": { "lines": [1, 0, null, 2] }
        }
    }"#;
    let (dir, path) = tmp_write("rs.json", json);
    let files = files_vec(&["app/models/order.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert!(
        (report.coverage_pct[&PathBuf::from("app/models/order.rb")] - 2.0_f64 / 3.0_f64).abs()
            < 1e-9
    );
    assert!(report.missing_files.is_empty());
}

#[test]
fn simplecov_suite_nested_shape() {
    // SimpleCov format: suite → coverage → files.
    let json = r#"{
        "RSpec": {
            "coverage": {
                "lib/a.rb": [1, 1, null, 0]
            }
        }
    }"#;
    let (dir, path) = tmp_write("rs.json", json);
    let files = files_vec(&["lib/a.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    // Non-nil = 3, hit = 2 → 2/3.
    assert!((report.coverage_pct[&PathBuf::from("lib/a.rb")] - 2.0_f64 / 3.0_f64).abs() < 1e-9);
}

#[test]
fn simplecov_merges_suites_via_per_line_max() {
    // Two suites, same file. Position 0: 0 ∨ 1 = 1. Position 1: nil ∨ 0 = 0.
    // Position 2: 3 ∨ nil = 3. Non-nil = 3, hit = 2 (positions 0 and 2).
    let json = r#"{
        "RSpec": { "coverage": { "a.rb": [0, null, 3] } },
        "Minitest": { "coverage": { "a.rb": [1, 0, null] } }
    }"#;
    let (dir, path) = tmp_write("rs.json", json);
    let files = files_vec(&["a.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert!((report.coverage_pct[&PathBuf::from("a.rb")] - 2.0_f64 / 3.0_f64).abs() < 1e-9);
}

#[test]
fn simplecov_absolute_path_strips_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let root_str = dir.path().to_str().unwrap().to_string();
    let json = format!(
        r#"{{ "coverage": {{ "{root_str}/app/x.rb": [1, 1, 1] }} }}"#,
        root_str = root_str
    );
    let path = dir.path().join("rs.json");
    std::fs::write(&path, &json).unwrap();
    let files = files_vec(&["app/x.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert!((report.coverage_pct[&PathBuf::from("app/x.rb")] - 1.0).abs() < 1e-9);
}

#[test]
fn simplecov_absolute_path_falls_back_to_suffix_match() {
    // Absolute path with a prefix that's NOT the project root — Ruby
    // suffix-matches against the file set.
    let dir = tempfile::tempdir().unwrap();
    let json = r#"{ "coverage": { "/elsewhere/absolute/app/models/user.rb": [1, 0] } }"#;
    let path = dir.path().join("rs.json");
    std::fs::write(&path, json).unwrap();
    let files = files_vec(&["app/models/user.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert!(
        (report.coverage_pct[&PathBuf::from("app/models/user.rb")] - 0.5).abs() < 1e-9,
        "suffix match should identify app/models/user.rb"
    );
}

#[test]
fn simplecov_all_nil_lines_is_zero_rate() {
    let json = r#"{ "coverage": { "x.rb": [null, null, null] } }"#;
    let (dir, path) = tmp_write("rs.json", json);
    let files = files_vec(&["x.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert_eq!(report.coverage_pct[&PathBuf::from("x.rb")], 0.0);
}

#[test]
fn simplecov_missing_file_lands_in_missing_files() {
    let json = r#"{ "coverage": { "a.rb": [1, 1] } }"#;
    let (dir, path) = tmp_write("rs.json", json);
    let files = files_vec(&["a.rb", "b.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert_eq!(report.missing_files, vec![PathBuf::from("b.rb")]);
}

#[test]
fn simplecov_malformed_json_is_error() {
    let (dir, path) = tmp_write("rs.json", "not json at all {");
    let files: Vec<PathBuf> = vec![];
    let err = load(&path, dir.path(), &refs(&files));
    assert!(matches!(err, Err(CoverageError::MalformedJson(_))));
}

// --- LCOV ---

#[test]
fn lcov_lf_lh_direct_rate() {
    let lcov = "SF:lib/calc.rb
LF:10
LH:7
end_of_record
";
    let (dir, path) = tmp_write("coverage.info", lcov);
    let files = files_vec(&["lib/calc.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert!((report.coverage_pct[&PathBuf::from("lib/calc.rb")] - 0.7).abs() < 1e-9);
}

#[test]
fn lcov_da_fallback_without_lf_lh() {
    let lcov = "SF:a.rb
DA:1,5
DA:2,0
DA:3,2
DA:4,0
end_of_record
";
    let (dir, path) = tmp_write("coverage.info", lcov);
    let files = files_vec(&["a.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    // 4 DA lines, 2 positive → 0.5.
    assert!((report.coverage_pct[&PathBuf::from("a.rb")] - 0.5).abs() < 1e-9);
}

#[test]
fn lcov_hit_exceeds_found_is_error() {
    let lcov = "SF:bad.rb
LF:3
LH:5
end_of_record
";
    let (dir, path) = tmp_write("coverage.info", lcov);
    let files = files_vec(&["bad.rb"]);
    let err = load(&path, dir.path(), &refs(&files));
    assert!(matches!(
        err,
        Err(CoverageError::LineHitsExceedLinesFound(_))
    ));
}

#[test]
fn lcov_zero_lf_is_zero_rate_not_div_by_zero() {
    let lcov = "SF:empty.rb
LF:0
LH:0
end_of_record
";
    let (dir, path) = tmp_write("coverage.info", lcov);
    let files = files_vec(&["empty.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert_eq!(report.coverage_pct[&PathBuf::from("empty.rb")], 0.0);
}

#[test]
fn lcov_multiple_records() {
    let lcov = "SF:a.rb
LF:4
LH:4
end_of_record
SF:b.rb
LF:5
LH:1
end_of_record
";
    let (dir, path) = tmp_write("coverage.info", lcov);
    let files = files_vec(&["a.rb", "b.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert_eq!(report.coverage_pct[&PathBuf::from("a.rb")], 1.0);
    assert!((report.coverage_pct[&PathBuf::from("b.rb")] - 0.2).abs() < 1e-9);
}

// --- Cobertura XML ---

#[test]
fn cobertura_line_rate_in_range_passes_through() {
    let xml = r#"<?xml version="1.0"?>
<coverage>
  <packages>
    <package>
      <classes>
        <class filename="lib/a.rb" line-rate="0.8"/>
        <class filename="./lib/b.rb" line-rate="0.5"/>
      </classes>
    </package>
  </packages>
</coverage>"#;
    let (dir, path) = tmp_write("cov.xml", xml);
    let files = files_vec(&["lib/a.rb", "lib/b.rb"]);
    let report = load(&path, dir.path(), &refs(&files)).unwrap();
    assert_eq!(report.coverage_pct[&PathBuf::from("lib/a.rb")], 0.8);
    // `./` prefix stripped per Ruby.
    assert_eq!(report.coverage_pct[&PathBuf::from("lib/b.rb")], 0.5);
}

#[test]
fn cobertura_missing_line_rate_is_error() {
    let xml = r#"<?xml version="1.0"?>
<coverage><packages><package><classes>
  <class filename="a.rb"/>
</classes></package></packages></coverage>"#;
    let (dir, path) = tmp_write("cov.xml", xml);
    let files: Vec<PathBuf> = vec![];
    let err = load(&path, dir.path(), &refs(&files));
    assert!(matches!(err, Err(CoverageError::MissingLineRate(_))));
}

#[test]
fn cobertura_line_rate_out_of_range_is_error() {
    let xml = r#"<?xml version="1.0"?>
<coverage><packages><package><classes>
  <class filename="a.rb" line-rate="1.5"/>
</classes></package></packages></coverage>"#;
    let (dir, path) = tmp_write("cov.xml", xml);
    let files: Vec<PathBuf> = vec![];
    let err = load(&path, dir.path(), &refs(&files));
    assert!(matches!(err, Err(CoverageError::LineRateOutOfRange(_))));
}

#[test]
fn cobertura_malformed_xml_is_error() {
    let (dir, path) = tmp_write("cov.xml", "<coverage><not>");
    let files: Vec<PathBuf> = vec![];
    let err = load(&path, dir.path(), &refs(&files));
    assert!(matches!(err, Err(CoverageError::MalformedXml(_))));
}

// --- dispatch / general ---

#[test]
fn unsupported_extension_is_error() {
    let (dir, path) = tmp_write("coverage.txt", "whatever");
    let files: Vec<PathBuf> = vec![];
    let err = load(&path, dir.path(), &refs(&files));
    assert!(matches!(err, Err(CoverageError::UnsupportedExtension(_))));
}

#[test]
fn file_not_found_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = vec![];
    let err = load(&dir.path().join("nope.json"), dir.path(), &refs(&files));
    assert!(matches!(err, Err(CoverageError::FileNotFound(_))));
}
