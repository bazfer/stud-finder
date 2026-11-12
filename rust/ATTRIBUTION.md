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
