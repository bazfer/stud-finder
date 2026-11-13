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
/// `HIGH_SCORE_THRESHOLD = 0.75`.
pub const HIGH_SCORE_THRESHOLD: f64 = 0.75;

/// Evidence threshold for `low_evidence_high_score` — matches Ruby
/// `LOW_EVIDENCE_THRESHOLD = 0.50`. Missing/null evidence counts
/// as low.
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidJson(msg) => write!(f, "Error: invalid JSON input: {msg}"),
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
    let mut rows: Vec<&Value> = Vec::new();
    for key in ["ruby", "javascript"] {
        if let Some(arr) = payload.get(key).and_then(|v| v.as_array()) {
            rows.extend(arr.iter());
        }
    }

    let trunk_touched: Vec<Finding> = rows
        .iter()
        .filter(|r| row_class(r).as_deref() == Some("trunk"))
        .map(|r| build_finding(r, "Changed file is classified as trunk."))
        .collect();

    let low_evidence_high_score: Vec<Finding> = rows
        .iter()
        .filter(|r| {
            let score = row_score(r).unwrap_or(0.0);
            score >= HIGH_SCORE_THRESHOLD && is_low_evidence(row_evidence(r))
        })
        .map(|r| build_finding(r, low_evidence_high_score_reason().as_str()))
        .collect();

    let newness_trunk_adjacent: Vec<Finding> = rows
        .iter()
        .filter(|r| {
            r.get("new_file").and_then(Value::as_bool).unwrap_or(false)
                && r.get("escalation").and_then(Value::as_str) == Some("trunk_adjacent")
        })
        .map(|r| build_finding(r, "New file is trunk-adjacent."))
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

fn row_score(row: &Value) -> Option<f64> {
    row.get("score").and_then(Value::as_f64)
}

fn row_evidence(row: &Value) -> Option<f64> {
    row.get("evidence").and_then(Value::as_f64)
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
        score: row_score(row),
        evidence: row_evidence(row),
        classification: row_class(row),
        reason: reason.to_string(),
    }
}

/// Render the gate result as Ruby-parity markdown.
///
/// Matches `Gate.markdown(result, enforce:)` verbatim on shape:
/// `## Stud Finder gate` title, `**Mode:**` / `**Summary:**` lines,
/// one bullet per check with a ✅/⚠️ status, then collapsed
/// `<details>` blocks per check containing either `_No findings._`
/// or a markdown table.
pub fn markdown(result: &GateResult, enforce: bool) -> String {
    let mut out = String::new();
    out.push_str("## Stud Finder gate\n\n");
    out.push_str(&format!(
        "**Mode:** {}\n",
        if enforce { "enforce" } else { "observation" }
    ));
    out.push_str(&format!(
        "**Summary:** {} across {} checks.\n\n",
        pluralize(result.finding_count(), "finding"),
        CHECKS.len()
    ));
    // Summary bullets.
    for check in CHECKS {
        let findings = result.for_check(check);
        let status = if findings.is_empty() { "✅" } else { "⚠️" };
        out.push_str(&format!(
            "- {status} `{check}` — {}\n",
            pluralize(findings.len(), "finding")
        ));
    }
    out.push('\n');
    // Per-check collapsible details.
    for check in CHECKS {
        let findings = result.for_check(check);
        let open = if findings.is_empty() { "" } else { " open" };
        out.push_str(&format!("<details{open}>\n"));
        out.push_str(&format!(
            "<summary><strong>{check}</strong> — {}</summary>\n\n",
            pluralize(findings.len(), "finding")
        ));
        if findings.is_empty() {
            out.push_str("_No findings._\n");
        } else {
            out.push_str("| file | class | score | evidence | reason |\n");
            out.push_str("| --- | --- | ---: | ---: | --- |\n");
            for f in findings {
                out.push_str(&format!(
                    "| `{}` | {} | {} | {} | {} |\n",
                    escape_md(&f.path),
                    escape_md(f.classification.as_deref().unwrap_or("")),
                    format_number(f.score),
                    format_evidence(f.evidence),
                    escape_md(&f.reason)
                ));
            }
        }
        out.push_str("\n</details>\n\n");
    }
    out
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
}
