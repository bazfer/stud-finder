//! stud-finder CLI (Rust port, work in progress).
//!
//! Current behaviour: walks a path, parses every Ruby file with `sf-ruby`,
//! builds a `sf-core::DependencyGraph`, pulls churn + coupling signals
//! from `sf-history` over the same repo, scores it with `sf-score`, and
//! prints the
//! `class | score | fan_in | fan_out | complexity | churn | coupling | file`
//! table sorted by (class rank, score desc, path asc). JS/TS files are
//! still unprocessed this slice — they land with `sf-js` in a later PR.

// Tests sit mid-file so the exclusion regression lives right next to
// the `analyze_ruby_source` function it exercises; print helpers follow.
#![allow(clippy::items_after_test_module)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use sf_core::{DependencyGraph, FileRef, Language};
use sf_history::{DEFAULT_WINDOW_DAYS, HistoryWarning};
use sf_score::{CouplingSignal, ScoredFile, Scorer, Signals};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "stud-finder",
    about = "Rank files by structural risk (Rust port, WIP)."
)]
struct Cli {
    /// Repository path to scan.
    path: PathBuf,
    /// Path to a coverage report (SimpleCov `resultset.json`, LCOV
    /// `.info`, or Cobertura XML). Format is picked by extension.
    /// When provided, enables the coverage signal + the interaction
    /// cross-term in the composite.
    #[arg(long)]
    coverage: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let files = collect(&cli.path);
    // Score the Ruby graph on its own — JS/TS files are unprocessed in
    // this slice and must NOT participate in the percentile calculation
    // (they would otherwise pin a fan_in=0/fan_out=0 row into the
    // distribution and depress Ruby percentiles). sf-js builds its own
    // graph in a later slice; each language gets its own ranking
    // section, matching the Ruby implementation.
    let (graph, complexity) = build_signals(&cli.path, &files);

    // Pull churn + coupling from git history over the same registered
    // Ruby files. A non-git-repo or missing `git` degrades to zero
    // signals with a stderr warning — the run still completes.
    let ruby_paths: Vec<PathBuf> = graph.files().map(Path::to_path_buf).collect();
    let ruby_refs: Vec<&Path> = ruby_paths.iter().map(PathBuf::as_path).collect();
    let history = sf_history::analyze(&cli.path, &ruby_refs, DEFAULT_WINDOW_DAYS);
    for warning in &history.warnings {
        match warning {
            HistoryWarning::BulkCommitsSkipped {
                count,
                max_commit_files,
            } => eprintln!(
                "stud-finder: {count} bulk commits (> {max_commit_files} files) skipped from coupling"
            ),
            HistoryWarning::NotAGitRepo => eprintln!(
                "stud-finder: {} is not a git repository; churn + coupling signals are skipped",
                cli.path.display()
            ),
            HistoryWarning::GitNotFound => {
                eprintln!(
                    "stud-finder: `git` not found in PATH; churn + coupling signals are skipped"
                )
            }
            HistoryWarning::GitFailed(msg) => eprintln!("stud-finder: git log failed: {msg}"),
        }
    }
    let coupling: HashMap<PathBuf, CouplingSignal> = history
        .coupling
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                CouplingSignal {
                    partner: v.partner,
                    ratio: v.ratio,
                    co_changes: v.co_changes,
                },
            )
        })
        .collect();

    // Coverage (optional, from --coverage flag). `None` means the
    // flag wasn't provided — coverage + interaction stay out of the
    // composite. `Some(_)` activates both signals even if the loaded
    // report ended up empty (Ruby `coverage_available? = !@coverage.nil?`
    // semantics).
    let coverage = load_coverage(cli.coverage.as_deref(), &cli.path, &ruby_refs);

    let signals = Signals {
        graph: &graph,
        complexity,
        churn_commits: history.churn_commits,
        churn_lines: history.churn_lines,
        coupling,
        coverage,
    };
    let scored = Scorer::new().score_with_signals(signals);
    let mut rows = scored;
    rows.sort_by(|a, b| {
        a.class
            .rank()
            .cmp(&b.class.rank())
            .then_with(|| {
                b.composite
                    .partial_cmp(&a.composite)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.path.cmp(&b.path))
    });
    println!("# Ruby");
    print_table(&rows);

    // Build the JavaScript section on its own graph + complexity so
    // JS files don't pool into the Ruby percentile distribution.
    // sf-score's availability-mode renormalization handles the
    // "fan_in + complexity" subset cleanly — no coverage / churn /
    // coupling on the JS side this slice.
    let (js_graph, js_complexity) = build_js_signals(&cli.path, &files);
    if js_graph.files().count() > 0 {
        let js_signals = Signals {
            graph: &js_graph,
            complexity: js_complexity,
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
        };
        let mut js_rows = Scorer::new().score_with_signals(js_signals);
        js_rows.sort_by(|a, b| {
            a.class
                .rank()
                .cmp(&b.class.rank())
                .then_with(|| {
                    b.composite
                        .partial_cmp(&a.composite)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.path.cmp(&b.path))
        });
        println!();
        println!("# JavaScript");
        print_js_table(&js_rows);
    }

    Ok(())
}

fn build_js_signals(root: &Path, files: &[FileRef]) -> (DependencyGraph, HashMap<PathBuf, usize>) {
    let mut graph = DependencyGraph::new();
    let mut complexity: HashMap<PathBuf, usize> = HashMap::new();

    // Pre-flight parse each JS file so we know which ones to EXCLUDE
    // from the scored population. Matches Ohm's exclusion-not-
    // imputation policy from PR #78, now applied to JS: a file that
    // fails to parse must not be registered + must not sink its
    // neighbours' percentiles. Fixes Ohm's parse-error zero-imputation
    // callout on PR #81.
    let mut included: Vec<(&Path, String, usize)> = Vec::new();
    for file in files.iter().filter(|f| f.language == Language::JavaScript) {
        let abs = root.join(&file.path);
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("stud-finder: read {}: {}", abs.display(), err);
                continue;
            }
        };
        let report = sf_js::max_complexity(&source, Some(file.path.as_path()));
        if !report.parsed_cleanly {
            eprintln!(
                "stud-finder: {} had parse errors; excluding the file from the JS scored population",
                file.path.display()
            );
            continue;
        }
        included.push((file.path.as_path(), source, report.complexity));
    }

    // Register only the Included files, then wire path-exact edges.
    for (path, _, _) in &included {
        graph.register_file(path.to_path_buf());
    }
    for (path, source, cx) in &included {
        complexity.insert(path.to_path_buf(), *cx);
        let refs = sf_js::extract_refs(path, source);
        for r in refs {
            if let Some(resolved) = resolve_js_module(&graph, &r.target) {
                // Path-exact edge — bypasses sf-core's Zeitwerk
                // basename resolver so `a/index.js` and `b/index.js`
                // stay distinct. Fixes Ohm's P1 on PR #81.
                graph.add_path_edge(path.to_path_buf(), resolved);
            }
        }
    }

    (graph, complexity)
}

const JS_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];

/// Resolves a target produced by `sf_js::extract_refs` against the
/// files registered in `graph`. Order (Ohm's P1 on PR #81):
///   1. If the target already has one of the recognised JS extensions
///      AND is registered exactly, use it as-is.
///   2. Otherwise, if the target has NO recognised extension, try
///      each `.ts/.tsx/.js/.jsx/.mjs/.cjs` suffix.
///   3. Otherwise, try `<target>/index.<ext>` for each extension.
///
/// An explicit extension that doesn't match a registered file is NOT
/// silently rewritten (that would create false edges).
fn resolve_js_module(graph: &DependencyGraph, target: &Path) -> Option<PathBuf> {
    let has_js_ext = target
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| JS_EXTS.contains(&ext));

    if has_js_ext {
        // Exact-extension spec: ONLY accept a registered file at that
        // exact path. Don't fall through to extension inference.
        if graph.files().any(|f| f == target) {
            return Some(target.to_path_buf());
        }
        return None;
    }

    // Extensionless target: try each JS extension.
    for ext in JS_EXTS {
        let candidate = target.with_extension(ext);
        if graph.files().any(|f| f == candidate.as_path()) {
            return Some(candidate);
        }
    }
    // Directory-style: `<target>/index.<ext>`.
    for ext in JS_EXTS {
        let candidate = target.join(format!("index.{ext}"));
        if graph.files().any(|f| f == candidate.as_path()) {
            return Some(candidate);
        }
    }
    None
}

fn collect(root: &Path) -> Vec<FileRef> {
    let mut out = Vec::new();
    for result in WalkDir::new(root) {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!(
                    "stud-finder: skip {}: {}",
                    err.path()
                        .map(Path::display)
                        .map_or_else(|| "<unknown>".to_string(), |p| p.to_string()),
                    err
                );
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let language = match ext {
            "rb" => Language::Ruby,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Language::JavaScript,
            _ => continue,
        };
        out.push(FileRef {
            path: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
            language,
        });
    }
    out
}

fn build_signals(root: &Path, files: &[FileRef]) -> (DependencyGraph, HashMap<PathBuf, usize>) {
    let mut graph = DependencyGraph::new();
    let mut complexity: HashMap<PathBuf, usize> = HashMap::new();
    // Read + pre-flight-parse every Ruby file. Files with prism parse
    // errors are EXCLUDED from the scored population entirely — not
    // just from the complexity map. Imputation would otherwise leave a
    // parse-failed file with complexity defaulted to 0 but still
    // present in the fan_in/fan_out graph, pulling down the complexity
    // percentile of healthy neighbours. JS/TS files are skipped
    // outright: no extractor this slice.
    for file in files.iter().filter(|f| f.language == Language::Ruby) {
        let abs = root.join(&file.path);
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("stud-finder: read {}: {}", abs.display(), err);
                continue;
            }
        };
        match analyze_ruby_source(&source) {
            Analysis::Included(refs, cx) => {
                graph.register_file(file.path.clone());
                for r in refs {
                    graph.add_reference(file.path.clone(), r.name);
                }
                complexity.insert(file.path.clone(), cx);
            }
            Analysis::Excluded => {
                eprintln!(
                    "stud-finder: {} had parse errors; excluding the file from the scored population",
                    file.path.display()
                );
            }
        }
    }
    (graph, complexity)
}

/// Outcome of a per-file Ruby pre-flight. `Included` carries the refs
/// and the file-level complexity; `Excluded` is for files prism
/// couldn't parse cleanly — those files leave the scored population
/// entirely, matching Ohm's exclusion-not-imputation policy on PR #78.
enum Analysis {
    Included(Vec<sf_ruby::RubyRef>, usize),
    Excluded,
}

fn analyze_ruby_source(source: &str) -> Analysis {
    let report = sf_ruby::max_complexity(source);
    if !report.parsed_cleanly {
        return Analysis::Excluded;
    }
    let refs = sf_ruby::extract_refs(Path::new(""), source);
    Analysis::Included(refs, report.complexity)
}

/// Load a coverage report when `--coverage <path>` is supplied.
/// Errors are non-fatal — they print to stderr and the run proceeds
/// with an empty coverage map (coverage + interaction drop out of
/// the composite). Missing files reported by sf-coverage are logged
/// as a single informational line, matching Ruby CLI behaviour.
fn load_coverage(
    path: Option<&Path>,
    root: &Path,
    files: &[&Path],
) -> Option<HashMap<PathBuf, f64>> {
    // `None` → user didn't pass --coverage; coverage + interaction
    // stay out of the composite. `Some(_)` activates both signals
    // even if the loaded map is empty (Ruby
    // `coverage_available? = !@coverage.nil?`). On load failure we
    // also return `None` so the run degrades cleanly instead of
    // activating an all-risk-1.0 column that would mislead.
    let path = path?;
    match sf_coverage::load(path, root, files) {
        Ok(report) => {
            if !report.missing_files.is_empty() {
                eprintln!(
                    "stud-finder: {} files in the dependency graph had no coverage entry",
                    report.missing_files.len()
                );
            }
            for warning in &report.warnings {
                match warning {
                    sf_coverage::CoverageWarning::UnmatchedReportedPath(p) => eprintln!(
                        "stud-finder: coverage report path `{p}` did not match any Ruby file"
                    ),
                }
            }
            Some(report.coverage_pct)
        }
        Err(err) => {
            eprintln!(
                "stud-finder: failed to load coverage report `{}`: {err}",
                path.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_ruby_source_is_included_with_refs_and_complexity() {
        let src = r#"
            class Order
              def process(x)
                if x > 0
                  Payment.capture
                end
              end
            end
        "#;
        match analyze_ruby_source(src) {
            Analysis::Included(refs, cx) => {
                assert!(refs.iter().any(|r| r.name == "Payment"));
                assert!(cx >= 2, "if adds +1, so complexity >= 2; got {cx}");
            }
            Analysis::Excluded => panic!("clean source should be Included"),
        }
    }

    #[test]
    fn parse_failing_ruby_source_is_excluded() {
        // Ohm's P1-on-PR#78-review-2 regression: a parse-failed file
        // must not enter the scored population at all — no refs, no
        // complexity, no graph registration. Returning `Excluded`
        // guarantees `build_signals` skips every per-file step for
        // this source.
        let src = "class Foo\n  this is: not; valid! ruby !!\nend\n";
        assert!(
            matches!(analyze_ruby_source(src), Analysis::Excluded),
            "a file with prism parse errors must produce Excluded"
        );
    }

    #[test]
    fn exclusion_keeps_neighbour_scores_independent_of_broken_file() {
        // End-to-end: a graph built from a mix of clean + broken sources
        // (via `analyze_ruby_source` + manual `DependencyGraph` wiring,
        // the same shape `build_signals` uses) produces IDENTICAL
        // ScoredFile rows for the clean files as a graph built from the
        // clean files alone. Imputation (register broken + default
        // complexity to 0) would shift neighbour percentiles.
        use sf_core::DependencyGraph;
        use sf_score::{Scorer, Signals};

        let clean_a = "class A\n  def go(x)\n    if x then B.do end\n  end\nend\n";
        let clean_b = "class B\n  def hi\n    C.m\n  end\nend\n";
        let clean_c = "class C\n  def m; 1; end\nend\n";
        let broken = "class Broken\n  this is: not; valid! ruby !!\nend\n";

        let include = |graph: &mut DependencyGraph,
                       cx: &mut HashMap<PathBuf, usize>,
                       path: &str,
                       source: &str| {
            if let Analysis::Included(refs, c) = analyze_ruby_source(source) {
                let p = PathBuf::from(path);
                graph.register_file(p.clone());
                for r in refs {
                    graph.add_reference(p.clone(), r.name);
                }
                cx.insert(p, c);
            }
        };

        let (mut g_mixed, mut cx_mixed) = (DependencyGraph::new(), HashMap::new());
        include(&mut g_mixed, &mut cx_mixed, "a.rb", clean_a);
        include(&mut g_mixed, &mut cx_mixed, "b.rb", clean_b);
        include(&mut g_mixed, &mut cx_mixed, "c.rb", clean_c);
        // Broken file is offered but must be dropped by Analysis::Excluded.
        include(&mut g_mixed, &mut cx_mixed, "broken.rb", broken);

        let (mut g_clean, mut cx_clean) = (DependencyGraph::new(), HashMap::new());
        include(&mut g_clean, &mut cx_clean, "a.rb", clean_a);
        include(&mut g_clean, &mut cx_clean, "b.rb", clean_b);
        include(&mut g_clean, &mut cx_clean, "c.rb", clean_c);

        let scorer = Scorer::new();
        let mixed_rows = scorer.score_with_signals(Signals {
            graph: &g_mixed,
            complexity: cx_mixed,
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
        });
        let clean_rows = scorer.score_with_signals(Signals {
            graph: &g_clean,
            complexity: cx_clean,
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
        });

        assert_eq!(
            mixed_rows.len(),
            clean_rows.len(),
            "the parse-failed file must not appear in the scored rows"
        );
        for clean_row in &clean_rows {
            let mixed_row = mixed_rows
                .iter()
                .find(|r| r.path == clean_row.path)
                .unwrap_or_else(|| {
                    panic!(
                        "missing row for {} in the mixed graph",
                        clean_row.path.display()
                    )
                });
            assert!(
                (mixed_row.composite - clean_row.composite).abs() < 1e-9,
                "exclusion must not shift the composite of clean files; {} drifted {} vs {}",
                clean_row.path.display(),
                mixed_row.composite,
                clean_row.composite
            );
            assert_eq!(mixed_row.class, clean_row.class);
        }
    }
}

fn print_js_table(rows: &[ScoredFile]) {
    // JS table: class | score | fan_in | fan_out | complexity | file.
    // fan_out is included because sf-score always activates it when
    // the graph has any edges; the Ruby JS-section scorer uses only
    // fan_in + complexity, but surfacing fan_out in the output keeps
    // the composite's weighting explicit rather than hidden.
    let class_width = rows
        .iter()
        .map(|r| r.class.as_str().len())
        .max()
        .unwrap_or(0)
        .max("class".len());
    let fan_in_width = rows
        .iter()
        .map(|r| width_of(r.fan_in))
        .max()
        .unwrap_or(1)
        .max("fan_in".len());
    let fan_out_width = rows
        .iter()
        .map(|r| width_of(r.fan_out))
        .max()
        .unwrap_or(1)
        .max("fan_out".len());
    let complexity_width = rows
        .iter()
        .map(|r| width_of(r.complexity))
        .max()
        .unwrap_or(1)
        .max("complexity".len());
    let score_width = "0.0000".len();

    println!(
        "{:<cw$}  {:>sw$}  {:>fi$}  {:>fo$}  {:>cx$}  file",
        "class",
        "score",
        "fan_in",
        "fan_out",
        "complexity",
        cw = class_width,
        sw = score_width,
        fi = fan_in_width,
        fo = fan_out_width,
        cx = complexity_width,
    );
    for r in rows {
        println!(
            "{:<cw$}  {:>sw$.4}  {:>fi$}  {:>fo$}  {:>cx$}  {}",
            r.class.as_str(),
            r.composite,
            r.fan_in,
            r.fan_out,
            r.complexity,
            r.path.display(),
            cw = class_width,
            sw = score_width,
            fi = fan_in_width,
            fo = fan_out_width,
            cx = complexity_width,
        );
    }
}

fn print_table(rows: &[ScoredFile]) {
    let class_width = rows
        .iter()
        .map(|r| r.class.as_str().len())
        .max()
        .unwrap_or(0)
        .max("class".len());
    let fan_in_width = rows
        .iter()
        .map(|r| width_of(r.fan_in))
        .max()
        .unwrap_or(1)
        .max("fan_in".len());
    let fan_out_width = rows
        .iter()
        .map(|r| width_of(r.fan_out))
        .max()
        .unwrap_or(1)
        .max("fan_out".len());
    let complexity_width = rows
        .iter()
        .map(|r| width_of(r.complexity))
        .max()
        .unwrap_or(1)
        .max("complexity".len());
    let churn_width = rows
        .iter()
        .map(|r| width_of(r.churn_commits))
        .max()
        .unwrap_or(1)
        .max("churn".len());
    let coupling_width = "0.00".len().max("coupling".len());
    let coverage_width = "0.00".len().max("coverage".len());
    let score_width = "0.0000".len();

    println!(
        "{:<cw$}  {:>sw$}  {:>fi$}  {:>fo$}  {:>cx$}  {:>ch$}  {:>cp$}  {:>cv$}  file",
        "class",
        "score",
        "fan_in",
        "fan_out",
        "complexity",
        "churn",
        "coupling",
        "coverage",
        cw = class_width,
        sw = score_width,
        fi = fan_in_width,
        fo = fan_out_width,
        cx = complexity_width,
        ch = churn_width,
        cp = coupling_width,
        cv = coverage_width,
    );
    for r in rows {
        println!(
            "{:<cw$}  {:>sw$.4}  {:>fi$}  {:>fo$}  {:>cx$}  {:>ch$}  {:>cp$.2}  {:>cv$.2}  {}",
            r.class.as_str(),
            r.composite,
            r.fan_in,
            r.fan_out,
            r.complexity,
            r.churn_commits,
            r.max_coupling,
            r.coverage,
            r.path.display(),
            cw = class_width,
            sw = score_width,
            fi = fan_in_width,
            fo = fan_out_width,
            cx = complexity_width,
            ch = churn_width,
            cp = coupling_width,
            cv = coverage_width,
        );
    }
}

fn width_of(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}
