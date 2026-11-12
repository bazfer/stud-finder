//! Cyclomatic complexity for Ruby source via ruby-prism.
//!
//! Approximates RuboCop's `Metrics/CyclomaticComplexity`: each method
//! starts at 1 and gains +1 per branching node in its body. The file's
//! complexity is the MAX across all top-level `def` nodes (methods
//! nested inside another `def` or inside a `define_method` block
//! currently count toward their enclosing outer method, matching
//! ripple's choice rather than RuboCop's "every def is its own method"
//! distinction — a refinement for a later slice).
//!
//! The decision set this slice counts:
//!
//! - `if` (every `IfNode` — the outermost `if`, every `elsif` (which
//!   prism models as a nested `IfNode` in the else branch), modifier
//!   form, and the ternary `?:`)
//! - `unless` (incl. modifier)
//! - `while`, `until` (incl. modifier form)
//! - `for`
//! - each `when` clause of a `case`
//! - each `in` clause of a `case ... in` pattern-match
//! - a `begin`/`def` body carrying `rescue` clauses counts ONCE,
//!   however many rescue clauses it holds — matching the RuboCop rule
//! - `&&`, `||`, `and`, `or`
//!
//! Not counted this slice (future slices may extend): `||=` / `&&=`,
//! safe-navigation `&.`, blocks passed to iterating methods
//! (`each`/`map`/`select`/…), guard conditions on pattern-match arms,
//! modifier `rescue` outside `begin`, and anything outside a top-level
//! `def`. See the `rust/ATTRIBUTION.md` discussion of the rule set
//! borrowed in shape from ripple's `crates/ripple-core/src/metrics.rs`.
//!
//! The decision set, the rescue-counts-once rule, and the "method is
//! 1 + decisions" convention follow ripple's documented semantics
//! (ripple-core `metrics` module at commit 6604d76). Only the approach
//! is adapted — this file is a from-scratch walker over the subset of
//! RuboCop rules the first slice covers.

use ruby_prism::{
    AndNode, BeginNode, CaseMatchNode, CaseNode, DefNode, ForNode, IfNode, InNode, OrNode,
    UnlessNode, UntilNode, Visit, WhenNode, WhileNode,
};

/// Max cyclomatic complexity across the `def` nodes in `source`. Zero
/// when the file has no method definitions.
pub fn max_complexity(source: &str) -> usize {
    let parsed = ruby_prism::parse(source.as_bytes());
    let root = parsed.node();
    let mut collector = MethodCollector { max: 0 };
    collector.visit(&root);
    collector.max
}

/// Walks the tree looking for `def` nodes. For each one, scores its
/// body with a fresh `DecisionCounter`. Does NOT recurse into inner
/// `def` bodies — nested defs are counted as part of their enclosing
/// def's complexity, matching ripple's documented choice.
struct MethodCollector {
    max: usize,
}

impl<'pr> Visit<'pr> for MethodCollector {
    fn visit_def_node(&mut self, node: &DefNode<'pr>) {
        let mut counter = DecisionCounter { decisions: 0 };
        if let Some(body) = node.body() {
            counter.visit(&body);
        }
        let method_complexity = 1 + counter.decisions;
        if method_complexity > self.max {
            self.max = method_complexity;
        }
        // Don't recurse into this def — any nested defs inside it count
        // toward THIS def's complexity (see DecisionCounter), not as
        // their own method, and we don't want MethodCollector to score
        // them as separate top-level defs.
    }
}

/// Counts branching decisions inside one method body. Visits inner
/// `def` nodes too so their branches count toward the enclosing method.
struct DecisionCounter {
    decisions: usize,
}

impl DecisionCounter {
    fn plus_one(&mut self) {
        self.decisions += 1;
    }
}

impl<'pr> Visit<'pr> for DecisionCounter {
    fn visit_if_node(&mut self, node: &IfNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_if_node(self, node);
    }

    fn visit_unless_node(&mut self, node: &UnlessNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_unless_node(self, node);
    }

    fn visit_while_node(&mut self, node: &WhileNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_while_node(self, node);
    }

    fn visit_until_node(&mut self, node: &UntilNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_until_node(self, node);
    }

    fn visit_for_node(&mut self, node: &ForNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_for_node(self, node);
    }

    fn visit_when_node(&mut self, node: &WhenNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_when_node(self, node);
    }

    fn visit_in_node(&mut self, node: &InNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_in_node(self, node);
    }

    fn visit_and_node(&mut self, node: &AndNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_and_node(self, node);
    }

    fn visit_or_node(&mut self, node: &OrNode<'pr>) {
        self.plus_one();
        ruby_prism::visit_or_node(self, node);
    }

    fn visit_case_node(&mut self, node: &CaseNode<'pr>) {
        // Descend — `case` itself doesn't count, but its `when`
        // clauses do (via visit_when_node above).
        ruby_prism::visit_case_node(self, node);
    }

    fn visit_case_match_node(&mut self, node: &CaseMatchNode<'pr>) {
        // Same for `case ... in` — the `in` clauses count, not the
        // case header.
        ruby_prism::visit_case_match_node(self, node);
    }

    fn visit_begin_node(&mut self, node: &BeginNode<'pr>) {
        // A begin body with any rescue clauses counts ONCE, no matter
        // how many rescue arms it carries, per the RuboCop rule.
        if node.rescue_clause().is_some() {
            self.plus_one();
        }
        ruby_prism::visit_begin_node(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_has_zero_complexity() {
        assert_eq!(max_complexity(""), 0);
    }

    #[test]
    fn file_without_methods_has_zero_complexity() {
        let src = r#"
            x = 1
            y = x + 2
            puts y
        "#;
        assert_eq!(max_complexity(src), 0);
    }

    #[test]
    fn linear_method_has_complexity_one() {
        let src = r#"
            def plain
              x = 1
              y = x + 1
              y
            end
        "#;
        assert_eq!(max_complexity(src), 1);
    }

    #[test]
    fn if_adds_one() {
        let src = r#"
            def with_if(x)
              if x > 0
                :positive
              end
            end
        "#;
        assert_eq!(max_complexity(src), 2);
    }

    #[test]
    fn if_elsif_adds_two() {
        let src = r#"
            def tri(x)
              if x > 0
                :pos
              elsif x < 0
                :neg
              else
                :zero
              end
            end
        "#;
        assert_eq!(max_complexity(src), 3);
    }

    #[test]
    fn ternary_adds_one() {
        let src = r#"
            def t(x)
              x > 0 ? :pos : :neg
            end
        "#;
        assert_eq!(max_complexity(src), 2);
    }

    #[test]
    fn unless_and_modifier_forms_add_one() {
        let src = r#"
            def guarded(x)
              return :nope unless x
              x
            end
        "#;
        assert_eq!(max_complexity(src), 2);
    }

    #[test]
    fn while_until_for_each_add_one() {
        let src = r#"
            def loops(xs)
              while xs.any?
                xs.shift
              end
              until xs.empty?
                xs.pop
              end
              for x in xs
                puts x
              end
            end
        "#;
        assert_eq!(max_complexity(src), 1 + 3);
    }

    #[test]
    fn case_when_each_when_adds_one() {
        let src = r#"
            def label(x)
              case x
              when 1 then :one
              when 2 then :two
              when 3 then :three
              else :other
              end
            end
        "#;
        // 1 base + 3 when clauses (else and case header don't count).
        assert_eq!(max_complexity(src), 4);
    }

    #[test]
    fn logical_operators_add_one_each() {
        let src = r#"
            def guard(a, b, c)
              return :bad unless a && b
              return :also_bad if b || c
              a and b or c
            end
        "#;
        // 1 base
        //  + 1 for `unless`
        //  + 1 for `&&`
        //  + 1 for `if` (modifier form)
        //  + 1 for `||`
        //  + 1 for `and`
        //  + 1 for `or`
        assert_eq!(max_complexity(src), 7);
    }

    #[test]
    fn rescue_clauses_count_once_per_body() {
        let src = r#"
            def try_it
              begin
                dangerous
              rescue ArgumentError
                :arg
              rescue TypeError
                :typ
              rescue => e
                :fallback
              end
            end
        "#;
        // 1 base + 1 for the begin-with-rescue (regardless of clause count).
        assert_eq!(max_complexity(src), 2);
    }

    #[test]
    fn max_across_methods_is_returned() {
        let src = r#"
            def tiny
              1
            end

            def medium(x)
              if x then :a else :b end
            end

            def big(xs)
              xs.each do |x|
                if x > 0
                  puts :pos
                elsif x < 0
                  puts :neg
                end
              end
              return :empty if xs.empty?
            end
        "#;
        // tiny = 1, medium = 2, big = 1 + 2 (if/elsif) + 1 (modifier if) = 4.
        // Note: the `.each do |x|` block body's `if`/`elsif` count
        // toward `big` because the block is INSIDE big's body —
        // DecisionCounter walks through blocks.
        let got = max_complexity(src);
        assert_eq!(got, 4, "expected big method to dominate; got {got}");
    }
}
