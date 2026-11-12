//! stud-finder CLI (Rust port, work in progress).
//!
//! Current behaviour: walks a path, parses every Ruby file with `sf-ruby`,
//! builds a `sf-core::DependencyGraph`, and prints one row per Ruby file
//! with its `fan_in` count. JS/TS files are still unprocessed this slice —
//! they land with `sf-js` in a later PR.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use sf_core::{DependencyGraph, FileRef, Language};
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
    let graph = build_graph(&cli.path, &files);
    print_table(&graph, &files);
    Ok(())
}

fn collect(root: &Path) -> Vec<FileRef> {
    let mut out = Vec::new();
    for result in WalkDir::new(root) {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("stud-finder: skip {}: {}", err.path().map(Path::display).map_or_else(|| "<unknown>".to_string(), |p| p.to_string()), err);
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

fn build_graph(root: &Path, files: &[FileRef]) -> DependencyGraph {
    let mut graph = DependencyGraph::new();
    for file in files {
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
    }
    graph
}

fn print_table(graph: &DependencyGraph, files: &[FileRef]) {
    let mut rows: Vec<(usize, &Path)> = files
        .iter()
        .filter(|f| f.language == Language::Ruby)
        .map(|f| (graph.fan_in(&f.path), f.path.as_path()))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

    let widest = rows
        .iter()
        .map(|(fi, _)| width_of(*fi))
        .max()
        .unwrap_or(1)
        .max("fan_in".len());

    println!("{:>width$}  {}", "fan_in", "file", width = widest);
    for (fan_in, path) in rows {
        println!("{:>width$}  {}", fan_in, path.display(), width = widest);
    }
}

fn width_of(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}
