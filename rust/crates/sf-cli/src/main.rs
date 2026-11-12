//! stud-finder CLI (Rust port, work in progress).
//!
//! Current behaviour: walks a path, parses every Ruby file with `sf-ruby`,
//! builds a `sf-core::DependencyGraph`, scores it with `sf-score`, and
//! prints the `class | score | fan_in | fan_out | file` table sorted by
//! (class rank, score desc, path asc). JS/TS files are still
//! unprocessed this slice — they land with `sf-js` in a later PR.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use sf_core::{DependencyGraph, FileRef, Language};
use sf_score::{ScoredFile, Scorer, Signals};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "stud-finder",
    about = "Rank files by structural risk (Rust port, WIP)."
)]
struct Cli {
    /// Repository path to scan.
    path: PathBuf,
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
    let signals = Signals {
        graph: &graph,
        complexity,
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
    print_table(&rows);
    Ok(())
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
            "js" | "jsx" | "ts" | "tsx" => Language::JavaScript,
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
    // Register only Ruby files — JS/TS files have no reference extractor
    // this slice and would otherwise distort the percentile distribution
    // (every JS file would sit at fan_in=0, fan_out=0 and push the Ruby
    // percentiles up). sf-js will build its own graph in a later slice.
    for file in files.iter().filter(|f| f.language == Language::Ruby) {
        graph.register_file(file.path.clone());
    }
    for file in files.iter().filter(|f| f.language == Language::Ruby) {
        let abs = root.join(&file.path);
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("stud-finder: read {}: {}", abs.display(), err);
                continue;
            }
        };
        for r in sf_ruby::extract_refs(&file.path, &source) {
            graph.add_reference(file.path.clone(), r.name);
        }
        complexity.insert(file.path.clone(), sf_ruby::max_complexity(&source));
    }
    (graph, complexity)
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

fn width_of(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}
