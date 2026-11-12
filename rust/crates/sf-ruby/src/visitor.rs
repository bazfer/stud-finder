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

    /// Handle a `CallNode` that may be a Rails association DSL or carry a
    /// literal `class_name:` option. Both are dispatched here so that a
    /// `class_name:` override on an association call SUPPRESSES the
    /// inferred association target: `belongs_to :author, class_name: "User"`
    /// must emit only `User`, not `Author` and `User`.
    fn handle_call(&mut self, call: &CallNode<'_>) {
        let method = call.name().as_slice();
        let line = line_of(call.location().start_line());

        // Receiverless guard: association + class_name DSLs are only valid
        // at the top of a class body, with no receiver. `some_obj.has_many(:xs)`
        // is a user call on an unrelated object — not a Rails edge.
        let receiver = call.receiver();
        let receiverless = receiver.as_ref().is_none_or(|r| r.as_self_node().is_some());

        let association_plural = match method {
            b"has_many" | b"has_and_belongs_to_many" => Some(true),
            b"belongs_to" | b"has_one" => Some(false),
            _ => None,
        };

        if association_plural.is_none() && !source_has_class_name_arg(call) {
            return;
        }

        if !receiverless {
            return;
        }

        let class_name_literal = extract_class_name_option(call);

        if let Some(plural) = association_plural {
            if let Some(literal) = class_name_literal {
                self.push(literal, RefKind::ClassNameOption, line);
            } else {
                self.emit_association_target(call, plural, line);
            }
            return;
        }

        if let Some(literal) = class_name_literal {
            self.push(literal, RefKind::ClassNameOption, line);
        }
    }

    fn emit_association_target(&mut self, call: &CallNode<'_>, plural: bool, line: u32) {
        let Some(args_node) = call.arguments() else {
            return;
        };
        let Some(first) = args_node.arguments().iter().next() else {
            return;
        };
        if let Some(sym) = first.as_symbol_node() {
            if let Ok(value) = std::str::from_utf8(sym.unescaped()) {
                let class = associate_to_class(value, plural);
                self.push(class, RefKind::RailsAssociation, line);
            }
        } else if let Some(s) = first.as_string_node() {
            if let Ok(value) = std::str::from_utf8(s.unescaped()) {
                let class = associate_to_class(value, plural);
                self.push(class, RefKind::RailsAssociation, line);
            }
        }
    }
}

/// Find a literal `class_name: "..."` option on a call, if any.
fn extract_class_name_option(call: &CallNode<'_>) -> Option<String> {
    let args_node = call.arguments()?;
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
                    let normalized = literal.strip_prefix("::").unwrap_or(literal).to_string();
                    if !normalized.is_empty() {
                        return Some(normalized);
                    }
                }
            }
        }
    }
    None
}

/// Cheap pre-check: does any argument of this call hold a `class_name`
/// key? Used to skip calls that aren't associations but also don't carry
/// a class_name option worth inspecting.
fn source_has_class_name_arg(call: &CallNode<'_>) -> bool {
    let Some(args_node) = call.arguments() else {
        return false;
    };
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
            if std::str::from_utf8(sym.unescaped()) == Ok("class_name") {
                return true;
            }
        }
    }
    false
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
        self.handle_call(node);
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
