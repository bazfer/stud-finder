//! CI-safety gate over a stud-finder JSON report. Matches Ruby
//! `StudFinder::Gate` in `lib/stud_finder/gate.rb` verbatim —
//! three checks, two thresholds, markdown emitter.
//!
//! # Checks
//!
//! - **`trunk_touched`**: any row whose `class` is `"trunk"`. The
//!   change set includes a load-bearing file; reviewer must look.
//! - **`low_evidence_high_score`**: any row with `score >=
//!   HIGH_SCORE_THRESHOLD` (0.75) AND `evidence <=
//!   LOW_EVIDENCE_THRESHOLD` (0.50, or null). High score, shaky
//!   metadata — the ranking is suspect.
//! - **`newness_trunk_adjacent`**: any row with `new_file: true`
//!   AND `escalation == "trunk_adjacent"`. A new file depending on
//!   a Trunk warrants extra eyes.
//!
//! # Input shape
//!
//! Reads the `ruby` + `javascript` arrays from the JSON produced by
//! `stud-finder --output json`. Each row is read loosely (field-
//! optional where Ruby uses `row['field']` defaults). Malformed JSON
//! surfaces as `Error::InvalidJson`.

use serde_json::Value;

/// Score threshold for `low_evidence_high_score` — matches Ruby
/// `HIGH_SCORE_THRESHOLD = 0.75`. The check fires when
/// `score >= HIGH_SCORE_THRESHOLD`.
pub const HIGH_SCORE_THRESHOLD: f64 = 0.75;

/// Evidence threshold for `low_evidence_high_score` — matches Ruby
/// `LOW_EVIDENCE_THRESHOLD = 0.50`. The check fires when evidence
/// is strictly less than this value (Ruby `value.to_f <
/// LOW_EVIDENCE_THRESHOLD`) OR is missing/null. Ohm noted the
/// prior docstring said "<=" — the code was correct, the doc was
/// wrong.
pub const LOW_EVIDENCE_THRESHOLD: f64 = 0.50;

/// Ordered list of check names. Matches Ruby `CHECKS` verbatim so
/// the markdown emitter prints checks in the same order.
pub const CHECKS: &[&str] = &[
    "trunk_touched",
    "low_evidence_high_score",
    "newness_trunk_adjacent",
];

/// One flagged row attributed to a specific check.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub path: String,
    pub language: Option<String>,
    pub score: Option<f64>,
    pub evidence: Option<f64>,
    pub classification: Option<String>,
    pub reason: String,
}

/// Collected findings across the three checks. Ordered field access
/// goes through `for_check(name)` which returns them in Ruby's
/// `CHECKS` order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GateResult {
    pub trunk_touched: Vec<Finding>,
    pub low_evidence_high_score: Vec<Finding>,
    pub newness_trunk_adjacent: Vec<Finding>,
}

impl GateResult {
    pub fn finding_count(&self) -> usize {
        self.trunk_touched.len()
            + self.low_evidence_high_score.len()
            + self.newness_trunk_adjacent.len()
    }

    pub fn has_findings(&self) -> bool {
        self.finding_count() > 0
    }

    pub fn for_check(&self, check: &str) -> &[Finding] {
        match check {
            "trunk_touched" => &self.trunk_touched,
            "low_evidence_high_score" => &self.low_evidence_high_score,
            "newness_trunk_adjacent" => &self.newness_trunk_adjacent,
            _ => &[],
        }
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidJson(String),
    InvalidReport(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidJson(msg) => write!(f, "Error: invalid JSON input: {msg}"),
            Error::InvalidReport(msg) => {
                write!(f, "Error: malformed stud-finder report: {msg}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Run the three checks over a stud-finder JSON report. `raw` is
/// the full payload (as emitted by `stud-finder --output json`) —
/// we read the `ruby` + `javascript` arrays and merge them. Rows
/// missing a field are treated as Ruby would: `row[:field]` returns
/// nil, which propagates to `None` here.
pub fn run(raw: &str) -> Result<GateResult, Error> {
    let payload: Value =
        serde_json::from_str(raw).map_err(|e| Error::InvalidJson(e.to_string()))?;
    // Shape validation (Ohm's P1 on PR #91). Prior revision used
    // `payload.get(key).and_then(as_array)` which silently returned
    // None for scalar roots or non-array sections — a malformed
    // report then became a passing gate under --enforce. Require
    // the shape explicitly; errors surface as `Error::InvalidReport`.
    let root_obj = payload.as_object().ok_or_else(|| {
        Error::InvalidReport("root of stud-finder report must be a JSON object".to_string())
    })?;
    let mut rows: Vec<&Value> = Vec::new();
    for key in ["ruby", "javascript"] {
        let Some(section) = root_obj.get(key) else {
            continue;
        };
        if section.is_null() {
            // Ruby `Array(nil) == []` — treat null as missing.
            continue;
        }
        let arr = section.as_array().ok_or_else(|| {
            Error::InvalidReport(format!("`{key}` section must be an array of row objects"))
        })?;
        for (idx, row) in arr.iter().enumerate() {
            if !row.is_object() {
                return Err(Error::InvalidReport(format!(
                    "`{key}[{idx}]` must be a row object"
                )));
            }
            rows.push(row);
        }
    }

    // Validate + parse every row up front so a bad score/evidence
    // type surfaces as InvalidReport rather than silently skipping
    // the row (Ohm's P2 on PR #91 r2). Each cached tuple carries
    // the parsed score/evidence + row ref for subsequent checks.
    struct Parsed<'a> {
        row: &'a Value,
        class: Option<String>,
        score: Option<f64>,
        evidence: Option<f64>,
    }
    let mut parsed: Vec<Parsed<'_>> = Vec::with_capacity(rows.len());
    for r in &rows {
        parsed.push(Parsed {
            row: r,
            class: row_class(r),
            score: row_score(r)?,
            evidence: row_evidence(r)?,
        });
    }

    let trunk_touched: Vec<Finding> = parsed
        .iter()
        .filter(|p| p.class.as_deref() == Some("trunk"))
        .map(|p| build_finding(p.row, "Changed file is classified as trunk."))
        .collect();

    let low_evidence_high_score: Vec<Finding> = parsed
        .iter()
        .filter(|p| {
            // Ruby `row.fetch('score', 0).to_f` — missing score
            // defaults to 0, which never satisfies the >= 0.75
            // check, so an absent score simply never qualifies.
            let score = p.score.unwrap_or(0.0);
            score >= HIGH_SCORE_THRESHOLD && is_low_evidence(p.evidence)
        })
        .map(|p| build_finding(p.row, low_evidence_high_score_reason().as_str()))
        .collect();

    let newness_trunk_adjacent: Vec<Finding> = parsed
        .iter()
        .filter(|p| {
            p.row
                .get("new_file")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && p.row.get("escalation").and_then(Value::as_str) == Some("trunk_adjacent")
        })
        .map(|p| build_finding(p.row, "New file is trunk-adjacent."))
        .collect();

    Ok(GateResult {
        trunk_touched,
        low_evidence_high_score,
        newness_trunk_adjacent,
    })
}

fn low_evidence_high_score_reason() -> String {
    format!(
        "Score is >= {HIGH_SCORE_THRESHOLD} while evidence is nil or < {LOW_EVIDENCE_THRESHOLD}."
    )
}

fn is_low_evidence(v: Option<f64>) -> bool {
    match v {
        None => true,
        Some(n) => n < LOW_EVIDENCE_THRESHOLD,
    }
}

fn row_class(row: &Value) -> Option<String> {
    row.get("class").and_then(Value::as_str).map(str::to_string)
}

/// Ruby-compatible numeric coercion matching `String#to_f`:
///
/// - `nil` → None
/// - number → f64
/// - string: take leading optional sign, decimal/exponent prefix,
///   parse that. Unparseable prefix → 0.0. Stops at the first
///   non-numeric character (`"0.9junk"` → 0.9). Rejects `NaN` /
///   `Infinity` literals (Ruby `to_f` does not recognize those
///   strings — they return 0.0). Also collapses any non-finite
///   parse result (defensive) to 0.0.
/// - bool / array / object → None (Ruby lacks `.to_f` for these
///   and sf-gate inputs should never carry them; skip the row).
///
/// Caught as Ohm's P2 on PR #91 r1: Rust `str::parse::<f64>()`
/// accepts `"NaN"` and refuses `"0.9junk"`; neither matches Ruby.
/// Coerce a row field to f64 using Ruby `to_f` semantics. Returns
/// `Err(Error::InvalidReport)` for JSON types Ruby wouldn't accept
/// — bool, array, object. Those are programming errors in the
/// report, not benign missing data, so we surface them loudly.
/// `null` and absent → None (treated as missing by the caller,
/// which maps score-missing to 0 and evidence-missing to low).
fn coerce_f64(field: &str, v: Option<&Value>) -> Result<Option<f64>, Error> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(Value::String(s)) => Ok(Some(ruby_to_f(s))),
        _ => Err(Error::InvalidReport(format!(
            "`{field}` must be a number, numeric string, or null"
        ))),
    }
}

/// Port of Ruby `String#to_f` (verified against ruby:3.3.13-slim):
/// - Skip leading whitespace.
/// - Optional `+`/`-`.
/// - Digit-sequence (optional `.` + digit-sequence) + optional
///   `e`/`E` + optional sign + digit-sequence.
/// - Digit-sequences allow underscores wedged BETWEEN two digits
///   (Ruby numeric-literal rule). A leading/trailing `_` ends the
///   sequence — in `"1e_1"` or `"_1"` the parser stops before `_`.
/// - No numeric prefix at all → 0.0. `"NaN"`/`"Infinity"` have no
///   leading digit so they parse to 0.0.
/// - Overflow keeps the Infinity result (Ruby `"1e999".to_f` is
///   Infinity, which the >= 0.75 check still satisfies).
///
/// Caught as Ohm's P2 on PR #91 r3: prior hand-rolled walk
/// panicked on `"_1"` (indexed `bytes[end-1]` at end=0) and
/// mishandled underscored exponents (`"1e-0_1"`).
fn ruby_to_f(s: &str) -> f64 {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let mut end = i;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let int_start = end;
    advance_digit_run(bytes, &mut end);
    let had_int_digits = end > int_start;
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        let frac_start = end;
        advance_digit_run(bytes, &mut end);
        let had_frac_digits = end > frac_start;
        // Ruby `.` must have EITHER an integer-part digit before it
        // OR a fractional-part digit after it. Neither → no numeric
        // prefix at all → 0.0.
        if !had_int_digits && !had_frac_digits {
            return 0.0;
        }
    } else if !had_int_digits {
        return 0.0;
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let exp_marker = end;
        let mut exp_end = end + 1;
        if exp_end < bytes.len() && (bytes[exp_end] == b'+' || bytes[exp_end] == b'-') {
            exp_end += 1;
        }
        let exp_digits_start = exp_end;
        advance_digit_run(bytes, &mut exp_end);
        if exp_end == exp_digits_start {
            // Bare `e` with no digits → Ruby snips it (`"1e"` →
            // 1.0, `"1e_1"` → 1.0).
            end = exp_marker;
        } else {
            end = exp_end;
        }
    }
    let raw = &s[i..end];
    let cleaned: String = raw.chars().filter(|c| *c != '_').collect();
    match cleaned.parse::<f64>() {
        Ok(n) if !n.is_nan() => n,
        _ => 0.0,
    }
}

/// Advance `end` across a Ruby-style digit sequence: ascii digits
/// with optional single underscores between consecutive digits.
/// A leading `_` (before any digit) or a trailing `_` (no digit
/// after) terminates the run without consuming the `_`. Never
/// indexes `bytes[end - 1]` without first proving `end > start`.
fn advance_digit_run(bytes: &[u8], end: &mut usize) {
    // Need at least one leading digit before `_` is legal.
    while *end < bytes.len() {
        let b = bytes[*end];
        if b.is_ascii_digit() {
            *end += 1;
            continue;
        }
        if b == b'_' {
            let has_prev_digit = *end > 0 && bytes[*end - 1].is_ascii_digit();
            let has_next_digit = *end + 1 < bytes.len() && bytes[*end + 1].is_ascii_digit();
            if has_prev_digit && has_next_digit {
                *end += 1;
                continue;
            }
        }
        break;
    }
}

fn row_score(row: &Value) -> Result<Option<f64>, Error> {
    coerce_f64("score", row.get("score"))
}

fn row_evidence(row: &Value) -> Result<Option<f64>, Error> {
    coerce_f64("evidence", row.get("evidence"))
}

fn build_finding(row: &Value, reason: &str) -> Finding {
    Finding {
        path: row
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        language: row
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_string),
        // `build_finding` only runs on rows that already passed
        // the parse step in `run`, so unwrap-or-default is safe
        // for score/evidence. Ohm's P2 on PR #91 r2.
        score: row_score(row).ok().flatten(),
        evidence: row_evidence(row).ok().flatten(),
        classification: row_class(row),
        reason: reason.to_string(),
    }
}

/// Render the gate result as Ruby-parity markdown.
///
/// Matches `Gate.markdown(result, enforce:)` verbatim, down to the
/// trailing bytes: the Ruby implementation is a single line-array
/// joined with `\n`, so its return value ends with ONE `\n` (from
/// the join separator preceding the trailing empty-string that
/// each `<details>` block appends). We mirror the same shape by
/// building a `Vec<String>` and joining with `\n`, which also
/// eliminates the previous risk of miscounted trailing newlines
/// (Ohm's non-blocking note on PR #91 — the old
/// `push_str("\n</details>\n\n")` pattern produced `\n\n` at EOF).
pub fn markdown(result: &GateResult, enforce: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    // Header (Ruby `markdown_header`).
    lines.push("## Stud Finder gate".to_string());
    lines.push(String::new());
    lines.push(format!(
        "**Mode:** {}",
        if enforce { "enforce" } else { "observation" }
    ));
    lines.push(format!(
        "**Summary:** {} across {} checks.",
        pluralize(result.finding_count(), "finding"),
        CHECKS.len()
    ));
    lines.push(String::new());
    // Summary bullets (Ruby `markdown_summary`).
    for check in CHECKS {
        let findings = result.for_check(check);
        let status = if findings.is_empty() { "✅" } else { "⚠️" };
        lines.push(format!(
            "- {status} `{check}` — {}",
            pluralize(findings.len(), "finding")
        ));
    }
    lines.push(String::new());
    // Per-check collapsible details (Ruby `markdown_check_detail`).
    for check in CHECKS {
        let findings = result.for_check(check);
        let open = if findings.is_empty() { "" } else { " open" };
        lines.push(format!("<details{open}>"));
        lines.push(format!(
            "<summary><strong>{check}</strong> — {}</summary>",
            pluralize(findings.len(), "finding")
        ));
        lines.push(String::new());
        if findings.is_empty() {
            lines.push("_No findings._".to_string());
        } else {
            lines.push("| file | class | score | evidence | reason |".to_string());
            lines.push("| --- | --- | ---: | ---: | --- |".to_string());
            for f in findings {
                lines.push(format!(
                    "| `{}` | {} | {} | {} | {} |",
                    escape_md(&f.path),
                    escape_md(f.classification.as_deref().unwrap_or("")),
                    format_number(f.score),
                    format_evidence(f.evidence),
                    escape_md(&f.reason)
                ));
            }
        }
        lines.push(String::new());
        lines.push("</details>".to_string());
        lines.push(String::new());
    }
    lines.join("\n")
}

fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn escape_md(s: &str) -> String {
    s.replace('|', "\\|")
}

fn format_number(v: Option<f64>) -> String {
    match v {
        None => "—".to_string(),
        Some(n) => format!("{n:.4}"),
    }
}

fn format_evidence(v: Option<f64>) -> String {
    match v {
        None => "nil".to_string(),
        Some(n) => format!("{n:.4}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "meta": {},
        "warnings": [],
        "ruby": [
            {"path": "a.rb", "class": "trunk", "score": 0.9, "evidence": 1.0, "new_file": false, "escalation": ""},
            {"path": "b.rb", "class": "branch", "score": 0.8, "evidence": 0.4, "new_file": false, "escalation": ""},
            {"path": "c.rb", "class": "branch", "score": 0.2, "evidence": 0.9, "new_file": true, "escalation": "trunk_adjacent"}
        ],
        "javascript": [
            {"path": "x.js", "class": "leaf", "score": 0.1, "evidence": 1.0, "new_file": false, "escalation": ""}
        ]
    }"#;

    #[test]
    fn trunk_touched_flags_only_trunk_rows() {
        let r = run(SAMPLE).expect("valid");
        assert_eq!(r.trunk_touched.len(), 1);
        assert_eq!(r.trunk_touched[0].path, "a.rb");
    }

    #[test]
    fn low_evidence_high_score_uses_both_thresholds() {
        // b.rb has score 0.8 >= 0.75 AND evidence 0.4 < 0.50.
        let r = run(SAMPLE).expect("valid");
        assert_eq!(r.low_evidence_high_score.len(), 1);
        assert_eq!(r.low_evidence_high_score[0].path, "b.rb");
    }

    #[test]
    fn newness_trunk_adjacent_requires_both_fields() {
        // c.rb is new_file + trunk_adjacent.
        let r = run(SAMPLE).expect("valid");
        assert_eq!(r.newness_trunk_adjacent.len(), 1);
        assert_eq!(r.newness_trunk_adjacent[0].path, "c.rb");
    }

    #[test]
    fn missing_evidence_counts_as_low() {
        let json = r#"{"ruby": [{"path": "x.rb", "class": "branch", "score": 0.9, "evidence": null}], "javascript": []}"#;
        let r = run(json).expect("valid");
        assert_eq!(r.low_evidence_high_score.len(), 1);
    }

    #[test]
    fn invalid_json_errors_with_ruby_shape() {
        let err = run("{not json").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.starts_with("Error: invalid JSON input:"),
            "message prefix: {msg}"
        );
    }

    #[test]
    fn markdown_renders_mode_and_summary_header() {
        let r = run(SAMPLE).expect("valid");
        let md = markdown(&r, true);
        assert!(md.starts_with("## Stud Finder gate\n\n"));
        assert!(md.contains("**Mode:** enforce"));
        assert!(md.contains("**Summary:** 3 findings"));
    }

    #[test]
    fn markdown_opens_details_only_for_checks_with_findings() {
        // Build an all-empty result and assert <details> is closed.
        let empty = GateResult::default();
        let md = markdown(&empty, false);
        assert!(md.contains("<details>"));
        assert!(!md.contains("<details open>"));
        // 3 "No findings." lines, one per check.
        assert_eq!(md.matches("_No findings._").count(), 3);
    }

    #[test]
    fn markdown_escapes_pipes_in_paths() {
        let json = r#"{"ruby": [{"path": "weird|path.rb", "class": "trunk", "score": 0.9, "evidence": 1.0}], "javascript": []}"#;
        let r = run(json).expect("valid");
        let md = markdown(&r, false);
        assert!(
            md.contains("weird\\|path.rb"),
            "pipe in path must be escaped: {md}"
        );
    }

    #[test]
    fn pluralize_singular_vs_plural() {
        assert_eq!(pluralize(0, "finding"), "0 findings");
        assert_eq!(pluralize(1, "finding"), "1 finding");
        assert_eq!(pluralize(2, "finding"), "2 findings");
    }

    #[test]
    fn missing_sections_empty_input_yields_zero_findings() {
        let r = run("{}").expect("empty object is valid JSON");
        assert_eq!(r.finding_count(), 0);
    }

    // --- PR #91 Ohm P1 + P2 regressions ---

    #[test]
    fn array_root_is_rejected_as_malformed() {
        let err = run("[]").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("malformed stud-finder report") && msg.contains("root"),
            "expected malformed-root error: {msg}"
        );
    }

    #[test]
    fn scalar_root_is_rejected_as_malformed() {
        let err = run("42").unwrap_err();
        assert!(err.to_string().contains("malformed stud-finder report"));
    }

    #[test]
    fn non_array_section_is_rejected_as_malformed() {
        let err = run(r#"{"ruby": {"path": "a.rb", "class": "trunk"}}"#).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("`ruby` section must be an array"),
            "expected non-array section error: {msg}"
        );
    }

    #[test]
    fn non_object_row_is_rejected_as_malformed() {
        let err = run(r#"{"ruby": ["not-an-object"], "javascript": []}"#).unwrap_err();
        assert!(err.to_string().contains("must be a row object"));
    }

    #[test]
    fn null_section_counts_as_empty() {
        // Ruby `Array(nil) == []`. Explicit null = zero rows, no
        // error.
        let r = run(r#"{"ruby": null, "javascript": null}"#).expect("null = empty");
        assert_eq!(r.finding_count(), 0);
    }

    #[test]
    fn numeric_string_score_triggers_high_score_check() {
        // Ohm P2: "0.9" must coerce the same way Ruby `"0.9".to_f`
        // does. Prior revision (`as_f64()`) treated it as missing
        // and skipped the finding.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":"0.9","evidence":0.1}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(
            r.low_evidence_high_score.len(),
            1,
            "numeric-string score must coerce and trigger finding"
        );
    }

    #[test]
    fn numeric_string_evidence_coerces_to_high() {
        // Conversely, `"0.8"` evidence string must coerce to 0.8 →
        // above the LOW_EVIDENCE threshold → the row does NOT
        // qualify for low_evidence_high_score even with a high
        // score.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":0.9,"evidence":"0.8"}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(
            r.low_evidence_high_score.len(),
            0,
            "numeric-string evidence must coerce and not trigger finding"
        );
    }

    #[test]
    fn score_overflow_is_infinity_matching_ruby() {
        // Ohm P2 on PR #91 r2: Ruby `"1e999".to_f` returns
        // Infinity. Infinity >= 0.75 so the row flags. Prior
        // revision collapsed Infinity to 0.0, missing the finding.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":"1e999","evidence":0.1}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(
            r.low_evidence_high_score.len(),
            1,
            "overflow score must remain high: {:?}",
            r.low_evidence_high_score
        );
    }

    #[test]
    fn underscore_in_numeric_string_matches_ruby_to_f() {
        // Ohm P2 on PR #91 r2: Ruby `"0.7_5".to_f == 0.75`,
        // `"1_0.5".to_f == 10.5`. Rust's native parse rejects
        // underscores; our ruby_to_f strips them when they sit
        // between digits.
        assert_eq!(ruby_to_f("0.7_5"), 0.75);
        assert_eq!(ruby_to_f("1_0.5"), 10.5);
    }

    #[test]
    fn bool_score_or_evidence_is_rejected_as_invalid_report() {
        // Ohm P2 on PR #91 r2: Ruby `true.to_f` raises
        // NoMethodError and Ruby's Gate blows up on this input.
        // We reject with InvalidReport instead of silently
        // treating as missing.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":true,"evidence":0.1}],"javascript":[]}"#;
        let err = run(json).unwrap_err();
        assert!(
            err.to_string().contains("malformed stud-finder report"),
            "expected InvalidReport for bool score: {err}"
        );
    }

    #[test]
    fn array_evidence_is_rejected_as_invalid_report() {
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":0.9,"evidence":[0.1]}],"javascript":[]}"#;
        let err = run(json).unwrap_err();
        assert!(err.to_string().contains("malformed stud-finder report"));
    }

    #[test]
    fn leading_underscore_string_does_not_panic_and_returns_zero() {
        // Ohm P2 on PR #91 r3: `"_1"` previously panicked at
        // `bytes[end - 1]` with end=0. Must now return 0.0 like
        // Ruby `"_1".to_f`.
        assert_eq!(ruby_to_f("_1"), 0.0);
        assert_eq!(ruby_to_f("__"), 0.0);
        assert_eq!(ruby_to_f("_"), 0.0);
    }

    #[test]
    fn exponent_underscore_matches_ruby_to_f() {
        // Ohm P2 on PR #91 r3: `"1e-0_1".to_f == 0.1` in Ruby
        // (exponent is -01 after underscore strip). Prior
        // revision only accepted underscores in the mantissa.
        assert_eq!(ruby_to_f("1e-0_1"), 0.1);
        // Trailing/leading underscore in exponent disallowed
        // (Ruby snips the whole `e...` suffix).
        assert_eq!(ruby_to_f("1e_1"), 1.0);
        assert_eq!(ruby_to_f("1_e1"), 1.0);
    }

    #[test]
    fn exponent_underscore_evidence_flags_low_evidence_high_score() {
        // Composite check — `"1e-0_1"` evidence = 0.1 < 0.5 → low.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":0.9,"evidence":"1e-0_1"}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(
            r.low_evidence_high_score.len(),
            1,
            "`1e-0_1` → 0.1 → low: {:?}",
            r.low_evidence_high_score
        );
    }

    #[test]
    fn ruby_to_f_prefix_coercion_matches_docker_ruby() {
        // Verified against ruby:3.3.13-slim:
        //   "0.9".to_f        => 0.9
        //   "0.9junk".to_f    => 0.9
        //   "NaN".to_f        => 0.0
        //   "Infinity".to_f   => 0.0
        //   "1e3".to_f        => 1000.0
        //   " 0.5 ".to_f      => 0.5
        //   "-0.5".to_f       => -0.5
        //   "junk".to_f       => 0.0
        //   "".to_f           => 0.0
        assert_eq!(ruby_to_f("0.9"), 0.9);
        assert_eq!(ruby_to_f("0.9junk"), 0.9);
        assert_eq!(ruby_to_f("NaN"), 0.0);
        assert_eq!(ruby_to_f("Infinity"), 0.0);
        assert_eq!(ruby_to_f("1e3"), 1000.0);
        assert_eq!(ruby_to_f(" 0.5 "), 0.5);
        assert_eq!(ruby_to_f("-0.5"), -0.5);
        assert_eq!(ruby_to_f("junk"), 0.0);
        assert_eq!(ruby_to_f(""), 0.0);
    }

    #[test]
    fn score_with_numeric_prefix_triggers_high_score_check() {
        // Ohm P2 on PR #91 r1: `"0.9junk".to_f` is 0.9 in Ruby,
        // not NaN or garbage. Previously Rust parse failed and
        // substituted 0.0, missing the finding.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":"0.9junk","evidence":0.1}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(r.low_evidence_high_score.len(), 1);
    }

    #[test]
    fn evidence_nan_literal_coerces_to_low_matching_ruby() {
        // Ohm P2 on PR #91 r1: `"NaN".to_f` is 0.0 in Ruby — NOT
        // a NaN. Previously Rust parsed "NaN" to f64::NAN, which
        // failed the `< 0.50` comparison (NaN < anything is false),
        // so the row wasn't flagged.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":0.9,"evidence":"NaN"}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(
            r.low_evidence_high_score.len(),
            1,
            "NaN-string evidence must coerce to 0.0 (low)"
        );
    }

    #[test]
    fn garbage_string_evidence_coerces_to_zero_matching_ruby() {
        // Ruby `"garbage".to_f == 0.0` → treated as low.
        let json = r#"{"ruby":[{"path":"a.rb","class":"leaf","score":0.9,"evidence":"garbage"}],"javascript":[]}"#;
        let r = run(json).expect("valid");
        assert_eq!(
            r.low_evidence_high_score.len(),
            1,
            "garbage-string evidence must coerce to 0.0 (low): {:?}",
            r.low_evidence_high_score
        );
    }
}
