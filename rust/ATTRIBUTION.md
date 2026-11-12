# Attribution

The Rust port on this branch draws on prior work by Saugat Khadka in the
`ripple` project at <https://github.com/saugatkhadka/stud-finder/tree/ripple>.

When we lift or adapt a file, algorithm, or test fixture from ripple, the
source commit hash and the file path it came from are recorded at the
top of the receiving file. This document is the roll-up index of those
lifts.

Both ripple and stud-finder are MIT-licensed. When we lift code from
ripple, MIT REQUIRES the source's copyright notice and the MIT
permission notice to be preserved in all substantial copies. That is the
actual compliance bar. The provenance table below is extra discipline on
top — it makes lifts auditable as both projects evolve and makes
upstreaming improvements back to ripple straightforward — but the
copyright + permission notice preservation is non-optional.

**Per-file mechanic for each lift:**

1. Top-of-file comment with the source commit hash and source path.
2. Preserved copyright line and MIT permission notice from upstream
   (ripple-rooted, naming Saugat Khadka as copyright holder for lifted
   portions).
3. Whether the file is an adapted port or a verbatim lift.
4. A row added to the **Lifts** table below linking local path ↔ ripple
   source.

**Upstream hygiene note (2026-11-12):** ripple's repository declares
`license = "MIT"` in its workspace `Cargo.toml` but does not ship a
`LICENSE` text file at the repo root. When we lift, we supply the
standard MIT permission-notice body ourselves in the receiving file, with
Saugat Khadka named as copyright holder for that portion. If this
becomes a recurring friction point, ask Saugat to add a `LICENSE` file
to ripple so lifts can reference it directly.

## Lifts

Entries get added here as code is adapted. Each row names the local
path, the ripple source commit and path, and a one-line note on what was
kept or changed.

| Local path | Source (commit : path) | Notes |
|------------|-------------------------------|-------|
| `rust/crates/sf-ruby/src/visitor.rs` | ripple 6604d76 : `crates/ripple-ruby/src/rails.rs` | Adapted — kept the DSL trigger set, the receiverless-call dispatch on the method name as bytes, and the pattern of walking `CallNode` nodes. Reduced scope to this slice's four reference kinds (bare constants, constant paths, Rails association DSL, `class_name:` option) and dropped the controller/action, HTTP-request, `require`/`require_relative`, `constantize`/`const_get`, render/layout, and serializer-model logic. Preserved MIT copyright + permission notice naming Saugat Khadka as copyright holder for the lifted portions, at the top of the receiving file. |
| `rust/crates/sf-score/src/lib.rs` | stud-finder main `11913e4..46d2fe9` : `lib/stud_finder/scorer.rb` | Ported from the Ruby `StudFinder::Scorer`, same repo (bazfer/stud-finder, main branch). Preserved constants (`FAN_IN_FLOOR = 25`, trunk/branch percentile thresholds 85/50), the 4:2:5:5 base-weight ratio (renormalized to 2 signals as 4:2 here), the percentile-rank-of-composite classification shape, and the absolute-floor escalation rule (`Leaf` → `Branch` when `fan_in >= FAN_IN_FLOOR`; never downgrade). No cross-project lift; both ends are bazfer MIT. |
| `rust/crates/sf-ruby/src/complexity.rs` | ripple 6604d76 : `crates/ripple-core/src/metrics.rs` (approach) + stud-finder main : RuboCop rule subset | Written fresh — not a verbatim lift. The decision set (If/Unless/Case-When/Case-In/While/Until/For/Rescue-counts-once/&&/`||`/`and`/`or`), the "method is 1 + decisions" convention, and the "nested defs count toward the enclosing method" choice follow ripple's documented semantics in the metrics module. The walker itself is straightforward ruby-prism visitor code sized for the RuboCop subset we need this slice (no AbcSize, PerceivedComplexity, BlockLength, safe-navigation `&.`, iterating-method blocks, or modifier-rescue). Attribution here is for the rule-set shape, not file content. |
| `rust/crates/sf-score/src/lib.rs` (3-signal extension) | stud-finder main : `lib/stud_finder/scorer.rb` normalize_weights + active_signal_keys | Generalized the composite from a fixed 2-signal weighted sum to an "n available signals, renormalize weights" shape matching Ruby's `Scorer#normalize_weights`. Added `COMPLEXITY_FLOOR = 15` Leaf→Branch escalation matching Ruby's `COMPLEXITY_FLOOR` constant. Existing lift row for sf-score applies. |
| `rust/crates/sf-history/src/lib.rs` | ripple 6604d76 : `crates/ripple-history/src/gitlog.rs` (shape) + stud-finder main : `lib/stud_finder/churn.rb` + `lib/stud_finder/temporal_coupling.rb` (semantics) | Written fresh. Shape (shell out to one `git log` subprocess, stream `-z` output, bulk-commit filter, per-commit file dedup, pairwise accumulation) follows ripple's `gitlog.rs` pattern. The actual churn + coupling semantics match stud-finder Ruby exactly: `--numstat --no-merges --find-renames --diff-filter=ACDMR`, 180-day default window, binary files guarded via `added != '-'`, rename notation normalized to the new path, bulk threshold = 50 files (counted after restricting to the input file set), coupling ratio = `co_changes / min(own_changes[a], own_changes[b])` (NOT Jaccard), default filters `min_co_changes = 5` + `coupling_threshold = 0.30`. No rename following across the window — a rename within the window is accepted as the new path alone. No sidecar persistence, no incremental update, no lift filter, no base-rate filter (ripple has all of these; stud-finder needs none of them). |
| `rust/crates/sf-score/src/lib.rs` (4- + 5-signal extension) | stud-finder main : `lib/stud_finder/scorer.rb` `DEFAULT_WEIGHTS` + `BASE_WEIGHT_KEYS` + `composite_churn_pct` + `coupling_pct` | Added churn + coupling to the composite. Churn is a base-weight signal at ratio 5 (base keys stay `fan_in : fan_out : complexity : churn = 4 : 2 : 5 : 5`). Coupling is an extra signal outside the base ratio at weight 0.05, matching Ruby `DEFAULT_WEIGHTS[:coupling]`. Churn composite is the percentile of `(count_pct + line_pct) / 2.0` per file, matching Ruby `composite_churn_pct`. Coupling signal is the per-file strongest partner's ratio (0.0 when no partner cleared the Ruby filters). Weights renormalize over whatever signals the caller provided — back-compat with the 2- + 3-signal callers from PR #77 / #78. Existing lift row for sf-score applies. |
| `rust/crates/sf-coverage/src/lib.rs` | stud-finder main : `lib/stud_finder/coverage/resultset.rb` + `lcov.rb` + `cobertura.rb` + `detector.rb` | Ported fresh — three format readers (SimpleCov resultset.json, LCOV `.info`, Cobertura XML) behind one `load(path, root, files)` entry point. Format pick by extension (matches Ruby `Detector::PARSERS`). SimpleCov: supports flat `{coverage: ...}` + nested `{suite: {coverage: ...}}`, handles the `{lines: [...]}` and bare-array shapes, merges multiple suites via per-line max (nil only wins if BOTH are nil), rate = `positive_count / non_nil_count`. LCOV: `end_of_record`-delimited records, `SF:`/`LF:`/`LH:`, falls back to counting `DA:line,count` entries; `LH > LF` is a hard error. Cobertura: XPath for `/coverage/packages/package/classes/class`, `line-rate` passthrough with 0.0..=1.0 range check. Path normalization: strip `project_root/` prefix, suffix-match for absolute paths, `./`-strip otherwise — all matching Ruby. `missing_files` tracks the caller-requested files that didn't appear in the report. No cross-project lift. |
| `rust/crates/sf-score/src/lib.rs` (6- + 7-signal extension) | stud-finder main : `lib/stud_finder/scorer.rb` `coverage_available?` + `interaction_score` + `DEFAULT_WEIGHTS[:coverage]/[:interaction]` | Added coverage + interaction to the composite. Coverage is a base-signal weight 0.095 (matches `fan_out` ratio). Interaction is `fan_in_pct * coverage_pct * w_int` (ratio 0.095), outside the base `4:2:5:5` ratio — matching Ruby `interaction_score`. Both activate together via the `has_coverage` gate (matches Ruby `coverage_available?`). Weights renormalize over whatever the caller supplied so the 5-signal (pre-coverage) and 6-/7-signal (with coverage) callers back-compat cleanly. Existing lift row for sf-score applies. |
| `rust/crates/sf-js/src/lib.rs` | stud-finder main : `lib/stud_finder/js_fan_in.rb` + `js_complexity.rb` (intent); oxc 0.50 (parser) | **Deliberate upgrade, not a direct port**. stud-finder Ruby shells out to `dependency-cruiser` + `eslint` — both require node and npm. The Rust port replaces both with oxc, a native Rust JS/TS parser, so the port runs without node. Semantic parity with Ruby: relative-only module specifiers (bare packages dropped), import shapes `import ... from '...'` + `import('...')` + `require('...')` + `export ... from '...'` + `export * from '...'` all recognised. Complexity decision set: if/else-if, ternary, `||`/`&&`/`??`, `case` clauses, `for`/`for-in`/`for-of`, `while`/`do-while`, `catch`. Per-function max across file (does not recurse into nested function bodies — each is its own entry in the max). Resolution is simpler than depcruise (no tsconfig/package.json walk, no conditional exports): `./x` → try `x.{ts,tsx,js,jsx,mjs,cjs}` then `x/index.{ts,tsx,js,jsx,mjs,cjs}` against the walkdir-registered JS file set. No cross-project lift. |
| `rust/crates/sf-cli/src/main.rs` (JS section) | stud-finder main : `lib/stud_finder/cli.rb` (two-section output contract) | sf-cli grows a second output section headed `# JavaScript` that scores JS files against their own `DependencyGraph` + complexity map. Ruby and JS percentiles are NOT pooled (Ruby would flatten the Ruby distribution with JS fan_in=0 rows). Matches the Ruby CLI's separate-section behaviour. The JS score uses sf-score's availability-mode renormalization with just fan_in + complexity (no churn / coupling / coverage this slice — Ruby's js_fan_in and js_complexity have the same scope). |
