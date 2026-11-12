//! ruby-prism AST visitor that pulls out constant references, Rails
//! association DSL calls, and literal `class_name: "..."` options.
//!
//! The DSL-trigger set, the receiverless-call dispatch on the method name
//! as bytes, and the overall shape of walking `CallNode`s while tracking
//! class/module nesting are adapted from Saugat Khadka's ripple-ruby crate
//! (commit 6604d76, `crates/ripple-ruby/src/rails.rs`). The MIT notice
//! preservation record lives in `rust/ATTRIBUTION.md`; the licensed
//! copyright + permission notice for lifted portions is at the top of this
//! file.
//!
//! Lifted portion copyright notice:
//!
//! Copyright (c) 2026 Saugat Khadka
//!
//! Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the
//! "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish,
//! distribute, sublicense, and/or sell copies of the Software, and to
//! permit persons to whom the Software is furnished to do so, subject to
//! the following conditions:
//!
//! The above copyright notice and this permission notice shall be included
//! in all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
//! OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
//! IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
//! TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
//! SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use crate::inflect::associate_to_class;
use crate::{RefKind, RubyRef};
use ruby_prism::{CallNode, ConstantPathNode, ConstantReadNode, Visit};

/// Substrings a Ruby file must contain to be worth parsing for Rails DSL.
/// Files without any of these skip the full prism parse. Lifted from
/// ripple-ruby's `TRIGGERS` constant with the subset relevant to this slice.
const DSL_TRIGGERS: &[&str] = &[
    "belongs_to",
    "has_one",
    "has_many",
    "has_and_belongs_to_many",
    "class_name",
];

pub fn collect(source: &str) -> Vec<RubyRef> {
    let bytes = source.as_bytes();
    let parsed = ruby_prism::parse(bytes);
    let root = parsed.node();
    let mut v = Visitor { refs: Vec::new() };
    v.visit(&root);
    v.refs
}

struct Visitor {
    refs: Vec<RubyRef>,
}

impl Visitor {
    fn push(&mut self, name: String, kind: RefKind, line: u32) {
        if name.is_empty() {
            return;
        }
        self.refs.push(RubyRef { name, kind, line });
    }

    fn handle_association(&mut self, call: &CallNode<'_>) {
        let method = call.name().as_slice();
        let plural = match method {
            b"has_many" | b"has_and_belongs_to_many" => true,
            b"belongs_to" | b"has_one" => false,
            _ => return,
        };
        let line = line_of(call.location().start_line());
        let Some(args_node) = call.arguments() else {
            return;
        };
        let first = args_node.arguments().iter().next();
        let Some(arg) = first else { return };
        if let Some(sym) = arg.as_symbol_node() {
            if let Ok(value) = std::str::from_utf8(sym.unescaped()) {
                let class = associate_to_class(value, plural);
                self.push(class, RefKind::RailsAssociation, line);
            }
        } else if let Some(s) = arg.as_string_node() {
            if let Ok(value) = std::str::from_utf8(s.unescaped()) {
                let class = associate_to_class(value, plural);
                self.push(class, RefKind::RailsAssociation, line);
            }
        }
    }

    fn handle_class_name_option(&mut self, call: &CallNode<'_>) {
        let Some(args_node) = call.arguments() else {
            return;
        };
        let line = line_of(call.location().start_line());
        for arg in args_node.arguments().iter() {
            let Some(hash) = arg.as_keyword_hash_node() else {
                continue;
            };
            for element in hash.elements().iter() {
                let Some(assoc) = element.as_assoc_node() else {
                    continue;
                };
                let key = assoc.key();
                let Some(sym) = key.as_symbol_node() else {
                    continue;
                };
                let Ok(key_text) = std::str::from_utf8(sym.unescaped()) else {
                    continue;
                };
                if key_text != "class_name" {
                    continue;
                }
                let value = assoc.value();
                if let Some(str_node) = value.as_string_node() {
                    if let Ok(literal) = std::str::from_utf8(str_node.unescaped()) {
                        let normalized = literal
                            .strip_prefix("::")
                            .unwrap_or(literal)
                            .to_string();
                        self.push(normalized, RefKind::ClassNameOption, line);
                    }
                }
            }
        }
    }
}

impl<'pr> Visit<'pr> for Visitor {
    fn visit_constant_read_node(&mut self, node: &ConstantReadNode<'pr>) {
        let bytes = node.location().as_slice();
        if let Ok(name) = std::str::from_utf8(bytes) {
            let line = line_of(node.location().start_line());
            self.push(name.to_string(), RefKind::Constant, line);
        }
    }

    fn visit_constant_path_node(&mut self, node: &ConstantPathNode<'pr>) {
        let bytes = node.location().as_slice();
        if let Ok(text) = std::str::from_utf8(bytes) {
            let normalized = text.trim_start_matches("::").to_string();
            if !normalized.is_empty() {
                let line = line_of(node.location().start_line());
                self.push(normalized, RefKind::Constant, line);
            }
        }
        // Don't descend — a nested ConstantReadNode visit would double-emit
        // the leaf name, so we skip ruby_prism::visit_constant_path_node.
    }

    fn visit_call_node(&mut self, node: &CallNode<'pr>) {
        self.handle_association(node);
        self.handle_class_name_option(node);
        ruby_prism::visit_call_node(self, node);
    }
}

fn line_of(start_line: i32) -> u32 {
    u32::try_from(start_line).unwrap_or(0)
}

/// Fast pre-filter: a file with none of the DSL triggers can skip the
/// Rails handling pass. We still parse for constant references because
/// constants appear in essentially every Ruby file.
#[allow(dead_code)]
pub fn has_dsl_trigger(source: &str) -> bool {
    DSL_TRIGGERS.iter().any(|t| source.contains(t))
}
