//! Ruby reference extraction.
//!
//! Given a Ruby source file, produce the list of constants and classes that
//! file references. Those references feed `sf-core::DependencyGraph` which,
//! after a repo-wide pass, lets us compute each file's `fan_in` — how many
//! other files point at it.
//!
//! Two kinds of references are collected:
//!
//! - **Explicit constant uses:** `Foo`, `Foo::Bar`, `::Foo::Bar`, as they
//!   appear in the source. The resolver downstream figures out which file
//!   owns the constant; this crate's job is to emit the written reference.
//! - **Rails-convention associations:** `belongs_to :user`, `has_one :profile`,
//!   `has_many :comments`, `has_and_belongs_to_many :tags`, and literal
//!   `class_name: "Foo::Bar"` options. Association symbols are inflected
//!   into the implied constant name — `:user` → `User`, `:comments`
//!   singularized → `Comment`, `:user_roles` singularized → `UserRole`.
//!
//! The DSL trigger list and the visitor approach are adapted from Saugat
//! Khadka's ripple-ruby crate (commit 6604d76, `crates/ripple-ruby/src/rails.rs`).
//! The MIT notice preservation record is in `rust/ATTRIBUTION.md`.
//!
//! This first-slice extractor deliberately omits: template references
//! (ERB/HAML/Slim), view-helper linking, method-level analysis, rubydex
//! scope resolution, non-trivial pluralization. All of those land in
//! future slices.

mod complexity;
mod inflect;
mod visitor;

use std::path::{Path, PathBuf};

pub use complexity::{ComplexityReport, max_complexity};
pub use inflect::associate_to_class;

/// A single reference pulled out of a Ruby file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RubyRef {
    /// The referenced constant name, as the resolver will look it up.
    /// `::`-prefixed forms are kept normalized without the prefix; absolute
    /// vs scope-relative resolution is the resolver's concern, not ours.
    pub name: String,
    /// What kind of reference this is.
    pub kind: RefKind,
    /// 1-based line number in the source file.
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// An explicit constant reference: `Foo`, `Foo::Bar`.
    Constant,
    /// A Rails association (`belongs_to`, `has_many`, …) whose class name
    /// we inferred from the symbol argument via `inflect`.
    RailsAssociation,
    /// A literal `class_name: "Foo"` option on any DSL call.
    ClassNameOption,
}

/// Parse a Ruby source file and return the references it makes.
///
/// `path` is only used for error messages; parsing is entirely in-memory.
pub fn extract_refs(path: &Path, source: &str) -> Vec<RubyRef> {
    let _ = path;
    visitor::collect(source)
}

/// Convenience wrapper that reads the file from disk before extracting.
pub fn extract_refs_from_file(path: &Path) -> std::io::Result<Vec<RubyRef>> {
    let source = std::fs::read_to_string(path)?;
    Ok(extract_refs(path, &source))
}

/// Result type for repo-walking callers that want a `(file, refs)` pair.
pub type FileRefs = (PathBuf, Vec<RubyRef>);
