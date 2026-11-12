//! Output format emitters for sf-cli: `table` (the default from the
//! earlier slices), `json`, `csv`, `markdown`. Each format emits the
//! Ruby and (when present) JavaScript sections side-by-side; the Ruby
//! section carries the full 7-signal shape (fan_in / fan_out /
//! complexity / churn / coupling / coverage / score / class), the JS
//! section a narrower fan_in / fan_out / complexity shape.
//!
//! JSON: one top-level object `{ ruby: [...], javascript: [...] }`
//! with every ScoredFile field on each row (raw + percentile +
//! composite + class + evidence + coverage partner). Missing
//! sections are dropped (never present as empty arrays).
//! CSV: one header row per section; the JS section prefaced by a
//! `# JavaScript` comment line matching the table output.
//! Markdown: pipe-tables with headers, one block per section headed
//! `# Ruby` / `# JavaScript`.
//!
//! All emitters leave empty sections silent — a repo with no Ruby
//! files emits only the JS block, and vice versa.

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
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
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
            r.path.display(),
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
            r.path.display(),
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
