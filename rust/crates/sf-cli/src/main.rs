//! stud-finder CLI (Rust port, work in progress).
//!
//! Current behaviour: walks a path, counts Ruby and JS/TS files, prints
//! the totals. Exists to prove the workspace builds and the file-walk
//! layer works end to end before signals are plumbed in.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use sf_core::{FileRef, Language};
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
    let ruby = files.iter().filter(|f| f.language == Language::Ruby).count();
    let js = files
        .iter()
        .filter(|f| f.language == Language::JavaScript)
        .count();
    println!(
        "{} ruby, {} javascript/typescript, {} total",
        ruby,
        js,
        files.len()
    );
    Ok(())
}

fn collect(root: &Path) -> Vec<FileRef> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
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
