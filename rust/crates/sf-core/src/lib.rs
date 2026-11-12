//! Shared data model and dependency-graph primitives.
//!
//! Signal crates (`sf-ruby`, `sf-js`, `sf-history`, `sf-coverage`) produce
//! per-file facts; `sf-score` consumes those facts to produce the
//! stud-finder composite score, classification, and evidence. `sf-cli`
//! glues them together.
//!
//! Current surface: the file/language tag, a minimal dependency graph
//! keyed by "primary class" (basename-derived), and a `fan_in` lookup.
//! The primary-class resolver is deliberately simple — it recovers the
//! Zeitwerk-style file ↔ class correspondence only for files whose class
//! name matches the basename directly. Nested-namespace files, Rails-root
//! stripping (`app/models/`, `app/services/`, `lib/`), and multi-constant
//! files all come in later slices.

mod graph;

use std::path::PathBuf;

pub use graph::DependencyGraph;

/// A repo-relative file path tagged with the language stud-finder treats
/// it as. Non-source files are filtered out before this struct is built.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileRef {
    pub path: PathBuf,
    pub language: Language,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Ruby,
    JavaScript,
}
