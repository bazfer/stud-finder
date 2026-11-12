use super::*;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

// --- extract_refs ---

#[test]
fn static_import_is_resolved_against_importer() {
    let src = "import { x } from './util';\n";
    let refs = extract_refs(&p("src/app/index.ts"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("src/app/util"),
        }]
    );
}

#[test]
fn relative_parent_specifier_traverses_up() {
    let src = "import { x } from '../util/x';\n";
    let refs = extract_refs(&p("src/app/index.ts"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("src/util/x"),
        }]
    );
}

#[test]
fn dynamic_import_is_recognised() {
    let src = "const m = await import('./lazy');\n";
    let refs = extract_refs(&p("src/app/index.ts"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("src/app/lazy"),
        }]
    );
}

#[test]
fn require_call_is_recognised() {
    let src = "const m = require('./cjs/mod');\n";
    let refs = extract_refs(&p("lib/entry.js"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("lib/cjs/mod"),
        }]
    );
}

#[test]
fn re_export_from_is_recognised() {
    let src = "export { x } from './util';\n";
    let refs = extract_refs(&p("src/app/index.ts"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("src/app/util"),
        }]
    );
}

#[test]
fn export_star_from_is_recognised() {
    let src = "export * from './util';\n";
    let refs = extract_refs(&p("src/app/index.ts"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("src/app/util"),
        }]
    );
}

#[test]
fn bare_package_specifiers_are_dropped() {
    let src = r#"
        import React from 'react';
        import { Button } from '@mui/material';
        import local from './local';
    "#;
    let refs = extract_refs(&p("src/app/index.tsx"), src);
    assert_eq!(
        refs,
        vec![JsRef {
            target: p("src/app/local"),
        }],
        "only the relative import should survive"
    );
}

#[test]
fn malformed_source_returns_empty_without_panic() {
    // Deliberately broken TSX. Must not panic.
    let src = "const x = <div><span>; !!!";
    let _ = extract_refs(&p("broken.tsx"), src);
    // No assertion on content — just absence of panic is the contract.
}

#[test]
fn unsupported_extension_returns_empty() {
    let src = "import x from './y';";
    let refs = extract_refs(&p("some.txt"), src);
    assert!(refs.is_empty());
}

// --- max_complexity ---

fn c(source: &str) -> usize {
    max_complexity(source).complexity
}

#[test]
fn empty_source_scores_zero() {
    let r = max_complexity("");
    assert_eq!(r.complexity, 0);
    assert!(r.parsed_cleanly);
}

#[test]
fn file_without_functions_scores_zero() {
    assert_eq!(c("const x = 1; const y = x + 1;"), 0);
}

#[test]
fn linear_function_has_complexity_one() {
    let src = r#"
        function plain() {
          const x = 1;
          const y = x + 1;
          return y;
        }
    "#;
    assert_eq!(c(src), 1);
}

#[test]
fn if_adds_one() {
    let src = r#"
        function f(x) {
          if (x > 0) return x;
          return 0;
        }
    "#;
    assert_eq!(c(src), 2);
}

#[test]
fn if_else_if_adds_two() {
    let src = r#"
        function f(x) {
          if (x > 0) return 1;
          else if (x < 0) return -1;
          else return 0;
        }
    "#;
    assert_eq!(c(src), 3);
}

#[test]
fn ternary_adds_one() {
    let src = r#"
        function f(x) { return x > 0 ? 1 : -1; }
    "#;
    assert_eq!(c(src), 2);
}

#[test]
fn logical_operators_count_one_each() {
    let src = r#"
        function f(a, b, c) {
          return a && b || c;
        }
    "#;
    // 1 base + 1 (&&) + 1 (||)
    assert_eq!(c(src), 3);
}

#[test]
fn nullish_coalesce_counts_one() {
    let src = r#"
        function f(x) { return x ?? "fallback"; }
    "#;
    assert_eq!(c(src), 2);
}

#[test]
fn for_variants_each_count_one() {
    let src = r#"
        function f(xs) {
          for (let i = 0; i < 10; i++) {}
          for (const x of xs) {}
          for (const k in xs) {}
        }
    "#;
    // 1 base + 3 loops
    assert_eq!(c(src), 4);
}

#[test]
fn while_and_do_while_each_count_one() {
    let src = r#"
        function f() {
          while (true) {}
          do {} while (false);
        }
    "#;
    assert_eq!(c(src), 3);
}

#[test]
fn switch_case_counts_per_case() {
    let src = r#"
        function f(x) {
          switch (x) {
            case 1: return 'a';
            case 2: return 'b';
            case 3: return 'c';
            default: return 'd';
          }
        }
    "#;
    // 1 base + 3 case clauses (default doesn't count).
    assert_eq!(c(src), 4);
}

#[test]
fn catch_counts_one() {
    let src = r#"
        function f() {
          try { return 1; } catch (e) { return 0; }
        }
    "#;
    assert_eq!(c(src), 2);
}

#[test]
fn max_across_functions_dominates() {
    let src = r#"
        function tiny() { return 1; }
        function big(x) {
          if (x > 0) {
            if (x > 10) return 'big';
            return 'small';
          }
          return 'zero';
        }
    "#;
    // tiny = 1, big = 1 + 2 ifs = 3.
    assert_eq!(c(src), 3);
}

#[test]
fn arrow_functions_are_counted() {
    let src = r#"
        const f = (x) => (x > 0 ? 1 : -1);
    "#;
    assert_eq!(c(src), 2);
}

#[test]
fn nested_function_does_not_inflate_outer_complexity() {
    // Inner function has 3 (big) but it counts as its own function,
    // not toward outer. outer has complexity 1.
    let src = r#"
        function outer() {
          function inner(x) {
            if (x > 0 && x < 10) return 1;
            return 0;
          }
          return inner;
        }
    "#;
    // inner: 1 + 1 (if) + 1 (&&) = 3
    // outer: 1
    // max = 3
    assert_eq!(c(src), 3);
}

#[test]
fn parse_error_flips_parsed_cleanly_false() {
    let src = "function f( { ??? !!";
    let r = max_complexity(src);
    assert!(!r.parsed_cleanly, "syntax error must flip the flag");
}

#[test]
fn ts_syntax_parses_cleanly() {
    let src = r#"
        function f<T>(x: T, y?: number): T { return x; }
    "#;
    let r = max_complexity(src);
    assert!(r.parsed_cleanly, "valid TS should parse: {:?}", r);
    assert_eq!(r.complexity, 1);
}

#[test]
fn jsx_syntax_parses_cleanly() {
    let src = r#"
        const App = () => <div>hi</div>;
    "#;
    let r = max_complexity(src);
    assert!(r.parsed_cleanly, "valid JSX should parse: {:?}", r);
}
