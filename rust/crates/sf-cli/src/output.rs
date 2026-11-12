//! Output format emitters for sf-cli: `table` (the default from the
//! earlier slices), `json`, `csv`, `markdown`.
//!
//! **IMPORTANT — these are sf-rust-native formats, NOT drop-in
//! Ruby-compatible replacements.** Ohm's PR #82 review flagged that
//! stud-finder Ruby's JSON contract includes `meta` + `warnings` +
//! both section arrays (always present, never dropped) with row
//! fields like `score` (not `composite`), `rank`, `language`,
//! `newness`, `instability`, `loc`, `escalation`, and
//! `coupling_partners` — none of which are implemented yet. Ruby
//! CSV is a single stream with a `language` column and a
//! RESULT_COLUMNS header, not two comment-separated sections.
//!
//! Treat these formats as a partial initial implementation. The
//! parity surface is tracked as a compatibility backlog on the
//! stud-finder Rust port (see PR #82 discussion + ATTRIBUTION.md).
//!
//! Current shape:
//!
//! - JSON: `{ ruby: [...], javascript: [...] }`. Missing sections
//!   drop out. Row fields = every ScoredFile field (raw signals +
//!   `_pct` percentiles + composite + class + evidence + coupling
//!   partner). No meta or warnings block yet.
//! - CSV: `# Ruby` / `# JavaScript` comment headers separating two
//!   independent sections with their own column headers.
//! - Markdown: pipe-tables with headers + separator rows per section.
//!
//! All emitters leave empty sections silent.

use clap::ValueEnum;
use serde_json::{Map, Value, json};
use sf_score::{Class, ScoredFile};

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

pub struct Sections<'a> {
    pub ruby: &'a [ScoredFile],
    pub js: &'a [ScoredFile],
}

pub fn emit(format: OutputFormat, sections: Sections<'_>) {
    match format {
        OutputFormat::Table => unreachable!("table rendering stays inline in main.rs"),
        OutputFormat::Json => emit_json(sections),
        OutputFormat::Csv => emit_csv(sections),
        OutputFormat::Markdown => emit_markdown(sections),
    }
}

fn row_json(r: &ScoredFile) -> Value {
    let mut m: Map<String, Value> = Map::new();
    m.insert("path".into(), json!(r.path.display().to_string()));
    m.insert("class".into(), json!(class_str(r.class)));
    m.insert("composite".into(), json!(r.composite));
    m.insert("fan_in".into(), json!(r.fan_in));
    m.insert("fan_out".into(), json!(r.fan_out));
    m.insert("complexity".into(), json!(r.complexity));
    m.insert("churn_commits".into(), json!(r.churn_commits));
    m.insert("churn_lines".into(), json!(r.churn_lines));
    m.insert("max_coupling".into(), json!(r.max_coupling));
    m.insert(
        "max_coupling_partner".into(),
        json!(r.max_coupling_partner.display().to_string()),
    );
    m.insert("coverage".into(), json!(r.coverage));
    m.insert("fan_in_pct".into(), json!(r.fan_in_pct));
    m.insert("fan_out_pct".into(), json!(r.fan_out_pct));
    m.insert("complexity_pct".into(), json!(r.complexity_pct));
    m.insert("churn_pct".into(), json!(r.churn_pct));
    m.insert("coupling_pct".into(), json!(r.coupling_pct));
    m.insert("coverage_pct".into(), json!(r.coverage_pct));
    m.insert("evidence".into(), json!(r.evidence));
    Value::Object(m)
}

fn emit_json(sections: Sections<'_>) {
    let mut root = Map::new();
    if !sections.ruby.is_empty() {
        let rows: Vec<Value> = sections.ruby.iter().map(row_json).collect();
        root.insert("ruby".into(), Value::Array(rows));
    }
    if !sections.js.is_empty() {
        let rows: Vec<Value> = sections.js.iter().map(row_json).collect();
        root.insert("javascript".into(), Value::Array(rows));
    }
    let out = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".into());
    println!("{out}");
}

fn emit_csv(sections: Sections<'_>) {
    if !sections.ruby.is_empty() {
        println!("# Ruby");
        emit_csv_ruby(sections.ruby);
    }
    if !sections.js.is_empty() {
        if !sections.ruby.is_empty() {
            println!();
        }
        println!("# JavaScript");
        emit_csv_js(sections.js);
    }
}

const RUBY_HEADERS: &[&str] = &[
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

const JS_HEADERS: &[&str] = &["class", "score", "fan_in", "fan_out", "complexity", "file"];

fn emit_csv_ruby(rows: &[ScoredFile]) {
    println!("{}", RUBY_HEADERS.join(","));
    for r in rows {
        println!(
            "{},{},{},{},{},{},{},{},{}",
            class_str(r.class),
            format_score(r.composite),
            r.fan_in,
            r.fan_out,
            r.complexity,
            r.churn_commits,
            format_ratio(r.max_coupling),
            format_ratio(r.coverage),
            csv_escape(&r.path.display().to_string()),
        );
    }
}

fn emit_csv_js(rows: &[ScoredFile]) {
    println!("{}", JS_HEADERS.join(","));
    for r in rows {
        println!(
            "{},{},{},{},{},{}",
            class_str(r.class),
            format_score(r.composite),
            r.fan_in,
            r.fan_out,
            r.complexity,
            csv_escape(&r.path.display().to_string()),
        );
    }
}

fn csv_escape(s: &str) -> String {
    // Per RFC 4180 fields containing ',', '"', CR, or LF must be
    // double-quoted and inner quotes doubled.
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Escape a cell value for a Markdown pipe-table row. Pipes are
/// HTML-encoded as `&#124;` so the raw character never appears
/// inside a cell — pipe-table renderers decode the entity to a
/// literal `|`, but the raw source stays structurally parseable
/// (a `\|` backslash escape renders correctly too but still writes
/// a `|` byte, which confuses line-based pipe-counting). Newlines
/// and carriage returns are HTML-encoded for the same reason.
/// Ohm's non-blocking note on PR #82 flagged pipe/newline paths
/// corrupting the table.
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

fn emit_markdown_ruby(rows: &[ScoredFile]) {
    println!("| {} |", RUBY_HEADERS.join(" | "));
    println!("| {} |", vec!["---"; RUBY_HEADERS.len()].join(" | "));
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
    println!("| {} |", JS_HEADERS.join(" | "));
    println!("| {} |", vec!["---"; JS_HEADERS.len()].join(" | "));
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
