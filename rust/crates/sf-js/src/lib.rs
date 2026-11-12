//! JS/TS static analysis for stud-finder: module-graph fan_in via
//! import/require resolution, and per-function cyclomatic complexity.
//! Uses `oxc` for parsing (JS, TS, JSX, TSX).
//!
//! Scope vs Ruby parity:
//!
//! stud-finder's Ruby `JsFanIn` + `JsComplexity` shell out to
//! `dependency-cruiser` and `eslint` respectively — both require
//! node + npm in the ambient environment. This crate is a deliberate
//! upgrade to a Rust-native parser (oxc) so the port runs without
//! node and in the same cold-start budget as the rest of sf-cli.
//! `rust/ATTRIBUTION.md` carries the per-file provenance note.
//!
//! The Ruby behavior we match in intent:
//!
//! - `extract_refs(path, source)` → relative module specifiers only.
//!   Bare packages (`react`, `@scope/x`) are dropped (Ruby filters
//!   depcruise output to the file_set which excludes packages).
//! - Import shapes recognised: `import ... from '...'`,
//!   `import('...')`, `require('...')`, `export ... from '...'`,
//!   `export * from '...'`.
//! - Relative resolution: specifier joined with the importing
//!   file's directory, `.`/`..` segments resolved. Extension
//!   inference tries `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`
//!   when the specifier has no extension. Index resolution on a
//!   directory specifier is left to the caller (the CLI resolves
//!   against its walkdir set).
//! - `max_complexity(source)` → max cyclomatic complexity across
//!   all function bodies in the file. Decision set: `if`/`else if`
//!   (via nested IfStatement), ternary, `||`, `&&`, `??`,
//!   `case` clauses, `for` / `for-in` / `for-of`, `while` /
//!   `do-while`, `catch`. Each function starts at 1 and gains +1
//!   per decision found in its body, not descending into inner
//!   functions (their complexity is computed independently).

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, BlockStatement, CatchClause, ConditionalExpression, DoWhileStatement,
    Expression, ForInStatement, ForOfStatement, ForStatement, FunctionBody, IfStatement,
    LogicalExpression, Program, Statement, SwitchCase, SwitchStatement, TryStatement,
    WhileStatement,
};
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::operator::LogicalOperator;
use std::path::{Component, Path, PathBuf};

/// Per-file reference result. `target` is a relative path derived
/// from the caller's `path` + the import specifier. Extension
/// inference is attempted (`./foo` → `./foo.ts`/`./foo.tsx`/...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsRef {
    pub target: PathBuf,
}

/// Complexity report mirrors sf-ruby's shape so the CLI can use
/// the same shape for both languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComplexityReport {
    pub complexity: usize,
    pub parsed_cleanly: bool,
}

/// Extract resolved relative module references from `source`. `path`
/// is the importing file's repo-root-relative path and is used to
/// anchor relative specifiers. Bare package specifiers are dropped.
pub fn extract_refs(path: &Path, source: &str) -> Vec<JsRef> {
    let allocator = Allocator::default();
    let Some(source_type) = source_type_for(path) else {
        return Vec::new();
    };
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked {
        return Vec::new();
    }
    let mut specifiers: Vec<String> = Vec::new();
    collect_module_specifiers(&parsed.program, &mut specifiers);
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(""));
    specifiers
        .into_iter()
        .filter(|s| is_relative_specifier(s))
        .map(|s| JsRef {
            target: resolve_relative(&base_dir, &s),
        })
        .collect()
}

/// Max cyclomatic complexity across the file's functions. Files with
/// no function bodies score 0.
pub fn max_complexity(source: &str) -> ComplexityReport {
    let allocator = Allocator::default();
    // TSX source type handles JS, JSX, TS and TSX for the parse pass
    // — oxc's parser is permissive. For complexity-only the type
    // doesn't change the AST shape of the constructs we count.
    let source_type = SourceType::tsx();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let parsed_cleanly = parsed.errors.is_empty();
    if parsed.panicked {
        return ComplexityReport {
            complexity: 0,
            parsed_cleanly: false,
        };
    }
    let mut bodies: Vec<&FunctionBody> = Vec::new();
    collect_function_bodies_program(&parsed.program, &mut bodies);
    let mut max = 0usize;
    for body in bodies {
        let c = 1 + count_decisions_in_body(body);
        if c > max {
            max = c;
        }
    }
    ComplexityReport {
        complexity: max,
        parsed_cleanly,
    }
}

fn source_type_for(path: &Path) -> Option<SourceType> {
    let ext = path.extension().and_then(|s| s.to_str())?;
    match ext {
        "ts" => Some(SourceType::ts()),
        "tsx" => Some(SourceType::tsx()),
        "jsx" | "js" | "mjs" | "cjs" => Some(SourceType::jsx()),
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

// --- module-graph extraction ---

fn collect_module_specifiers<'a>(program: &Program<'a>, out: &mut Vec<String>) {
    for stmt in &program.body {
        collect_module_specifiers_from_stmt(stmt, out);
    }
}

fn collect_module_specifiers_from_stmt<'a>(stmt: &Statement<'a>, out: &mut Vec<String>) {
    match stmt {
        Statement::ImportDeclaration(decl) => {
            out.push(decl.source.value.to_string());
        }
        Statement::ExportNamedDeclaration(decl) => {
            if let Some(src) = &decl.source {
                out.push(src.value.to_string());
            }
            if let Some(oxc_ast::ast::Declaration::VariableDeclaration(v)) = &decl.declaration {
                for dec in &v.declarations {
                    if let Some(init) = &dec.init {
                        collect_module_specifiers_from_expr(init, out);
                    }
                }
            }
        }
        Statement::ExportAllDeclaration(decl) => {
            out.push(decl.source.value.to_string());
        }
        Statement::ExpressionStatement(stmt) => {
            collect_module_specifiers_from_expr(&stmt.expression, out);
        }
        Statement::BlockStatement(block) => {
            for inner in &block.body {
                collect_module_specifiers_from_stmt(inner, out);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    collect_module_specifiers_from_expr(init, out);
                }
            }
        }
        Statement::IfStatement(s) => {
            collect_module_specifiers_from_expr(&s.test, out);
            collect_module_specifiers_from_stmt(&s.consequent, out);
            if let Some(alt) = &s.alternate {
                collect_module_specifiers_from_stmt(alt, out);
            }
        }
        Statement::TryStatement(s) => {
            for inner in &s.block.body {
                collect_module_specifiers_from_stmt(inner, out);
            }
            if let Some(handler) = &s.handler {
                for inner in &handler.body.body {
                    collect_module_specifiers_from_stmt(inner, out);
                }
            }
            if let Some(finalizer) = &s.finalizer {
                for inner in &finalizer.body {
                    collect_module_specifiers_from_stmt(inner, out);
                }
            }
        }
        Statement::ReturnStatement(s) => {
            if let Some(arg) = &s.argument {
                collect_module_specifiers_from_expr(arg, out);
            }
        }
        _ => {}
    }
}

fn collect_module_specifiers_from_expr<'a>(expr: &Expression<'a>, out: &mut Vec<String>) {
    match expr {
        Expression::CallExpression(call) => {
            let is_require = matches!(
                &call.callee,
                Expression::Identifier(id) if id.name == "require"
            );
            if is_require && call.arguments.len() == 1 {
                if let Some(arg) = call.arguments.first() {
                    if let Some(lit) = expr_as_string_literal(arg.as_expression()) {
                        out.push(lit);
                    }
                }
            }
            // Descend into args to catch `fn(require('./x'))` and
            // similar nested patterns.
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_module_specifiers_from_expr(e, out);
                }
            }
        }
        Expression::ImportExpression(imp) => {
            if let Some(lit) = expr_as_string_literal(Some(&imp.source)) {
                out.push(lit);
            }
        }
        Expression::AwaitExpression(a) => {
            collect_module_specifiers_from_expr(&a.argument, out);
        }
        Expression::AssignmentExpression(a) => {
            collect_module_specifiers_from_expr(&a.right, out);
        }
        Expression::ParenthesizedExpression(p) => {
            collect_module_specifiers_from_expr(&p.expression, out);
        }
        Expression::ConditionalExpression(c) => {
            collect_module_specifiers_from_expr(&c.test, out);
            collect_module_specifiers_from_expr(&c.consequent, out);
            collect_module_specifiers_from_expr(&c.alternate, out);
        }
        Expression::LogicalExpression(e) => {
            collect_module_specifiers_from_expr(&e.left, out);
            collect_module_specifiers_from_expr(&e.right, out);
        }
        _ => {}
    }
}

fn expr_as_string_literal<'a>(expr: Option<&Expression<'a>>) -> Option<String> {
    match expr? {
        Expression::StringLiteral(lit) => Some(lit.value.to_string()),
        _ => None,
    }
}

// --- complexity ---

fn collect_function_bodies_program<'a, 'b>(
    program: &'b Program<'a>,
    out: &mut Vec<&'b FunctionBody<'a>>,
) {
    for stmt in &program.body {
        collect_function_bodies_stmt(stmt, out);
    }
}

fn collect_function_bodies_stmt<'a, 'b>(
    stmt: &'b Statement<'a>,
    out: &mut Vec<&'b FunctionBody<'a>>,
) {
    match stmt {
        Statement::FunctionDeclaration(f) => {
            if let Some(body) = &f.body {
                out.push(body);
                for inner in &body.statements {
                    collect_function_bodies_stmt(inner, out);
                }
            }
        }
        Statement::BlockStatement(block) => {
            for inner in &block.body {
                collect_function_bodies_stmt(inner, out);
            }
        }
        Statement::IfStatement(s) => {
            collect_function_bodies_stmt(&s.consequent, out);
            if let Some(alt) = &s.alternate {
                collect_function_bodies_stmt(alt, out);
            }
            collect_function_bodies_expr(&s.test, out);
        }
        Statement::ForStatement(s) => {
            if let Some(update) = &s.update {
                collect_function_bodies_expr(update, out);
            }
            collect_function_bodies_stmt(&s.body, out);
        }
        Statement::ForInStatement(s) => {
            collect_function_bodies_stmt(&s.body, out);
        }
        Statement::ForOfStatement(s) => {
            collect_function_bodies_stmt(&s.body, out);
        }
        Statement::WhileStatement(s) => {
            collect_function_bodies_expr(&s.test, out);
            collect_function_bodies_stmt(&s.body, out);
        }
        Statement::DoWhileStatement(s) => {
            collect_function_bodies_stmt(&s.body, out);
            collect_function_bodies_expr(&s.test, out);
        }
        Statement::SwitchStatement(s) => {
            collect_function_bodies_expr(&s.discriminant, out);
            for case in &s.cases {
                for inner in &case.consequent {
                    collect_function_bodies_stmt(inner, out);
                }
            }
        }
        Statement::TryStatement(s) => {
            for inner in &s.block.body {
                collect_function_bodies_stmt(inner, out);
            }
            if let Some(handler) = &s.handler {
                for inner in &handler.body.body {
                    collect_function_bodies_stmt(inner, out);
                }
            }
            if let Some(finalizer) = &s.finalizer {
                for inner in &finalizer.body {
                    collect_function_bodies_stmt(inner, out);
                }
            }
        }
        Statement::ExpressionStatement(s) => {
            collect_function_bodies_expr(&s.expression, out);
        }
        Statement::ReturnStatement(s) => {
            if let Some(arg) = &s.argument {
                collect_function_bodies_expr(arg, out);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    collect_function_bodies_expr(init, out);
                }
            }
        }
        Statement::ExportDefaultDeclaration(d) => {
            if let Some(expr) = d.declaration.as_expression() {
                collect_function_bodies_expr(expr, out);
            }
        }
        _ => {}
    }
}

fn collect_function_bodies_expr<'a, 'b>(
    expr: &'b Expression<'a>,
    out: &mut Vec<&'b FunctionBody<'a>>,
) {
    match expr {
        Expression::FunctionExpression(f) => {
            if let Some(body) = &f.body {
                out.push(body);
                for inner in &body.statements {
                    collect_function_bodies_stmt(inner, out);
                }
            }
        }
        Expression::ArrowFunctionExpression(f) => {
            out.push(&f.body);
            for inner in &f.body.statements {
                collect_function_bodies_stmt(inner, out);
            }
        }
        Expression::CallExpression(call) => {
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_function_bodies_expr(e, out);
                }
            }
        }
        Expression::ConditionalExpression(c) => {
            collect_function_bodies_expr(&c.test, out);
            collect_function_bodies_expr(&c.consequent, out);
            collect_function_bodies_expr(&c.alternate, out);
        }
        Expression::LogicalExpression(e) => {
            collect_function_bodies_expr(&e.left, out);
            collect_function_bodies_expr(&e.right, out);
        }
        Expression::ParenthesizedExpression(p) => {
            collect_function_bodies_expr(&p.expression, out);
        }
        Expression::AssignmentExpression(a) => {
            collect_function_bodies_expr(&a.right, out);
        }
        _ => {}
    }
}

/// Count branching decisions inside one function body. Does NOT
/// descend into nested function bodies — they contribute to their
/// own complexity entry in the max.
fn count_decisions_in_body<'a>(body: &FunctionBody<'a>) -> usize {
    let mut count = 0usize;
    for stmt in &body.statements {
        count_decisions_stmt(stmt, &mut count);
    }
    count
}

fn count_decisions_stmt<'a>(stmt: &Statement<'a>, count: &mut usize) {
    match stmt {
        Statement::IfStatement(s) => {
            *count += 1;
            count_decisions_if(s, count);
        }
        Statement::ForStatement(s) => {
            *count += 1;
            count_decisions_for(s, count);
        }
        Statement::ForInStatement(s) => {
            *count += 1;
            count_decisions_for_in(s, count);
        }
        Statement::ForOfStatement(s) => {
            *count += 1;
            count_decisions_for_of(s, count);
        }
        Statement::WhileStatement(s) => {
            *count += 1;
            count_decisions_while(s, count);
        }
        Statement::DoWhileStatement(s) => {
            *count += 1;
            count_decisions_do_while(s, count);
        }
        Statement::SwitchStatement(s) => count_decisions_switch(s, count),
        Statement::TryStatement(s) => count_decisions_try(s, count),
        Statement::BlockStatement(b) => count_decisions_block(b, count),
        Statement::ExpressionStatement(s) => count_decisions_expr(&s.expression, count),
        Statement::ReturnStatement(s) => {
            if let Some(arg) = &s.argument {
                count_decisions_expr(arg, count);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    count_decisions_expr(init, count);
                }
            }
        }
        // Nested functions: NOT descended into. Their own complexity
        // is computed via the separate collect_function_bodies pass.
        Statement::FunctionDeclaration(_) => {}
        _ => {}
    }
}

fn count_decisions_if<'a>(s: &IfStatement<'a>, count: &mut usize) {
    count_decisions_expr(&s.test, count);
    count_decisions_stmt(&s.consequent, count);
    if let Some(alt) = &s.alternate {
        // `else if` is modeled as nested IfStatement in the alternate
        // — the recursive count_decisions_stmt picks it up with
        // another +1. Plain `else` is a block and adds nothing.
        count_decisions_stmt(alt, count);
    }
}

fn count_decisions_for<'a>(s: &ForStatement<'a>, count: &mut usize) {
    if let Some(test) = &s.test {
        count_decisions_expr(test, count);
    }
    if let Some(update) = &s.update {
        count_decisions_expr(update, count);
    }
    count_decisions_stmt(&s.body, count);
}

fn count_decisions_for_in<'a>(s: &ForInStatement<'a>, count: &mut usize) {
    count_decisions_expr(&s.right, count);
    count_decisions_stmt(&s.body, count);
}

fn count_decisions_for_of<'a>(s: &ForOfStatement<'a>, count: &mut usize) {
    count_decisions_expr(&s.right, count);
    count_decisions_stmt(&s.body, count);
}

fn count_decisions_while<'a>(s: &WhileStatement<'a>, count: &mut usize) {
    count_decisions_expr(&s.test, count);
    count_decisions_stmt(&s.body, count);
}

fn count_decisions_do_while<'a>(s: &DoWhileStatement<'a>, count: &mut usize) {
    count_decisions_expr(&s.test, count);
    count_decisions_stmt(&s.body, count);
}

fn count_decisions_switch<'a>(s: &SwitchStatement<'a>, count: &mut usize) {
    count_decisions_expr(&s.discriminant, count);
    for case in &s.cases {
        count_decisions_case(case, count);
    }
}

fn count_decisions_case<'a>(case: &SwitchCase<'a>, count: &mut usize) {
    // `case X:` counts +1. `default:` (test is None) adds nothing.
    if case.test.is_some() {
        *count += 1;
        if let Some(t) = &case.test {
            count_decisions_expr(t, count);
        }
    }
    for s in &case.consequent {
        count_decisions_stmt(s, count);
    }
}

fn count_decisions_try<'a>(s: &TryStatement<'a>, count: &mut usize) {
    for inner in &s.block.body {
        count_decisions_stmt(inner, count);
    }
    if let Some(handler) = &s.handler {
        count_decisions_catch(handler, count);
    }
    if let Some(finalizer) = &s.finalizer {
        for inner in &finalizer.body {
            count_decisions_stmt(inner, count);
        }
    }
}

fn count_decisions_catch<'a>(handler: &CatchClause<'a>, count: &mut usize) {
    *count += 1;
    for inner in &handler.body.body {
        count_decisions_stmt(inner, count);
    }
}

fn count_decisions_block<'a>(block: &BlockStatement<'a>, count: &mut usize) {
    for inner in &block.body {
        count_decisions_stmt(inner, count);
    }
}

fn count_decisions_expr<'a>(expr: &Expression<'a>, count: &mut usize) {
    match expr {
        Expression::LogicalExpression(e) => count_decisions_logical(e, count),
        Expression::ConditionalExpression(c) => {
            *count += 1;
            count_decisions_conditional(c, count);
        }
        Expression::CallExpression(call) => {
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    count_decisions_expr(e, count);
                }
            }
        }
        Expression::ParenthesizedExpression(p) => count_decisions_expr(&p.expression, count),
        Expression::AssignmentExpression(a) => count_decisions_expr(&a.right, count),
        // Nested function expressions NOT descended into — their own
        // complexity is computed separately.
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) => {}
        _ => {}
    }
}

fn count_decisions_logical<'a>(e: &LogicalExpression<'a>, count: &mut usize) {
    // `||`, `&&`, `??` each count +1 per operator (eslint default).
    let op_count = match e.operator {
        LogicalOperator::And | LogicalOperator::Or | LogicalOperator::Coalesce => 1,
    };
    *count += op_count;
    count_decisions_expr(&e.left, count);
    count_decisions_expr(&e.right, count);
}

fn count_decisions_conditional<'a>(c: &ConditionalExpression<'a>, count: &mut usize) {
    count_decisions_expr(&c.test, count);
    count_decisions_expr(&c.consequent, count);
    count_decisions_expr(&c.alternate, count);
}

// Helper used by the test module for arrow-function-body dispatch.
#[allow(dead_code)]
fn arrow_body_ref<'a, 'b>(arrow: &'b ArrowFunctionExpression<'a>) -> &'b FunctionBody<'a> {
    &arrow.body
}

#[cfg(test)]
mod tests;
