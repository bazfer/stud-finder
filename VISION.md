# stud-finder — Vision & Roadmap to Risk Estimator

## What stud-finder is today

A **triage and orientation tool**. It surfaces which files deserve attention before you touch them, based on four structural signals: fan-in (blast radius), complexity (cognitive load), churn (change velocity), and coverage (safety net). Scores are percentile-ranked across the full codebase — so the output is always relative to the project itself.

Legitimate uses today:
- Bootstrapping orientation in an unfamiliar codebase — fast
- Prioritizing which modules get a formal stabilization review first
- Making implicit architectural risk explicit ("everyone knows employee.rb is risky" — now there's a number)
- Directing reviewer bandwidth at the files that matter

---

## The honest limits of the current model

**1. Coupling ≠ correctness.**
Fan-in measures blast radius, not bug probability. High fan-in files (`employee.rb`, `objective_template.rb`) tend to get the most attention, the most tests, the most experienced eyes. They can be riskier to change, but they may also be the best-maintained files in the codebase. High structural score does not mean high bug rate.

**2. The weights are invented.**
`fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15` — these were chosen on first principles. They haven't been back-tested against actual bug outcomes. Without fitting, the score is a ranking, not an estimate.

**3. Bugs live at interfaces, not in files.**
The dominant class of production bug — traced across 30 real incidents — is not "a single high-risk file was wrong." It's an implicit contract between a producer and a consumer that was never explicitly defined, then violated by a refactor or a lifecycle change. No individual file scores high; the interface between them is broken. File-level scoring misses this entire class.

**4. File-risk ≠ change-risk.**
A one-line comment edit to `employee.rb` is not the same risk as a 400-line refactor. The current score is on the file, not on the change. Without change-scope awareness, the score can't distinguish.

**5. Coverage measures execution, not correctness.**
Line coverage tells you which lines run during tests. It doesn't tell you whether the tests assert the invariants that matter. Files with 90%+ coverage have produced serious production bugs because the broken invariant was never tested.

**6. No runtime signal.**
Static analysis is backward-looking about structure. Where code is actually failing in production right now is a stronger signal than where it looks risky structurally.

---

## Why similar tools don't fully exist

CodeClimate measures complexity + churn. Structure101 measures coupling. Danger and CodeOwners operate on change shape. Nobody has combined all of these into a single validated risk score for review prioritization — because:

- Signal-to-noise at the file level is low without calibration against outcomes
- Teams that need precision use formal methods (property-based testing, TLA+, invariant documentation) on specific risky subsystems — not file-level rankings
- The actuators (review depth, staging gate) are hard to connect to file topology alone

This is not a reason not to build it. It's a reason to be honest about what validation is required before calling it an estimator.

---

## The path to a genuine risk estimator

### Already built (M1–M3)
- 4-signal composite score (fan_in, complexity, churn, coverage)
- `--diff-base` / `--only` filter — per-PR output scoped to touched files, full-repo scoring preserved
- Per-PR CircleCI job — stud-finder runs on every PR, posts markdown + CSV artifact and PR comment

### M4 — Fan-out, instability, and `stud-finder edges`
Retain the dependency graph that fan_in already builds internally (and discards). Two deliverables:

**Fan-out + instability in the core output (every row):**
- `fan_out` — raw count of files this file depends on (efferent coupling)
- `instability` — Robert Martin's metric: `fan_out / (fan_in + fan_out)`. Bounded [0, 1]. A file with fan_in=100, fan_out=10 → instability=0.09 (stable). A file with fan_in=2, fan_out=50 → instability=0.96 (fragile consumer).

Fan-out captures a different failure mode than fan-in. Several production bugs in the Covalent 2026 incident set were fan-out failures: a consumer depended on a fragile implicit contract, not on a high-blast-radius file. Fan-in alone would not have flagged them.

Instability is not yet added to the composite score — first validate it against known fan-out bugs (CO-21367 is the reference case), then calibrate the weight.

**`stud-finder edges FILE [PATH]` subcommand:**
Drill-down for a specific file. Emits:
- Dependents — files that depend on this file (incoming edges), sorted by score desc
- Dependencies — files this file depends on (outgoing edges), sorted by score desc

### M5 — Sentry integration
Query the Sentry REST API for production issues, parse stack trace frames, aggregate by source file. `sentry_events[file]` = distinct production errors that touched this file. Percentile-ranked as a scored signal.

This is the only runtime signal in the stack — not "this file looks risky structurally" but "this file is actually in the stack when things break in production." Stronger than any static approximation.

CLI: `--sentry-token`, `--sentry-org`, `--sentry-project`. Main implementation challenge: path normalization (Sentry frame paths → repo-relative paths).

### M6 — Temporal coupling
Co-change frequency from git history: file pairs that change together in the same commit more often than expected by chance. This captures hidden coupling that static analysis cannot see — implicit contracts, shared state, callback side effects that always require coordinated edits.

This is the most empirically defensible structural metric in the roadmap — observed behavior in real production git history, not a theoretical approximation. Files that always change together have a hidden dependency. If that dependency is not explicit, it's a risk.

### Pinned — Producer-consumer dependency mapping
Explicitly mapping what data each component consumes and who produces it. The interface between a data producer (a clone/publish flow, an import pipeline) and a data consumer (a blueprint serializer, a dashboard query) is where the hardest-to-detect bugs live. These interfaces are not visible to fan-in analysis because they operate through data shape (a join table schema, a JSON payload structure) rather than constant references.

This is a design artifact — an explicit contract document — not just a metric. The tooling question: can stud-finder surface candidate producer-consumer pairs (files with high temporal coupling but low static coupling) and flag them for explicit contract documentation?

### What's still missing to reach "validated estimator"

These five items are the gap between "plausible ranking" and "calibrated risk estimator":

**1. Calibrated weights (back-tested against bug history).**
Run stud-finder against git history at the introducing commit of each known production bug. What were the scores of the introducing files? Fit the weights so the score would have ranked those files highly before the bug surfaced. The seed data for this already exists: a 57-ticket CSV of true production issues with introducing PRs traced. This is a supervised fitting problem — the labels (bug/no-bug) and the features (fan_in, complexity, churn) are both available.

**2. Historical bug density as a direct input metric.**
Count production bugs introduced per file over a trailing window (e.g., 2 years). This is the strongest single signal available — it is the outcome variable itself, used as a predictor. Files that score high structurally AND have prior bug density are high-confidence risk. Files that score high structurally but have zero prior bugs may be well-maintained trunks. Combined with structural signals, this dramatically improves precision.

**3. Change-scope awareness (delta-risk = file-risk × change-magnitude × change-type).**
File-risk × lines changed is a trivially computable multiplier already partially available in the per-PR job. Change-type (touching a public interface vs. an internal method vs. a query) is harder — static analysis cannot distinguish reliably. LLM-based semantic classification of the diff is the natural extension here: classify the change type, feed it into a per-PR risk score that is change-specific, not file-specific.

**4. Test quality beyond line coverage.**
Mutation score — does killing a line cause a test failure? — is expensive to compute but dramatically more signal-rich than line coverage. Even assertion density (assertions per tested line) would improve on raw coverage. The goal: distinguish "this file has 90% coverage with meaningful assertions" from "this file has 90% coverage with happy-path-only tests that missed the broken invariant."

**5. Runtime signals (Sentry / error rates per file).**
Where code is actually failing in production is a stronger signal than where it looks structurally risky. A file with moderate structural score but high Sentry event rate is more dangerous than a high-score file that has never produced an error. Sentry API integration — mapping error events back to source files — would make stud-finder empirical at the risk-in-production level, not just structural.

---

## What each milestone unlocks

| Milestone | What it enables |
|-----------|----------------|
| M4 `--explain` | Actionable blast-radius view per file; fan-out as a new signal class |
| M5 temporal coupling | Empirical hidden coupling detection; surfaces implicit contracts |
| Pinned producer-consumer | Framework for explicit interface contracts; feeds stabilization review docs |
| Calibrated weights | Score becomes an estimate, not just a ranking |
| Historical bug density | Strongest predictor; validates structural signals against outcomes |
| Change-scope awareness | Per-PR risk score, not per-file; connects to actual review decisions |
| Test quality | Coverage signal becomes meaningful rather than gameable |
| Runtime signals | Ground truth: where code is failing right now |

**Immediately actionable (data already in hand):** calibrated weights + historical bug density. The 57-ticket CSV with introducing PRs is the training set.

**Near-term with existing infrastructure:** change-scope awareness (lines diff is in the per-PR job; LLM classification via Covy is a natural extension).

**Longer-term:** mutation score, Sentry integration.

---

## The ceiling without validation

Without calibrated weights and historical bug density (items 1 and 2 above), stud-finder identifies the right *neighborhoods* to be suspicious of but cannot say how suspicious to be about a specific change. It remains a triage tool — valuable, but not an estimator.

With items 1 and 2, stud-finder becomes a validated estimator: the score has a known relationship to bug probability, not just a plausible structural theory. With item 3 (change-scope), it becomes actionable at the PR level — where the review and staging gate decisions actually happen.

---

## Updated milestone roadmap

| Milestone | Status | Description |
|-----------|--------|-------------|
| M1 | Done | 4-signal composite score, Ruby |
| M2 | Done | JS/TS support, `--diff-base` / `--only` filter |
| M3 | Done (PR open) | Per-PR CircleCI job — artifact + PR comment |
| M4 | Next | Fan-out + instability in core output; `stud-finder edges FILE` subcommand |
| M5 | Queued | Sentry integration — runtime error frequency as a scored signal |
| M6 | Queued | Temporal coupling — co-change frequency from git history |
| Pinned | Queued | Producer-consumer dependency mapping |
| M7 | Lowest prio | Merge-to-staging S3 timeline producer |
| Future | Backlog | Calibrated weights, historical bug density, change-scope LLM classification, mutation score |
