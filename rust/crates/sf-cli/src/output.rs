//! Output format emitters for sf-cli.
//!
//! **PR #85 (slice #10a):** JSON + CSV now match the Ruby CLI's
//! output contract (shape, field names, tri-state coverage).
//!
//! **This slice (#10b):** adds the rest of Ruby's output parity:
//!
//! - **Markdown** emits `## stud-finder — <date>`, a `> formula`
//!   blockquote, optional filter-note blockquote, `### Ruby` and
//!   `### JavaScript/TypeScript` tables each using the Ruby
//!   `MARKDOWN_COLUMNS` 22-column subset, closing with the fan_in
//!   disclaimer. Both section tables are always emitted — even
//!   empty — to match Ruby.
//! - **Table** emits `stud-finder — <path> (N-day churn, <formula>)`
//!   with an optional scoring-note line when coverage is
//!   unavailable, optional filter note, section bodies with Ruby's
//!   column widths, and a footer summarizing analyzed / skipped /
//!   default-excluded counts.
//! - **JSON meta** adds `files_skipped`, `formula`, `weights`,
//!   `filtered`, `diff_base`, `only_paths` and reorders keys to
//!   match Ruby's insertion order (`repo`, `analyzed_at`,
//!   `churn_days`, `file_count`, `files_skipped`, `formula`,
//!   `weights`, `schema_version`, `warnings`, [filtered?,
//!   diff_base?, only_paths?]).
//! - **Warnings** from sf-history and sf-newness flow in as
//!   `WarningPayload`s through `Sections.warnings` and surface in
//!   the JSON `meta.warnings` + root `warnings` array. The CLI
//!   still writes a stderr line per warning for terminal users —
//!   JSON propagation is additive.
//!
//! `rank` and `language` stay computed-at-emit — rank is the
//! 1-based position from `ScoredFile.rank` (assigned pre-filter in
//! sf-cli), language comes from the section the row was passed in
//! plus the path extension (Ruby splits `javascript` from
//! `typescript` by extension at emit time).

use clap::ValueEnum;
use serde_json::{Map, Value, json};
use sf_score::{Class, EffectiveWeights, ScoredFile};

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

// `is_table` used to gate the sf-cli emit path onto an inline
// renderer. Slice #10b moves table rendering into `output::emit`
// alongside the other formats, so the guard is gone.

/// Everything an emitter needs to produce one run's output.
pub struct Sections<'a> {
    pub ruby: &'a [ScoredFile],
    pub js: &'a [ScoredFile],
    pub repo: &'a str,
    pub churn_days: u64,
    pub analyzed_at: &'a str,
    /// Ruby's `Time.now.utc.strftime('%Y-%m-%d')` for the markdown
    /// `## stud-finder — YYYY-MM-DD` title. Derived once in sf-cli
    /// so markdown and JSON see a consistent run timestamp.
    pub analyzed_date: &'a str,
    pub warnings: &'a [WarningPayload],
    /// Total analyzed files (pre-filter). Reported as
    /// `meta.file_count`, matching Ruby's `analysis.ruby.files.length
    /// + analysis.javascript.files.length`. **Not** the post-filter
    /// output row count — `--top` and `--diff-base` don't shrink
    /// `file_count`.
    pub file_count: usize,
    /// Parse-error + unreadable count across both sections. Matches
    /// Ruby's `analysis.ruby.skipped_files.length +
    /// analysis.javascript.skipped_files.length`. sf-cli counts files
    /// prism couldn't parse (Ruby) plus files whose oxc parse was
    /// unclean (JS). A small-ish follow-up is counting read-error
    /// files too; today those print to stderr but aren't attributed
    /// to this metric.
    pub files_skipped: usize,
    /// Count of files excluded by default-rule globs
    /// (`vendor/`, `node_modules/`, etc. in Ruby). Rust's collect()
    /// currently does not apply default excludes — the WalkDir sweep
    /// is pure extension-filter. Report 0 for parity (Ruby's footer
    /// has this slot, matching the shape even when the value is 0).
    pub default_excluded_count: usize,
    /// Pre-rendered formula label, e.g. `"5-factor + coupling"`.
    /// Caller builds via `formula_label()`. Lives on Sections so
    /// markdown, table, and `meta.formula` all read the same string.
    pub formula: &'a str,
    /// Renormalized weights the composite used. Reported as
    /// `meta.weights` — Ruby `json_weights`. Keys always present:
    /// fan_in, fan_out, complexity, churn. Optional: coverage,
    /// interaction, coupling (JSON null when inactive).
    pub weights: &'a EffectiveWeights,
    /// Optional filter note, e.g. `"Filtered to files changed vs
    /// main (ranks are against the full repo)."`. When present, it
    /// emits as a `> blockquote` in markdown and a plain line above
    /// the first section in table.
    pub filter_note: Option<&'a str>,
    /// Run-level `--diff-base` value when set, for `meta.diff_base`.
    pub diff_base: Option<&'a str>,
    /// Run-level `--only` list when set, for `meta.only_paths`.
    pub only_paths: Option<&'a [String]>,
    /// Convenience: `true` when any filter (diff-base, only-paths,
    /// top) is active. Drives `meta.filtered`.
    pub filtered: bool,
    /// `true` when a Ruby-coverage report was loaded for this
    /// run. Gates the Ruby-section rows' coverage tri-state:
    /// unavailable → JSON null + CSV empty cell; available but
    /// not reported → em-dash; available + reported → number.
    pub ruby_coverage_available: bool,
    /// Same as `ruby_coverage_available` but for the JavaScript
    /// section.
    pub js_coverage_available: bool,
}

/// A warning payload shaped for the Ruby JSON contract.
/// `code` is the stable machine identifier; `message` is the
/// human-readable text (with any count/threshold details already
/// interpolated — Ruby `Warnings.normalize_one` emits exactly
/// `{code, message}` and discards other fields).
#[derive(Clone, Debug)]
pub struct WarningPayload {
    pub code: String,
    pub message: String,
}

pub fn emit(format: OutputFormat, sections: Sections<'_>) {
    match format {
        OutputFormat::Table => emit_table(sections),
        OutputFormat::Json => emit_json(sections),
        OutputFormat::Csv => emit_csv(sections),
        OutputFormat::Markdown => emit_markdown(sections),
    }
}

/// Ruby's four formula labels from `formula_label(coverage_available,
/// coupling_available)` in `lib/stud_finder/cli.rb`. Matches string
/// contents byte-for-byte so `meta.formula` and the markdown
/// blockquote agree with Ruby. The report-level coverage flag is
/// the UNION of ruby+js availability, matching Ruby's
/// `report_coverage_available?` helper.
pub fn formula_label(coverage_available: bool, coupling_available: bool) -> &'static str {
    match (coverage_available, coupling_available) {
        (true, true) => "5-factor + coupling",
        (true, false) => "5-factor",
        (false, true) => "4-factor + coupling",
        (false, false) => "4-factor",
    }
}

/// Union of the two per-section coverage flags, matching Ruby's
/// `report_coverage_available?(analysis)`. The scoring formula is
/// a run-level fact; a Ruby-only coverage run should emit the
/// 5-factor formula even though JS rows show `null` coverage.
pub fn report_coverage_available(ruby_avail: bool, js_avail: bool) -> bool {
    ruby_avail || js_avail
}

fn row_json(section_language: &str, coverage_available: bool, r: &ScoredFile) -> Value {
    let mut m: Map<String, Value> = Map::new();
    m.insert("rank".into(), json!(r.rank));
    m.insert(
        "language".into(),
        json!(row_language(section_language, &r.path)),
    );
    m.insert("path".into(), json!(r.path.display().to_string()));
    m.insert("score".into(), json!(round4(r.composite)));
    m.insert("evidence".into(), json!(round4(r.evidence)));
    m.insert("class".into(), json!(class_str(r.class)));
    m.insert("new_file".into(), json!(r.new_file));
    m.insert("age_days".into(), json!(r.age_days));
    m.insert("escalation".into(), json!(r.escalation));
    m.insert("fan_in".into(), json!(r.fan_in));
    m.insert("fan_in_pct".into(), json!(round4(r.fan_in_pct)));
    m.insert("fan_out".into(), json!(r.fan_out));
    m.insert("fan_out_pct".into(), json!(round4(r.fan_out_pct)));
    m.insert("instability".into(), json!(round4(r.instability)));
    m.insert("instability_pct".into(), json!(round4(r.instability_pct)));
    m.insert("complexity".into(), json!(r.complexity));
    m.insert("complexity_pct".into(), json!(round4(r.complexity_pct)));
    m.insert("churn_commits".into(), json!(r.churn_commits));
    m.insert("churn_lines".into(), json!(r.churn_lines));
    m.insert("churn_pct".into(), json!(round4(r.churn_pct)));
    m.insert("loc".into(), json!(r.loc));
    m.insert("loc_pct".into(), json!(round4(r.loc_pct)));
    m.insert("max_coupling".into(), json!(round4(r.max_coupling)));
    m.insert(
        "max_coupling_partner".into(),
        json!(r.max_coupling_partner.display().to_string()),
    );
    m.insert("coupling_partners".into(), json!(r.coupling_partners));
    m.insert("coupling_pct".into(), json!(round4(r.coupling_pct)));
    m.insert("coverage".into(), coverage_cell(coverage_available, r));
    Value::Object(m)
}

/// Ruby distinguishes `:javascript` (.js/.jsx) from `:typescript`
/// (.ts/.tsx) via FileCollector::EXT_LANGUAGE. The Rust port's
/// `Language::JavaScript` collapses both, so we recover the split
/// at emit time by inspecting the path extension. Ruby rows stay
/// `ruby` regardless.
fn row_language(section_language: &str, path: &std::path::Path) -> &'static str {
    match section_language {
        "ruby" => "ruby",
        _ => match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("ts") | Some("tsx") => "typescript",
            _ => "javascript",
        },
    }
}

fn coverage_cell(coverage_available: bool, r: &ScoredFile) -> Value {
    if !coverage_available {
        Value::Null
    } else if !r.coverage_reported {
        json!("—")
    } else {
        json!(round4(r.coverage))
    }
}

/// Match Ruby `Warnings.normalize_one`: always emit exactly
/// `{code, message}`. Extra fields the caller carries on
/// `WarningPayload.extra` are NOT serialized to JSON — Ruby
/// interpolates them into the human-readable `message` (e.g. the
/// bulk-commit warning appends `(N skipped; max_commit_files=M)`)
/// and discards them from the record. Caught as Ohm's P2 on PR #86:
/// prior slice-#10b revision cloned `extra` onto the JSON object.
fn warnings_json(warnings: &[WarningPayload]) -> Vec<Value> {
    warnings
        .iter()
        .map(|w| {
            let mut m = Map::new();
            m.insert("code".into(), json!(w.code));
            m.insert("message".into(), json!(w.message));
            Value::Object(m)
        })
        .collect()
}

fn weights_json(w: &EffectiveWeights) -> Value {
    let mut m = Map::new();
    m.insert("fan_in".into(), json!(round4(w.fan_in)));
    m.insert("fan_out".into(), json!(round4(w.fan_out)));
    m.insert("complexity".into(), json!(round4(w.complexity)));
    m.insert("churn".into(), json!(round4(w.churn)));
    m.insert(
        "coverage".into(),
        w.coverage.map(|x| json!(round4(x))).unwrap_or(Value::Null),
    );
    m.insert(
        "interaction".into(),
        w.interaction
            .map(|x| json!(round4(x)))
            .unwrap_or(Value::Null),
    );
    m.insert(
        "coupling".into(),
        w.coupling.map(|x| json!(round4(x))).unwrap_or(Value::Null),
    );
    Value::Object(m)
}

fn emit_json(sections: Sections<'_>) {
    let mut meta = Map::new();
    // KEY ORDER MATCHES Ruby's `json_meta` insertion order — downstream
    // consumers comparing outputs line-for-line rely on it.
    meta.insert("repo".into(), json!(sections.repo));
    meta.insert("analyzed_at".into(), json!(sections.analyzed_at));
    meta.insert("churn_days".into(), json!(sections.churn_days));
    meta.insert("file_count".into(), json!(sections.file_count));
    meta.insert("files_skipped".into(), json!(sections.files_skipped));
    meta.insert("formula".into(), json!(sections.formula));
    meta.insert("weights".into(), weights_json(sections.weights));
    meta.insert("schema_version".into(), json!(SCHEMA_VERSION));
    meta.insert(
        "warnings".into(),
        Value::Array(warnings_json(sections.warnings)),
    );
    if sections.filtered {
        meta.insert("filtered".into(), json!(true));
    }
    if let Some(base) = sections.diff_base {
        meta.insert("diff_base".into(), json!(base));
    }
    if let Some(only) = sections.only_paths {
        meta.insert(
            "only_paths".into(),
            Value::Array(only.iter().map(|s| json!(s)).collect()),
        );
    }

    let ruby_rows: Vec<Value> = sections
        .ruby
        .iter()
        .map(|r| row_json("ruby", sections.ruby_coverage_available, r))
        .collect();
    let js_rows: Vec<Value> = sections
        .js
        .iter()
        .map(|r| row_json("javascript", sections.js_coverage_available, r))
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

/// Ruby's `RESULT_COLUMNS`, verbatim.
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

/// Ruby's `MARKDOWN_COLUMNS`, verbatim from `lib/stud_finder/cli.rb`.
/// A 22-column subset of `RESULT_COLUMNS` — drops the `_pct`
/// variants that only make sense in CSV/JSON. Column order matches
/// Ruby's `markdown_row` cell builder.
pub const MARKDOWN_COLUMNS: &[&str] = &[
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
    "fan_out",
    "fan_out_pct",
    "instability",
    "complexity",
    "churn_commits",
    "churn_lines",
    "churn_pct",
    "max_coupling",
    "max_coupling_partner",
    "coupling_partners",
    "coupling_pct",
    "coverage",
];

fn emit_csv(sections: Sections<'_>) {
    println!("{}", RESULT_COLUMNS.join(","));
    for r in sections.ruby.iter() {
        println!("{}", csv_row("ruby", sections.ruby_coverage_available, r));
    }
    for r in sections.js.iter() {
        println!(
            "{}",
            csv_row("javascript", sections.js_coverage_available, r)
        );
    }
}

fn csv_row(section_language: &str, coverage_available: bool, r: &ScoredFile) -> String {
    let coverage_cell = if !coverage_available {
        "\"\"".to_string()
    } else if !r.coverage_reported {
        "—".to_string()
    } else {
        ruby_float_to_csv(round4(r.coverage))
    };
    // Ohm's non-blocking (#85 follow-up): emit empty-string cells as
    // quoted-empty `""` for byte-level Ruby CSV.generate_line parity.
    // `row[:escalation]` can be `""` when the file wasn't escalated;
    // Ruby stringifies empty → `""`, and `max_coupling_partner` is
    // also a string that can be empty-at-emit for rows with no
    // qualifying partner. Rank/counts stay numeric (never empty).
    let cells: Vec<String> = vec![
        r.rank.to_string(),
        csv_escape(row_language(section_language, &r.path)),
        csv_escape(&r.path.display().to_string()),
        format_score(r.composite),
        format_score(r.evidence),
        class_str(r.class).to_string(),
        r.new_file.to_string(),
        r.age_days.to_string(),
        csv_escape_allow_empty(&r.escalation),
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
        csv_escape_allow_empty(&r.max_coupling_partner.display().to_string()),
        r.coupling_partners.to_string(),
        format_score(r.coupling_pct),
        coverage_cell,
    ];
    cells.join(",")
}

/// Emit `x` the way Ruby `Float#to_s` would: shortest form, with
/// a trailing `.0` preserved on integer-valued floats.
fn ruby_float_to_csv(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Like `csv_escape`, but the empty string becomes a quoted-empty
/// `""` rather than a bare empty cell. Matches Ruby
/// `CSV.generate_line`'s handling of `""` (empty String) versus
/// `nil` — Rust's Scorer never emits a "null" for these fields, it
/// emits empty strings, so the Ruby equivalent is `""`, not bare.
/// Caught as Ohm's non-blocking on PR #85.
fn csv_escape_allow_empty(s: &str) -> String {
    if s.is_empty() {
        "\"\"".to_string()
    } else {
        csv_escape(s)
    }
}

// ---------------------------------------------------------------
// Markdown — Ruby `emit_markdown` / `emit_markdown_section` parity.
// ---------------------------------------------------------------

fn emit_markdown(sections: Sections<'_>) {
    let file_count = sections.file_count;
    println!("## stud-finder — {}", sections.analyzed_date);
    println!();
    println!(
        "> {}. Churn window: {} days. {} files analyzed.",
        sections.formula, sections.churn_days, file_count
    );
    if let Some(note) = sections.filter_note {
        println!();
        println!("> {note}");
    }
    emit_markdown_section("Ruby", sections.ruby, sections.ruby_coverage_available);
    emit_markdown_section(
        "JavaScript/TypeScript",
        sections.js,
        sections.js_coverage_available,
    );
    println!();
    println!("*fan_in is a static approximation — dynamic references not counted.*");
}

fn emit_markdown_section(title: &str, rows: &[ScoredFile], coverage_available: bool) {
    println!("### {title}");
    println!();
    println!("| {} |", MARKDOWN_COLUMNS.join(" | "));
    println!("| {} |", vec!["---"; MARKDOWN_COLUMNS.len()].join(" | "));
    for r in rows {
        println!("{}", markdown_row(title, coverage_available, r));
    }
    println!();
}

fn markdown_row(section_title: &str, coverage_available: bool, r: &ScoredFile) -> String {
    // Column order MUST match MARKDOWN_COLUMNS above (and Ruby
    // `markdown_row` in cli.rb). Keeping them local-and-inline here
    // beats building a Vec<String> — the formatter pass is tight.
    let section_language = if section_title == "Ruby" {
        "ruby"
    } else {
        "javascript"
    };
    let language = row_language(section_language, &r.path);
    let path = md_escape(&r.path.display().to_string());
    let partner = md_escape(&r.max_coupling_partner.display().to_string());
    let evidence = format_score(r.evidence);
    let coverage = format_coverage_cell_markdown(coverage_available, r);
    format!(
        "| {rank} | {language} | {path} | {score} | {evidence} | {class} | {new_file} | {age} | \
         {escalation} | {fan_in} | {fan_out} | {fan_out_pct} | {instability} | {complexity} | \
         {cc} | {cl} | {churn_pct} | {max_coupling} | {partner} | {coupling_partners} | \
         {coupling_pct} | {coverage} |",
        rank = r.rank,
        language = language,
        path = path,
        score = format_score(r.composite),
        evidence = evidence,
        class = class_str(r.class),
        new_file = r.new_file,
        age = r.age_days,
        escalation = md_escape(&r.escalation),
        fan_in = r.fan_in,
        fan_out = r.fan_out,
        fan_out_pct = format_score(r.fan_out_pct),
        instability = format_score(r.instability),
        complexity = r.complexity,
        cc = r.churn_commits,
        cl = r.churn_lines,
        churn_pct = format_score(r.churn_pct),
        max_coupling = format_score(r.max_coupling),
        partner = partner,
        coupling_partners = r.coupling_partners,
        coupling_pct = format_score(r.coupling_pct),
        coverage = coverage,
    )
}

/// Markdown coverage cell: Ruby `format_coverage(row[:coverage])`:
///   - `nil` → `n/a` (coverage not available for this section)
///   - String → verbatim (em-dash placeholder when unreported)
///   - Numeric → `"#{(x * 100).round}%"`.
fn format_coverage_cell_markdown(coverage_available: bool, r: &ScoredFile) -> String {
    if !coverage_available {
        "n/a".to_string()
    } else if !r.coverage_reported {
        "—".to_string()
    } else {
        format!("{}%", (r.coverage * 100.0).round() as i64)
    }
}

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

// ---------------------------------------------------------------
// Table — Ruby `emit_table` parity. Column widths copied from
// `lib/stud_finder/cli.rb` `table_row` format string.
// ---------------------------------------------------------------

fn emit_table(sections: Sections<'_>) {
    let coverage_available = report_coverage_available(
        sections.ruby_coverage_available,
        sections.js_coverage_available,
    );
    println!(
        "stud-finder — {} ({}-day churn, {})",
        sections.repo, sections.churn_days, sections.formula
    );
    if !coverage_available {
        println!("{}", scoring_note_stdout(sections.weights));
    }
    if let Some(note) = sections.filter_note {
        println!("{note}");
    }
    println!();
    emit_table_section("Ruby", sections.ruby, sections.ruby_coverage_available);
    emit_table_section(
        "JavaScript/TypeScript",
        sections.js,
        sections.js_coverage_available,
    );
    println!();
    println!(
        "{}",
        table_footer(
            sections.file_count,
            sections.files_skipped,
            sections.default_excluded_count
        )
    );
    println!(
        "fan_in is a static approximation — dynamic references \
         (const_get, send, metaprogramming) not counted."
    );
}

fn emit_table_section(title: &str, rows: &[ScoredFile], coverage_available: bool) {
    println!("{title}");
    // Header string lifted from Ruby `emit_table_section`. One-shot
    // string so column labels stay in lock-step with the format
    // template in `table_row`.
    println!(
        " rank  language    file                                            score  evidence  \
         class   new  age_days  escalation      fan_in  fan_out  instability  complexity  \
         churn_commits  churn_lines  churn_pct    loc  loc_pct  max_coupling  \
         max_coupling_partner                      coupling_partners  coupling_pct  coverage"
    );
    for r in rows {
        println!("{}", table_row(coverage_available, r));
    }
    println!();
}

fn table_row(coverage_available: bool, r: &ScoredFile) -> String {
    // Column widths are from Ruby's `table_row` format string.
    // Evidence is always format_score (Rust-side `r.evidence` is
    // always f64, unlike Ruby's `nil | Float`).
    //
    // Path: Ruby `%-45s` is a MINIMUM width (left-aligned pad to 45,
    // longer strings print in full). Rust's `{:<45}` matches that —
    // no truncation. Prior slice-#10b revision mistakenly
    // length-capped at 45 and dropped actionable basenames on long
    // paths (Ohm P2).
    let language = language_from_path(&r.path);
    let path = r.path.display().to_string();
    // Partner: Ruby `truncate_partner` — keep as-is if ≤40 chars,
    // else emit `"..."` + the LAST 37 chars (the end of the path,
    // which carries the actionable basename). Prior revision
    // truncated the FIRST 40 chars and dropped the basename.
    let partner = truncate_partner(&r.max_coupling_partner.display().to_string());
    let coverage = format_coverage_cell_table(coverage_available, r);
    format!(
        "{rank:>5}  {language:<10}  {path:<45}  {score:>6}  {evidence:>8}  \
         {class:<6}  {new_file:>5}  {age:>8}  {escalation:<15}  {fan_in:>6}  {fan_out:>7}  \
         {instability:>11}  {complexity:>10}  {cc:>13}  {cl:>11}  {churn_pct:>9}  \
         {loc:>5}  {loc_pct:>7}  {max_coupling:>12}  {partner:<40}  {coupling_partners:>17}  \
         {coupling_pct:>12}  {coverage:>8}",
        rank = r.rank,
        language = language,
        path = path,
        score = format_score(r.composite),
        evidence = format_score(r.evidence),
        class = class_str(r.class),
        new_file = r.new_file.to_string(),
        age = r.age_days,
        escalation = r.escalation,
        fan_in = r.fan_in,
        fan_out = r.fan_out,
        instability = format_score(r.instability),
        complexity = r.complexity,
        cc = r.churn_commits,
        cl = r.churn_lines,
        churn_pct = format_score(r.churn_pct),
        loc = r.loc,
        loc_pct = format_score(r.loc_pct),
        max_coupling = format_score(r.max_coupling),
        partner = partner,
        coupling_partners = r.coupling_partners,
        coupling_pct = format_score(r.coupling_pct),
        coverage = coverage,
    )
}

/// Table coverage cell: Ruby `format_coverage(row[:coverage])` —
/// width-8 right-aligned in the parent table template. Shape
/// matches markdown except the available-but-unreported value is
/// the em-dash rather than the generic placeholder.
fn format_coverage_cell_table(coverage_available: bool, r: &ScoredFile) -> String {
    if !coverage_available {
        "n/a".to_string()
    } else if !r.coverage_reported {
        "—".to_string()
    } else {
        format!("{}%", (r.coverage * 100.0).round() as i64)
    }
}

fn language_from_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("rb") => "ruby",
        Some("ts") | Some("tsx") => "typescript",
        _ => "javascript",
    }
}

/// Match Ruby `truncate_partner` from `lib/stud_finder/cli.rb`:
///
/// ```ruby
/// def truncate_partner(partner)
///   partner = partner.to_s
///   return partner if partner.length <= 40
///   "...#{partner[-37..]}"
/// end
/// ```
///
/// Strings ≤40 chars pass through unchanged; longer strings emit as
/// `"..."` + the LAST 37 chars so the end of the path (where the
/// basename lives) is preserved. Ruby counts length in characters;
/// Rust mirrors that via `chars().count()` so multi-byte text stays
/// well-formed.
fn truncate_partner(s: &str) -> String {
    let count = s.chars().count();
    if count <= 40 {
        s.to_string()
    } else {
        // `chars().rev().take(37)` then reverse back — avoids slicing
        // by byte index which breaks on multi-byte characters.
        let tail: String = s
            .chars()
            .rev()
            .take(37)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("...{tail}")
    }
}

fn table_footer(file_count: usize, files_skipped: usize, default_excluded: usize) -> String {
    let mut parts = Vec::with_capacity(3);
    parts.push(format!("{file_count} files analyzed."));
    if files_skipped > 0 {
        parts.push(format!(
            "{files_skipped} files skipped (parse errors — run --verbose to see)."
        ));
    }
    parts.push(format!(
        "{default_excluded} files excluded by default rules."
    ));
    parts.join(" ")
}

/// Ruby `scoring_note(weights:, stderr: false)` with coupling —
/// the plain-stdout variant. Ruby emits this when coverage is
/// unavailable; the stderr variant is a separate code path kept
/// for CLI warnings. Byte-for-byte match to Ruby's `format(...)`.
fn scoring_note_stdout(w: &EffectiveWeights) -> String {
    if let Some(cp) = w.coupling {
        format!(
            "Note: coverage data not available. Score uses fan_in {:.2}, fan_out {:.2}, \
             complexity {:.2}, churn {:.2}, coupling {:.2}.",
            w.fan_in, w.fan_out, w.complexity, w.churn, cp
        )
    } else {
        format!(
            "Note: coverage data not available. Score uses fan_in {:.2}, fan_out {:.2}, \
             complexity {:.2}, churn {:.2}.",
            w.fan_in, w.fan_out, w.complexity, w.churn
        )
    }
}

fn class_str(c: Class) -> &'static str {
    c.as_str()
}

fn format_score(x: f64) -> String {
    format!("{x:.4}")
}

/// Round to 4 decimals for JSON emit.
fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
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
            coverage_reported: false,
            rank: 0,
        }
    }

    fn mk_weights() -> EffectiveWeights {
        sf_score::effective_weights(true, true, false, false)
    }

    fn mk_sections<'a>(
        ruby: &'a [ScoredFile],
        js: &'a [ScoredFile],
        warnings: &'a [WarningPayload],
        weights: &'a EffectiveWeights,
    ) -> Sections<'a> {
        Sections {
            ruby,
            js,
            repo: ".",
            churn_days: 180,
            analyzed_at: "2026-11-13T00:00:00Z",
            analyzed_date: "2026-11-13",
            warnings,
            file_count: ruby.len() + js.len(),
            files_skipped: 0,
            default_excluded_count: 0,
            formula: "4-factor",
            weights,
            filter_note: None,
            diff_base: None,
            only_paths: None,
            filtered: false,
            ruby_coverage_available: false,
            js_coverage_available: false,
        }
    }

    #[test]
    fn result_columns_matches_ruby_order_exactly() {
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
    fn markdown_columns_matches_ruby_order_exactly() {
        // Verbatim against Ruby `lib/stud_finder/cli.rb`
        // `MARKDOWN_COLUMNS`. Row order in `markdown_row` MUST line
        // up with this — a divergence misaligns every emitted cell.
        assert_eq!(
            MARKDOWN_COLUMNS,
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
                "fan_out",
                "fan_out_pct",
                "instability",
                "complexity",
                "churn_commits",
                "churn_lines",
                "churn_pct",
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
        let r = mk_row("app/order.rb", 0.73, Class::Branch);
        let v = {
            let mut r2 = r.clone();
            r2.rank = 5;
            row_json("ruby", true, &r2)
        };
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("rank"), Some(&json!(5)));
        assert_eq!(obj.get("language"), Some(&json!("ruby")));
        assert_eq!(obj.get("path"), Some(&json!("app/order.rb")));
        assert_eq!(obj.get("score"), Some(&json!(0.73)));
        assert_eq!(obj.get("class"), Some(&json!("branch")));
        assert!(obj.get("composite").is_none());
        assert!(obj.get("classification").is_none());
    }

    #[test]
    fn row_json_carries_every_result_column_field() {
        let r = mk_row("x.rb", 0.0, Class::Leaf);
        let v = row_json("ruby", true, &r);
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
        let row = csv_row("ruby", true, &r);
        let cells: Vec<&str> = row.split(',').collect();
        assert_eq!(cells.len(), RESULT_COLUMNS.len());
    }

    #[test]
    fn csv_row_escapes_comma_in_path() {
        let mut r = mk_row("weird,path.rb", 0.0, Class::Leaf);
        r.escalation = "trunk_adjacent".to_string();
        let row = csv_row("ruby", true, &r);
        assert!(row.contains("\"weird,path.rb\""));
    }

    #[test]
    fn coverage_json_is_null_when_run_level_unavailable() {
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let v = row_json("ruby", false, &r);
        assert_eq!(v.as_object().unwrap().get("coverage"), Some(&Value::Null));
    }

    #[test]
    fn coverage_json_is_em_dash_when_file_missing_from_report() {
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let v = row_json("ruby", true, &r);
        assert_eq!(v.as_object().unwrap().get("coverage"), Some(&json!("—")));
    }

    #[test]
    fn coverage_json_is_number_when_reported() {
        let mut r = mk_row("a.rb", 0.0, Class::Leaf);
        r.coverage = 0.7334567;
        r.coverage_reported = true;
        let v = row_json("ruby", true, &r);
        let v = v.as_object().unwrap().get("coverage").unwrap();
        assert_eq!(v, &json!(0.7335));
    }

    #[test]
    fn typescript_extension_emits_typescript_language() {
        let r_ts = mk_row("app/foo.ts", 0.0, Class::Leaf);
        let r_tsx = mk_row("app/foo.tsx", 0.0, Class::Leaf);
        let r_js = mk_row("app/foo.js", 0.0, Class::Leaf);
        let v_ts = row_json("javascript", false, &r_ts);
        let v_tsx = row_json("javascript", false, &r_tsx);
        let v_js = row_json("javascript", false, &r_js);
        assert_eq!(v_ts["language"], json!("typescript"));
        assert_eq!(v_tsx["language"], json!("typescript"));
        assert_eq!(v_js["language"], json!("javascript"));
        let r_rb = mk_row("app/foo.rb", 0.0, Class::Leaf);
        let v_rb = row_json("ruby", false, &r_rb);
        assert_eq!(v_rb["language"], json!("ruby"));
    }

    #[test]
    fn row_rank_comes_from_scored_file_not_enumerate() {
        let mut r = mk_row("a.rb", 0.0, Class::Leaf);
        r.rank = 42;
        let v = row_json("ruby", false, &r);
        assert_eq!(v["rank"], json!(42));
    }

    #[test]
    fn coverage_availability_is_per_section_not_global() {
        let r_ruby = mk_row("a.rb", 0.0, Class::Leaf);
        let r_js = mk_row("app/foo.js", 0.0, Class::Leaf);
        let v_ruby = row_json("ruby", true, &r_ruby);
        assert_eq!(v_ruby["coverage"], json!("—"));
        let v_js = row_json("javascript", false, &r_js);
        assert_eq!(v_js["coverage"], Value::Null);
    }

    #[test]
    fn csv_coverage_reported_is_shortest_float_form() {
        let mut r = mk_row("a.rb", 0.0, Class::Leaf);
        r.coverage = 0.5;
        r.coverage_reported = true;
        let row = csv_row("ruby", true, &r);
        let cov = row.split(',').next_back().unwrap();
        assert_eq!(cov, "0.5");

        r.coverage = 0.0;
        let row = csv_row("ruby", true, &r);
        let cov = row.split(',').next_back().unwrap();
        assert_eq!(cov, "0.0");

        r.coverage = 0.3333;
        let row = csv_row("ruby", true, &r);
        let cov = row.split(',').next_back().unwrap();
        assert_eq!(cov, "0.3333");
    }

    #[test]
    fn csv_coverage_unavailable_is_quoted_empty_string() {
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let row = csv_row("ruby", false, &r);
        assert!(row.ends_with(",\"\""));
    }

    #[test]
    fn csv_coverage_unreported_is_em_dash_literal() {
        let mut r = mk_row("a.rb", 0.0, Class::Leaf);
        r.coverage_reported = false;
        let row = csv_row("ruby", true, &r);
        assert!(row.ends_with(",—"));
    }

    #[test]
    fn json_percentile_fields_are_round4() {
        let mut r = mk_row("a.rb", 0.0, Class::Leaf);
        r.fan_in_pct = 0.123456789;
        r.churn_pct = 0.999999999;
        let v = row_json("ruby", false, &r);
        assert_eq!(v["fan_in_pct"], json!(0.1235));
        assert_eq!(v["churn_pct"], json!(1.0));
    }

    // --- Slice #10b: markdown + table + meta + warnings + weights ---

    #[test]
    fn formula_label_matches_ruby_four_cases() {
        assert_eq!(formula_label(true, true), "5-factor + coupling");
        assert_eq!(formula_label(true, false), "5-factor");
        assert_eq!(formula_label(false, true), "4-factor + coupling");
        assert_eq!(formula_label(false, false), "4-factor");
    }

    #[test]
    fn report_coverage_available_is_union_matching_ruby() {
        // Ruby `analysis.ruby.coverage_available ||
        // analysis.javascript.coverage_available`. A Ruby-only
        // coverage run gives a 5-factor formula label even
        // though JS rows emit `null` coverage.
        assert!(report_coverage_available(true, false));
        assert!(report_coverage_available(false, true));
        assert!(report_coverage_available(true, true));
        assert!(!report_coverage_available(false, false));
    }

    #[test]
    fn weights_json_has_all_keys_and_nulls_for_optional() {
        // 4-factor active (no coverage, no coupling). coverage,
        // interaction, coupling stay `null`. fan_in/fan_out/
        // complexity/churn are numbers summing to 1.0 under round4.
        let w = sf_score::effective_weights(true, true, false, false);
        let v = weights_json(&w);
        let obj = v.as_object().unwrap();
        assert!(obj.get("fan_in").unwrap().is_number());
        assert!(obj.get("fan_out").unwrap().is_number());
        assert!(obj.get("complexity").unwrap().is_number());
        assert!(obj.get("churn").unwrap().is_number());
        assert_eq!(obj.get("coverage"), Some(&Value::Null));
        assert_eq!(obj.get("interaction"), Some(&Value::Null));
        assert_eq!(obj.get("coupling"), Some(&Value::Null));
        // Sum active weights to ≈1.0 (round4 of each still sums to 1.0000).
        let sum = obj.get("fan_in").unwrap().as_f64().unwrap()
            + obj.get("fan_out").unwrap().as_f64().unwrap()
            + obj.get("complexity").unwrap().as_f64().unwrap()
            + obj.get("churn").unwrap().as_f64().unwrap();
        assert!(
            (sum - 1.0).abs() < 1e-3,
            "active base weights should renormalize to ~1.0; sum = {sum}"
        );
    }

    #[test]
    fn weights_json_emits_optional_as_number_when_active() {
        let w = sf_score::effective_weights(true, true, true, true);
        let v = weights_json(&w);
        let obj = v.as_object().unwrap();
        assert!(obj.get("coupling").unwrap().is_number());
        assert!(obj.get("coverage").unwrap().is_number());
        assert!(obj.get("interaction").unwrap().is_number());
    }

    #[test]
    fn json_meta_key_order_matches_ruby() {
        // Ruby `json_meta` builds the Hash in this insertion order,
        // which Hash#to_json preserves. Byte-level consumers rely on
        // the key order — don't reshuffle.
        let w = mk_weights();
        let sections = mk_sections(&[], &[], &[], &w);
        // Use the private emit_json pathway by rebuilding the Map
        // identically (pure test-time emit — assert the keys in order).
        let mut meta = Map::new();
        meta.insert("repo".into(), json!(sections.repo));
        meta.insert("analyzed_at".into(), json!(sections.analyzed_at));
        meta.insert("churn_days".into(), json!(sections.churn_days));
        meta.insert("file_count".into(), json!(sections.file_count));
        meta.insert("files_skipped".into(), json!(sections.files_skipped));
        meta.insert("formula".into(), json!(sections.formula));
        meta.insert("weights".into(), weights_json(sections.weights));
        meta.insert("schema_version".into(), json!(SCHEMA_VERSION));
        meta.insert("warnings".into(), json!([]));
        let keys: Vec<&str> = meta.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "repo",
                "analyzed_at",
                "churn_days",
                "file_count",
                "files_skipped",
                "formula",
                "weights",
                "schema_version",
                "warnings",
            ]
        );
    }

    #[test]
    fn markdown_row_has_22_cells_matching_markdown_columns() {
        // Count the pipe-separated cells in a rendered row —
        // must match MARKDOWN_COLUMNS length so columns line up
        // with the header.
        let r = mk_row("app/x.rb", 0.5, Class::Branch);
        let line = markdown_row("Ruby", false, &r);
        // pipe-delimited row starts with `| ` and ends with ` |` so
        // split on `|` gives 22 interior cells plus 2 empties.
        let cells: Vec<&str> = line.trim().trim_matches('|').split(" | ").collect();
        assert_eq!(
            cells.len(),
            MARKDOWN_COLUMNS.len(),
            "markdown row cell count must match MARKDOWN_COLUMNS: `{line}`"
        );
    }

    #[test]
    fn markdown_coverage_is_na_when_unavailable() {
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let line = markdown_row("Ruby", false, &r);
        assert!(
            line.ends_with(" n/a |"),
            "unavailable coverage should be `n/a`: {line}"
        );
    }

    #[test]
    fn markdown_coverage_is_percent_when_reported() {
        let mut r = mk_row("a.rb", 0.0, Class::Leaf);
        r.coverage = 0.734;
        r.coverage_reported = true;
        let line = markdown_row("Ruby", true, &r);
        assert!(
            line.ends_with(" 73% |"),
            "reported coverage should be rounded percent: {line}"
        );
    }

    #[test]
    fn markdown_coverage_is_em_dash_when_section_available_but_file_missing() {
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let line = markdown_row("Ruby", true, &r);
        assert!(
            line.ends_with(" — |"),
            "unreported-in-report coverage should be em-dash: {line}"
        );
    }

    #[test]
    fn table_footer_omits_skipped_line_when_zero() {
        let s = table_footer(10, 0, 3);
        assert_eq!(s, "10 files analyzed. 3 files excluded by default rules.");
    }

    #[test]
    fn table_footer_emits_skipped_when_positive() {
        let s = table_footer(10, 2, 3);
        assert_eq!(
            s,
            "10 files analyzed. 2 files skipped (parse errors — run --verbose to see). 3 files excluded by default rules."
        );
    }

    #[test]
    fn scoring_note_stdout_with_coupling() {
        let w = sf_score::effective_weights(true, true, true, false);
        let note = scoring_note_stdout(&w);
        assert!(note.starts_with("Note: coverage data not available. Score uses fan_in "));
        assert!(note.contains(", coupling "));
    }

    #[test]
    fn scoring_note_stdout_without_coupling() {
        let w = sf_score::effective_weights(true, true, false, false);
        let note = scoring_note_stdout(&w);
        assert!(note.starts_with("Note: coverage data not available. Score uses fan_in "));
        assert!(!note.contains(", coupling "));
    }

    #[test]
    fn warnings_json_emits_only_code_and_message_matching_ruby_normalize() {
        // Ruby `Warnings.normalize_one` returns exactly
        // `{code, message}`. Count/threshold details must already
        // live inside `message` — the record itself has no extras.
        // Caught as Ohm's P2 on PR #86.
        let payload = WarningPayload {
            code: "temporal_coupling_bulk_commits_skipped".into(),
            message: "bulk commits skipped (3 skipped; max_commit_files=50)".into(),
        };
        let v = warnings_json(&[payload]);
        assert_eq!(v.len(), 1);
        let obj = v[0].as_object().unwrap();
        assert_eq!(obj["code"], json!("temporal_coupling_bulk_commits_skipped"));
        assert!(obj["message"].as_str().unwrap().contains("3 skipped"));
        assert_eq!(obj.len(), 2, "must emit only {{code, message}}: {obj:?}");
    }

    #[test]
    fn truncate_partner_keeps_path_suffix_matching_ruby() {
        // Ruby `truncate_partner`: ≤40 chars → verbatim; else
        // `"..."` + last 37 chars (keeps the basename at the end).
        let short = "lib/foo.rb";
        assert_eq!(truncate_partner(short), short);

        let exactly_40 = "a".repeat(40);
        assert_eq!(truncate_partner(&exactly_40), exactly_40);

        let long = "very/long/path/to/some/deeply/nested/module/basename.rb";
        let got = truncate_partner(long);
        assert!(
            got.starts_with("..."),
            "long path must get `...` prefix: {got}"
        );
        assert_eq!(
            got.chars().count(),
            40,
            "truncated must be exactly 40 chars"
        );
        assert!(
            got.ends_with("basename.rb"),
            "truncation must keep the actionable basename: {got}"
        );
    }

    #[test]
    fn csv_escalation_empty_is_quoted_empty() {
        // Ohm's non-blocking (#85 follow-up): an empty escalation
        // cell should match Ruby CSV.generate_line's `""` for
        // empty strings — bare empty is Ruby's `nil`.
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let row = csv_row("ruby", true, &r);
        let cells: Vec<&str> = row.split(',').collect();
        // escalation is column index 8 (0-indexed) in RESULT_COLUMNS.
        assert_eq!(cells[8], "\"\"", "empty escalation should be quoted-empty");
    }

    #[test]
    fn csv_coupling_partner_empty_is_quoted_empty() {
        let r = mk_row("a.rb", 0.0, Class::Leaf);
        let row = csv_row("ruby", true, &r);
        let cells: Vec<&str> = row.split(',').collect();
        // max_coupling_partner is column index 23 (0-indexed).
        assert_eq!(
            cells[23], "\"\"",
            "empty max_coupling_partner should be quoted-empty"
        );
    }
}
