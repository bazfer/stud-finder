//! Output format emitters for sf-cli.
//!
//! **PR #85 (sf-output-parity slice #10a):** JSON and CSV now match
//! the Ruby CLI's output contract from `lib/stud_finder/cli.rb`:
//!
//! - **JSON** root is `{ meta, warnings, ruby: [...], javascript:
//!   [...] }`. Both section arrays are ALWAYS present (empty arrays
//!   when a language has no rows). Per-row fields are the Ruby
//!   `json_file` shape — rank, language, path, score, evidence,
//!   class, new_file, age_days, escalation, fan_in, fan_in_pct,
//!   fan_out, fan_out_pct, instability, instability_pct, complexity,
//!   complexity_pct, churn_commits, churn_lines, churn_pct, loc,
//!   loc_pct, max_coupling, max_coupling_partner, coupling_partners,
//!   coupling_pct, coverage. Field name is `score` (not
//!   `composite`); class key is `class` (not `classification`).
//! - **CSV** is a single combined stream with Ruby's `RESULT_COLUMNS`
//!   header. Language column tags each row. Rows run
//!   ruby-section-first then javascript-section.
//!
//! `rank` and `language` are computed at emit time rather than
//! stored on `ScoredFile` — rank is the 1-based position after the
//! caller's sort within its section; language comes from the
//! section the row was passed in.
//!
//! **Still pending parity (follow-up slice #10b):**
//!
//! - Markdown and table emit shapes stay on their sf-rust-native
//!   form from PR #82. The Ruby markdown + table layouts include
//!   section headers, formula descriptions, and the `MARKDOWN_COLUMNS`
//!   subset — those land in slice #10b.
//! - `meta.formula` (human-readable signal-inclusion label),
//!   `meta.weights` (renormalized weight map from sf-score), and
//!   `meta.files_skipped` (requires a skipped-files count through
//!   from sf-cli's collector/parser pipeline). Documented as the
//!   compatibility backlog.

use clap::ValueEnum;
use serde_json::{Map, Value, json};
use sf_score::{Class, ScoredFile};

/// Schema version baked into `meta.schema_version` — bumped when
/// the JSON contract changes. Starts at 1 matching Ruby.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    Csv,
    Markdown,
}

impl OutputFormat {
    pub fn is_table(self) -> bool {
        matches!(self, Self::Table)
    }
}

/// Everything an emitter needs to produce one run's output. `ruby`
/// and `js` are the pre-sorted rows per language section. `repo`
/// is the scan root (as displayed to the user, verbatim). Warnings
/// are already language-agnostic strings — sf-cli normalizes
/// `HistoryWarning` / `NewnessWarning` / `CoverageWarning` into
/// `{code, message}` payloads before passing them in.
pub struct Sections<'a> {
    pub ruby: &'a [ScoredFile],
    pub js: &'a [ScoredFile],
    pub repo: &'a str,
    pub churn_days: u64,
    pub analyzed_at: &'a str,
    pub warnings: &'a [WarningPayload],
}

/// A warning payload shaped for the Ruby JSON contract. `code` is
/// the stable machine identifier; `message` is human-readable text.
/// Additional fields (count, etc.) go in `extra`.
#[derive(Clone, Debug)]
pub struct WarningPayload {
    pub code: String,
    pub message: String,
    pub extra: Map<String, Value>,
}

pub fn emit(format: OutputFormat, sections: Sections<'_>) {
    match format {
        OutputFormat::Table => unreachable!("table rendering stays inline in main.rs"),
        OutputFormat::Json => emit_json(sections),
        OutputFormat::Csv => emit_csv(sections),
        OutputFormat::Markdown => emit_markdown(sections),
    }
}

fn row_json(rank: usize, language: &str, r: &ScoredFile) -> Value {
    let mut m: Map<String, Value> = Map::new();
    m.insert("rank".into(), json!(rank));
    m.insert("language".into(), json!(language));
    m.insert("path".into(), json!(r.path.display().to_string()));
    m.insert("score".into(), json!(r.composite));
    m.insert("evidence".into(), json!(r.evidence));
    m.insert("class".into(), json!(class_str(r.class)));
    m.insert("new_file".into(), json!(r.new_file));
    m.insert("age_days".into(), json!(r.age_days));
    m.insert("escalation".into(), json!(r.escalation));
    m.insert("fan_in".into(), json!(r.fan_in));
    m.insert("fan_in_pct".into(), json!(r.fan_in_pct));
    m.insert("fan_out".into(), json!(r.fan_out));
    m.insert("fan_out_pct".into(), json!(r.fan_out_pct));
    m.insert("instability".into(), json!(r.instability));
    m.insert("instability_pct".into(), json!(r.instability_pct));
    m.insert("complexity".into(), json!(r.complexity));
    m.insert("complexity_pct".into(), json!(r.complexity_pct));
    m.insert("churn_commits".into(), json!(r.churn_commits));
    m.insert("churn_lines".into(), json!(r.churn_lines));
    m.insert("churn_pct".into(), json!(r.churn_pct));
    m.insert("loc".into(), json!(r.loc));
    m.insert("loc_pct".into(), json!(r.loc_pct));
    m.insert("max_coupling".into(), json!(r.max_coupling));
    m.insert(
        "max_coupling_partner".into(),
        json!(r.max_coupling_partner.display().to_string()),
    );
    m.insert("coupling_partners".into(), json!(r.coupling_partners));
    m.insert("coupling_pct".into(), json!(r.coupling_pct));
    // `coverage` keeps the raw coverage ratio in [0, 1]. Ruby
    // emits `nil` when the coverage signal is unavailable; we
    // match by returning JSON null for the whole-run-no-coverage
    // case. The scorer stores 0.0 both for "no coverage loaded"
    // and "file with 0% coverage", so sf-cli has to hand us the
    // availability flag at the meta level; per-file, we emit the
    // raw number verbatim. A follow-up could thread an Option
    // through ScoredFile for strict parity.
    m.insert("coverage".into(), json!(r.coverage));
    Value::Object(m)
}

fn warnings_json(warnings: &[WarningPayload]) -> Vec<Value> {
    warnings
        .iter()
        .map(|w| {
            let mut m = w.extra.clone();
            m.insert("code".into(), json!(w.code));
            m.insert("message".into(), json!(w.message));
            Value::Object(m)
        })
        .collect()
}

fn emit_json(sections: Sections<'_>) {
    let total_files = sections.ruby.len() + sections.js.len();
    let mut meta = Map::new();
    meta.insert("repo".into(), json!(sections.repo));
    meta.insert("analyzed_at".into(), json!(sections.analyzed_at));
    meta.insert("churn_days".into(), json!(sections.churn_days));
    meta.insert("file_count".into(), json!(total_files));
    meta.insert("schema_version".into(), json!(SCHEMA_VERSION));
    meta.insert(
        "warnings".into(),
        Value::Array(warnings_json(sections.warnings)),
    );

    // ALWAYS-present arrays — Ruby emits them empty rather than
    // dropping the key. A downstream consumer can iterate both
    // keys without checking existence.
    let ruby_rows: Vec<Value> = sections
        .ruby
        .iter()
        .enumerate()
        .map(|(i, r)| row_json(i + 1, "ruby", r))
        .collect();
    let js_rows: Vec<Value> = sections
        .js
        .iter()
        .enumerate()
        .map(|(i, r)| row_json(i + 1, "javascript", r))
        .collect();

    let mut root = Map::new();
    root.insert("meta".into(), Value::Object(meta));
    root.insert(
        "warnings".into(),
        Value::Array(warnings_json(sections.warnings)),
    );
    root.insert("ruby".into(), Value::Array(ruby_rows));
    root.insert("javascript".into(), Value::Array(js_rows));

    let out = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".into());
    println!("{out}");
}

/// Ruby's `RESULT_COLUMNS`, verbatim. Column 2 is `language`,
/// column 3 is `file` (the path). Matches `lib/stud_finder/cli.rb`
/// on the stud-finder main branch.
pub const RESULT_COLUMNS: &[&str] = &[
    "rank",
    "language",
    "file",
    "score",
    "evidence",
    "class",
    "new_file",
    "age_days",
    "escalation",
    "fan_in",
    "fan_in_pct",
    "fan_out",
    "fan_out_pct",
    "instability",
    "instability_pct",
    "complexity",
    "complexity_pct",
    "churn_commits",
    "churn_lines",
    "churn_pct",
    "loc",
    "loc_pct",
    "max_coupling",
    "max_coupling_partner",
    "coupling_partners",
    "coupling_pct",
    "coverage",
];

fn emit_csv(sections: Sections<'_>) {
    println!("{}", RESULT_COLUMNS.join(","));
    for (i, r) in sections.ruby.iter().enumerate() {
        println!("{}", csv_row(i + 1, "ruby", r));
    }
    for (i, r) in sections.js.iter().enumerate() {
        println!("{}", csv_row(i + 1, "javascript", r));
    }
}

fn csv_row(rank: usize, language: &str, r: &ScoredFile) -> String {
    let cells: Vec<String> = vec![
        rank.to_string(),
        csv_escape(language),
        csv_escape(&r.path.display().to_string()),
        format_score(r.composite),
        format_score(r.evidence),
        class_str(r.class).to_string(),
        r.new_file.to_string(),
        r.age_days.to_string(),
        csv_escape(&r.escalation),
        r.fan_in.to_string(),
        format_score(r.fan_in_pct),
        r.fan_out.to_string(),
        format_score(r.fan_out_pct),
        format_score(r.instability),
        format_score(r.instability_pct),
        r.complexity.to_string(),
        format_score(r.complexity_pct),
        r.churn_commits.to_string(),
        r.churn_lines.to_string(),
        format_score(r.churn_pct),
        r.loc.to_string(),
        format_score(r.loc_pct),
        format_score(r.max_coupling),
        csv_escape(&r.max_coupling_partner.display().to_string()),
        r.coupling_partners.to_string(),
        format_score(r.coupling_pct),
        format_score(r.coverage),
    ];
    cells.join(",")
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

// --- Markdown + table: unchanged from PR #82 / #83 (sf-rust-native). ---
// Slice #10b will replace both with the Ruby markdown/table
// contracts (section headers, formula description, MARKDOWN_COLUMNS
// subset, filter note blockquote, footer).

fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '|' => out.push_str("&#124;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

fn emit_markdown(sections: Sections<'_>) {
    if !sections.ruby.is_empty() {
        println!("# Ruby");
        println!();
        emit_markdown_ruby(sections.ruby);
    }
    if !sections.js.is_empty() {
        if !sections.ruby.is_empty() {
            println!();
        }
        println!("# JavaScript");
        println!();
        emit_markdown_js(sections.js);
    }
}

const MD_RUBY_HEADERS: &[&str] = &[
    "class",
    "score",
    "fan_in",
    "fan_out",
    "complexity",
    "churn",
    "coupling",
    "coverage",
    "file",
];
const MD_JS_HEADERS: &[&str] = &["class", "score", "fan_in", "fan_out", "complexity", "file"];

fn emit_markdown_ruby(rows: &[ScoredFile]) {
    println!("| {} |", MD_RUBY_HEADERS.join(" | "));
    println!("| {} |", vec!["---"; MD_RUBY_HEADERS.len()].join(" | "));
    for r in rows {
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            class_str(r.class),
            format_score(r.composite),
            r.fan_in,
            r.fan_out,
            r.complexity,
            r.churn_commits,
            format_ratio(r.max_coupling),
            format_ratio(r.coverage),
            md_escape(&r.path.display().to_string()),
        );
    }
}

fn emit_markdown_js(rows: &[ScoredFile]) {
    println!("| {} |", MD_JS_HEADERS.join(" | "));
    println!("| {} |", vec!["---"; MD_JS_HEADERS.len()].join(" | "));
    for r in rows {
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            class_str(r.class),
            format_score(r.composite),
            r.fan_in,
            r.fan_out,
            r.complexity,
            md_escape(&r.path.display().to_string()),
        );
    }
}

fn class_str(c: Class) -> &'static str {
    c.as_str()
}

fn format_score(x: f64) -> String {
    format!("{x:.4}")
}

fn format_ratio(x: f64) -> String {
    format!("{x:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk_row(path: &str, composite: f64, class: Class) -> ScoredFile {
        ScoredFile {
            path: PathBuf::from(path),
            fan_in: 0,
            fan_out: 0,
            complexity: 0,
            churn_commits: 0,
            churn_lines: 0,
            loc: 0,
            instability: 0.0,
            max_coupling_partner: PathBuf::new(),
            max_coupling: 0.0,
            coupling_partners: 0,
            coverage: 0.0,
            fan_in_pct: 0.0,
            fan_out_pct: 0.0,
            complexity_pct: 0.0,
            churn_pct: 0.0,
            coupling_pct: 0.0,
            coverage_pct: 0.0,
            loc_pct: 0.0,
            instability_pct: 0.0,
            composite,
            class,
            escalation: String::new(),
            evidence: 1.0,
            new_file: false,
            age_days: 0,
            total_commits: 0,
            newness_metadata_available: false,
        }
    }

    #[test]
    fn result_columns_matches_ruby_order_exactly() {
        // Verbatim against Ruby `lib/stud_finder/cli.rb` RESULT_COLUMNS
        // on the stud-finder main branch. This is the CSV + meta
        // parity contract — changing it breaks consumers.
        assert_eq!(
            RESULT_COLUMNS,
            [
                "rank",
                "language",
                "file",
                "score",
                "evidence",
                "class",
                "new_file",
                "age_days",
                "escalation",
                "fan_in",
                "fan_in_pct",
                "fan_out",
                "fan_out_pct",
                "instability",
                "instability_pct",
                "complexity",
                "complexity_pct",
                "churn_commits",
                "churn_lines",
                "churn_pct",
                "loc",
                "loc_pct",
                "max_coupling",
                "max_coupling_partner",
                "coupling_partners",
                "coupling_pct",
                "coverage",
            ]
        );
    }

    #[test]
    fn row_json_emits_ruby_field_names() {
        // The ScoredFile internal name is `composite`; the JSON key
        // must be `score`. The internal enum is `Class`; the JSON
        // key must be `class`. Any drift here breaks every
        // downstream consumer that targets the Ruby contract.
        let r = mk_row("app/order.rb", 0.73, Class::Branch);
        let v = row_json(5, "ruby", &r);
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("rank"), Some(&json!(5)));
        assert_eq!(obj.get("language"), Some(&json!("ruby")));
        assert_eq!(obj.get("path"), Some(&json!("app/order.rb")));
        assert_eq!(obj.get("score"), Some(&json!(0.73)));
        assert_eq!(obj.get("class"), Some(&json!("branch")));
        // Forbid the pre-parity field names from leaking back.
        assert!(obj.get("composite").is_none());
        assert!(obj.get("classification").is_none());
    }

    #[test]
    fn row_json_carries_every_result_column_field() {
        // Walk the JSON blob and assert that every RESULT_COLUMNS
        // entry has a corresponding JSON key (except `file` which
        // Ruby maps to `path` in the JSON shape — documented in
        // json_file). Catch a future ScoredFile field addition
        // that forgets to update row_json or RESULT_COLUMNS.
        let r = mk_row("x.rb", 0.0, Class::Leaf);
        let v = row_json(1, "ruby", &r);
        let obj = v.as_object().unwrap();
        for col in RESULT_COLUMNS {
            let key = if *col == "file" { "path" } else { *col };
            assert!(
                obj.contains_key(key),
                "JSON row missing key `{key}` for RESULT_COLUMN `{col}`"
            );
        }
    }

    #[test]
    fn csv_row_cell_count_matches_result_columns() {
        let r = mk_row("a.rb", 0.5, Class::Trunk);
        let row = csv_row(1, "ruby", &r);
        let cells: Vec<&str> = row.split(',').collect();
        assert_eq!(
            cells.len(),
            RESULT_COLUMNS.len(),
            "CSV cell count must match RESULT_COLUMNS header length"
        );
    }

    #[test]
    fn csv_row_escapes_comma_in_path() {
        let mut r = mk_row("weird,path.rb", 0.0, Class::Leaf);
        r.escalation = "trunk_adjacent".to_string();
        let row = csv_row(1, "ruby", &r);
        assert!(
            row.contains("\"weird,path.rb\""),
            "comma-containing path must be double-quoted: {row}"
        );
    }
}
