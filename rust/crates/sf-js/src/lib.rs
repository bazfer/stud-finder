//! JS/TS static analysis for stud-finder: module-graph fan_in via
//! import/require resolution, and per-function cyclomatic complexity.
//! Uses `oxc`'s AST and generated `Visit` trait walkers so default
//! traversal covers every child position of the nodes we don't care
//! about (function bodies, class members, nested expressions, etc.).
//!
//! stud-finder Ruby's `JsFanIn` + `JsComplexity` shell out to
//! `dependency-cruiser` and `eslint`. This crate is a deliberate
//! Rust-native upgrade using oxc so the port runs without node.
//! `rust/ATTRIBUTION.md` carries the provenance note.
//!
//! What's recognised:
//!
//! - `extract_refs(path, source)` → relative module specifiers only.
//!   Bare packages (`react`, `@scope/x`) are dropped. Static imports,
//!   dynamic `import('…')`, `require('…')`, `export * from '…'`,
//!   `export { x } from '…'`. Found anywhere — inside function bodies,
//!   class methods, object literals, conditionals, call arguments,
//!   member chains (`require('…').foo`), etc.
//! - `max_complexity(source, path)` → max cyclomatic complexity across
//!   EVERY function body in the file: top-level, exported, class
//!   methods, object/array methods, arrow callbacks. Each function
//!   starts at 1 and gains +1 per decision; nested functions do NOT
//!   inflate their enclosing function's count (they're their own
//!   entries in the max). Decisions: if, ternary, `||`/`&&`/`??`,
//!   case clauses (default doesn't count), for / for-in / for-of,
//!   while / do-while, catch.

use oxc_allocator::Allocator;
use oxc_ast::Visit;
use oxc_ast::ast::{
    CatchClause, ConditionalExpression, DoWhileStatement, ForInStatement, ForOfStatement,
    ForStatement, IfStatement, LogicalExpression, SwitchCase, WhileStatement,
};
use oxc_ast::visit::walk;
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::operator::LogicalOperator;
use std::path::{Component, Path, PathBuf};

/// Per-file reference. `target` is derived from the caller's `path` +
/// the import specifier; the CLI resolves it against its walkdir set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsRef {
    pub target: PathBuf,
}

/// Complexity report mirrors sf-ruby's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComplexityReport {
    pub complexity: usize,
    pub parsed_cleanly: bool,
}

/// Extract resolved relative module references from `source`. `path`
/// is the importing file's repo-root-relative path and is used to
/// anchor relative specifiers.
pub fn extract_refs(path: &Path, source: &str) -> Vec<JsRef> {
    let allocator = Allocator::default();
    let Some(source_type) = source_type_for(path) else {
        return Vec::new();
    };
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked {
        return Vec::new();
    }
    let mut collector = ImportCollector {
        specifiers: Vec::new(),
    };
    collector.visit_program(&parsed.program);

    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(""));
    collector
        .specifiers
        .into_iter()
        .filter(|s| is_relative_specifier(s))
        .map(|s| JsRef {
            target: resolve_relative(&base_dir, &s),
        })
        .collect()
}

/// Max cyclomatic complexity across EVERY function in the file. Picks
/// the right `SourceType` from `path` (plain TS vs TSX matters for
/// angle-bracket type assertions like `<Foo>value`). When `path` is
/// None or extension is unrecognized, defaults to TSX (the most
/// permissive for the constructs we count).
pub fn max_complexity(source: &str, path: Option<&Path>) -> ComplexityReport {
    let allocator = Allocator::default();
    let source_type = path
        .and_then(source_type_for)
        .unwrap_or_else(SourceType::tsx);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let parsed_cleanly = parsed.errors.is_empty();
    if parsed.panicked {
        return ComplexityReport {
            complexity: 0,
            parsed_cleanly: false,
        };
    }
    let mut visitor = ComplexityVisitor {
        stack: Vec::new(),
        max: 0,
    };
    visitor.visit_program(&parsed.program);
    ComplexityReport {
        complexity: visitor.max,
        parsed_cleanly,
    }
}

fn source_type_for(path: &Path) -> Option<SourceType> {
    let ext = path.extension().and_then(|s| s.to_str())?;
    match ext {
        "ts" => Some(SourceType::ts()),
        "tsx" => Some(SourceType::tsx()),
        "jsx" => Some(SourceType::jsx()),
        "js" | "mjs" | "cjs" => Some(SourceType::cjs()),
        _ => None,
    }
}

fn is_relative_specifier(s: &str) -> bool {
    s.starts_with("./") || s.starts_with("../") || s == "." || s == ".."
}

fn resolve_relative(base_dir: &Path, specifier: &str) -> PathBuf {
    let mut parts: Vec<String> = base_dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(x) => x.to_str().map(str::to_string),
            _ => None,
        })
        .collect();
    for seg in specifier.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            s => parts.push(s.to_string()),
        }
    }
    PathBuf::from(parts.join("/"))
}

// --- import collection ---

struct ImportCollector {
    specifiers: Vec<String>,
}

impl<'a> Visit<'a> for ImportCollector {
    fn visit_import_declaration(&mut self, it: &oxc_ast::ast::ImportDeclaration<'a>) {
        self.specifiers.push(it.source.value.to_string());
        walk::walk_import_declaration(self, it);
    }

    fn visit_export_named_declaration(&mut self, it: &oxc_ast::ast::ExportNamedDeclaration<'a>) {
        if let Some(src) = &it.source {
            self.specifiers.push(src.value.to_string());
        }
        walk::walk_export_named_declaration(self, it);
    }

    fn visit_export_all_declaration(&mut self, it: &oxc_ast::ast::ExportAllDeclaration<'a>) {
        self.specifiers.push(it.source.value.to_string());
        walk::walk_export_all_declaration(self, it);
    }

    fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
        // `require('…')`.
        let is_require = matches!(
            &it.callee,
            oxc_ast::ast::Expression::Identifier(id) if id.name == "require"
        );
        if is_require && it.arguments.len() == 1 {
            if let Some(arg) = it.arguments.first() {
                if let Some(oxc_ast::ast::Expression::StringLiteral(lit)) = arg.as_expression() {
                    self.specifiers.push(lit.value.to_string());
                }
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &oxc_ast::ast::ImportExpression<'a>) {
        if let oxc_ast::ast::Expression::StringLiteral(lit) = &it.source {
            self.specifiers.push(lit.value.to_string());
        }
        walk::walk_import_expression(self, it);
    }
}

// --- complexity: single-pass, stack-scoped visitor ---

/// Walks the AST once, pushing a fresh 0 on `stack` at every function
/// boundary and popping it on exit to update `max`. Decision nodes
/// `+1` the top of the stack — scoped automatically to the innermost
/// enclosing function. No stored references outside the visitor, no
/// unsafe transmutes. Addresses Ohm's non-blocking note on PR #81.
struct ComplexityVisitor {
    stack: Vec<usize>,
    max: usize,
}

impl ComplexityVisitor {
    fn bump(&mut self) {
        if let Some(top) = self.stack.last_mut() {
            *top += 1;
        }
    }
    fn enter_function(&mut self) {
        self.stack.push(0);
    }
    fn leave_function(&mut self) {
        if let Some(decisions) = self.stack.pop() {
            let c = 1 + decisions;
            if c > self.max {
                self.max = c;
            }
        }
    }
}

impl<'a> Visit<'a> for ComplexityVisitor {
    fn visit_function(
        &mut self,
        it: &oxc_ast::ast::Function<'a>,
        flags: oxc_syntax::scope::ScopeFlags,
    ) {
        self.enter_function();
        walk::walk_function(self, it, flags);
        self.leave_function();
    }

    fn visit_arrow_function_expression(&mut self, it: &oxc_ast::ast::ArrowFunctionExpression<'a>) {
        self.enter_function();
        walk::walk_arrow_function_expression(self, it);
        self.leave_function();
    }

    fn visit_if_statement(&mut self, it: &IfStatement<'a>) {
        self.bump();
        walk::walk_if_statement(self, it);
    }

    fn visit_conditional_expression(&mut self, it: &ConditionalExpression<'a>) {
        self.bump();
        walk::walk_conditional_expression(self, it);
    }

    fn visit_logical_expression(&mut self, it: &LogicalExpression<'a>) {
        match it.operator {
            LogicalOperator::And | LogicalOperator::Or | LogicalOperator::Coalesce => self.bump(),
        }
        walk::walk_logical_expression(self, it);
    }

    fn visit_switch_case(&mut self, it: &SwitchCase<'a>) {
        if it.test.is_some() {
            self.bump();
        }
        walk::walk_switch_case(self, it);
    }

    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        self.bump();
        walk::walk_for_statement(self, it);
    }

    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        self.bump();
        walk::walk_for_in_statement(self, it);
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        self.bump();
        walk::walk_for_of_statement(self, it);
    }

    fn visit_while_statement(&mut self, it: &WhileStatement<'a>) {
        self.bump();
        walk::walk_while_statement(self, it);
    }

    fn visit_do_while_statement(&mut self, it: &DoWhileStatement<'a>) {
        self.bump();
        walk::walk_do_while_statement(self, it);
    }

    fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
        self.bump();
        walk::walk_catch_clause(self, it);
    }
}

#[cfg(test)]
mod tests;
