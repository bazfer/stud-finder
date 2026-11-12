//! Shared data model and dependency-graph primitives.
//!
//! Signal crates (`sf-ruby`, `sf-js`, `sf-history`, `sf-coverage`) will
//! produce per-file facts in these types; `sf-score` will consume those
//! facts to produce the stud-finder composite score, classification, and
//! evidence. Status: skeleton — only the file/language types exist so
//! `sf-cli` can walk a tree.

use std::path::PathBuf;

/// A repo-relative file path tagged with the language stud-finder treats
/// it as. Non-source files are filtered out before this struct is built,
/// so the `language` field is always one of the languages we score.
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
