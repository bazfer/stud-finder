# stud-finder — Signal Theory

This document explains **what each signal measures** and **why it correlates with risk**. It does not list weights, defaults, or flag semantics — those live in [README.md](README.md) and `stud-finder --help`, which stay current with the code. Read this once to understand the model; refer back to it only when adding a new signal or challenging an existing one.

---

## The scoring signals

Each file is scored on up to seven independently measured inputs. Six are direct signals; one (`interaction`) is a cross-term that only activates when coverage data is present.

### 1. Fan-in — Blast Radius

*"How many files depend on this one?"*

Rooted in Robert Martin's **afferent coupling (Ca)** metric (1994) and graph theory in-degree analysis. A file with fan-in 60 means 60 other files break if it breaks. The Stable Dependencies Principle says: high-coupling files must be treated as infrastructure.

Built via static analysis — Zeitwerk constant mapping for Rails, falling back to AST scanning for Ruby; `dependency-cruiser` for JavaScript/TypeScript. No runtime instrumentation.

### 2. Fan-out — Coupling Burden

*"How many files does this one depend on?"*

Rooted in Robert Martin's **efferent coupling (Ce)** metric. A high fan-out file has more direct dependencies to understand, coordinate, and mock in tests. Several production bugs surface not because the failing file had a large blast radius, but because it depended on a fragile implicit contract in one of its many upstream neighbors.

### 3. Complexity — Cognitive Load

*"How hard is this file to reason about?"*

Cyclomatic complexity, measured as the **maximum across any single method** in the file. A file with one function of complexity 12 is riskier than a file with ten functions of complexity 3 each — the hardest function determines how deep you have to go.

Computed via RuboCop for Ruby, ESLint for JS/TS. No manual annotation.

### 4. Churn — Change Velocity

*"How often is this file being touched, and how much?"*

A composite signal built in two stages:

1. Commit count and lines-changed are each percentile-ranked across the codebase over the churn window (default 180 days).
2. The two percentiles are averaged 50/50 and **the average is percentile-ranked again** to produce the final `churn_pct`.

The second re-ranking matters: without it, averaging two uniform distributions produces a triangular distribution centered at 0.5, which halves the effective variance of the churn signal. Re-ranking restores the intended dispersion, so churn contributes the weight the composite claims it does.

A file touched in 40 commits but only for small fixes is different from a file touched in 40 commits with major rewrites each time. Combining commit count with line volume captures both patterns.

### 5. Coverage — Safety Net

*"If this file breaks, will tests catch it?"*

Low coverage on a high-risk file is compounded danger — no complexity safety net, no test catch. Coverage is measured as inverse risk (`1 − coverage`) and percentile-ranked across the codebase. Files absent from the coverage report score as maximum coverage risk but render as `—` so missing data is distinct from explicit 0% coverage.

Supports Cobertura XML (RSpec + SimpleCov), LCOV (Jest, lcov), and SimpleCov JSON resultsets. Auto-detected by file extension.

Coverage is optional. When no report is provided, the composite drops to a four-signal formula and re-normalizes the remaining structural weights to sum to 1.0.

### 6. Temporal Coupling — Hidden Contracts

*"Which files always change together?"*

Co-change frequency from git history: file pairs that change together in the same commit more often than expected by chance. Captures hidden coupling that static analysis cannot see — implicit contracts, shared state, callback side effects that always require coordinated edits.

This is the most empirically defensible structural metric available: observed behavior in real production git history, not a theoretical approximation. When two files always change together, they have a hidden dependency; if that dependency is not explicit, it is a risk.

Each file's row keeps the strongest pair's ratio (`max_coupling`), that partner's path (`max_coupling_partner`), and the count of qualifying pairs. `coupling_pct` is the percentile rank of `max_coupling` across the codebase and contributes to the composite score. Requires git history; unavailable on shallow clones.

Coupling is weighted deliberately low. Files that historically change together is correlational, not causal — genuine implicit contracts share a bin with routine "we touched both because they were in the same feature." Underweighting keeps false positives from dominating the ranking until an outcome-labelled dataset justifies raising it.

### 7. Interaction — Fan-in × Coverage Risk

*"High blast radius AND weak test net."*

A cross-term, not an independent signal: `fan_in_pct × coverage_risk_pct`, contributed with its own weight when coverage data is present. Captures the compounded danger of a high-fan-in file that also has poor test coverage. The two signals are independently weighted (fan-in and coverage), so a linear combination cannot express "both risky at once"; the interaction term does.

Only active in the five-factor (coverage-present) formula. Silently dropped in the four-factor mode along with coverage.

---

## Percentile ranking

Every signal is percentile-ranked across the full codebase — so scores are always relative to the project itself, not an external benchmark. A file at the 90th percentile of fan-in has more incoming dependencies than 90% of its peers, regardless of whether that peer set has 50 files or 5,000.

Ties receive the same rank. Edge cases: a codebase with only one file gets `0.0` for every signal (nothing to rank against); a codebase where every file has the identical raw value for a signal also collapses to `0.0` — see **Insufficient-dispersion warnings** below.

---

## Score vs. classification vs. evidence

Three pieces of output serve three purposes. Confusing them causes gate consumers to threshold the wrong number.

- **`score`** (0.0–1.0, four decimals) — the weighted composite. This is **evidence about the file's structural risk**, nothing more. Higher score means the signals concentrated more risk on this file.
- **`class`** (`leaf` / `branch` / `trunk`) — the decision label. Driven by the composite-score PERCENTILE across the codebase against configurable thresholds (defaults: `branch` at top 50%, `trunk` at top 15%). This guarantees that some files are always trunk-tier relative to their repo — a file scoring 0.55 can be trunk if the rest of the codebase scores below it. This is what a gate should threshold on for verdicts.
- **`evidence`** (0.0–1.0) — a metadata confidence value based on file age, commit count, and optionally coverage presence. The formula is `max(history_only, with_coverage)`, where `history_only = (age + commits) / 2` and `with_coverage = (age + commits + 1.0) / 3` (only included when coverage data is present). Coverage is a bonus: a mature file with full history always reaches `1.0` even without a coverage report. A high score with low evidence means "structural signals concentrated risk here, but we're not certain because the file is young or the history is thin." Gates should threshold `evidence` for confidence, not raw `score`.

### Absolute floors

Classification runs the composite-score threshold first, then applies safety floors. A file with raw complexity ≥ 15 or raw fan-in ≥ 25 cannot classify as `leaf`; those files escalate to `branch` regardless of score. Floors escalate only — a file already classed as `branch` or `trunk` by score is never downgraded by the floor logic.

The floors catch the tiny-repo / uniform-signal failure mode: in a 10-file codebase where every file has a high raw complexity, percentile ranking flattens everyone to the middle, the composite score sits around 0.4, and every file classifies as `leaf` despite being structurally dangerous.

A floor escalation sets `escalation=complexity_floor` (raw complexity ≥ 15) or `escalation=fan_in_floor` (raw fan-in ≥ 25) on the output row. When a file qualifies for a floor escalation and is also considered new, `escalation=recency_floor` takes precedence — the newness marker wins over the floor marker.

### Newness rules

History-based signals under-protect brand-new files: a fresh AI-generated file may have little churn, low fan-in, and no established blast radius yet, even though it is often the least proven code in the change.

Post-scoring newness rules therefore adjust `class`, `new_file`, `age_days`, and `escalation` only — the numeric `score` stays honest and unchanged. A file is considered new when its first commit is within the newness window (default 30 days) or it has fewer than the minimum commit count (default 3). New files cannot classify below `branch`; those rows carry `escalation=recency_floor`.

A stronger rule runs first: if a new file depends on a structurally `trunk` file through its fan-out edges, it escalates to `trunk` with `escalation=trunk_adjacent`. This highlights new code consuming critical interfaces, where contract-violation risk is highest.

Newness rules require full git history — see the `fetch-depth: 0` note in the README.

---

## Insufficient-dispersion warnings

When every file in a codebase has an identical non-zero raw value for a signal — five files with 100 lines each, ten files with the same cyclomatic complexity, a tiny repo where nothing has been touched in the churn window — the percentile-rank pass collapses that signal to all-`0.0`. The signal contributes nothing to differentiation, but the underlying data is not "missing" — it exists, it just happens to be flat.

`analysis.warnings` emits `insufficient_dispersion_<signal>` in that case. The score is unchanged; the warning surfaces so consumers know a signal is silently uninformative rather than genuinely absent. Warnings fire per signal (`fan_in`, `fan_out`, `complexity`, `churn`, `coverage`, `interaction`, `coupling`) and only when raw data is present but dispersion is degenerate.

---

## What's out of scope for this document

- **Current default weights and thresholds.** Live in [README.md](README.md) and `stud-finder --help`. Weights get re-balanced between versions; keeping them in this document guarantees they drift out of sync.
- **CLI flag semantics.** `stud-finder --help` is the authoritative reference.
- **JSON output shape.** Emitted by `stud-finder --output json` — that output is the schema.
- **Roadmap / calibration plans.** Tracked in commit history and per-arc architect reviews, not in a maintained document.

---

## Instability (informational)

`instability` = `fan_out / (fan_in + fan_out)` — Robert Martin's I metric, bounded [0, 1]. Reported in every row for context but **not scored**. A file with fan-in 100 and fan-out 10 has instability 0.09 (stable, load-bearing); a file with fan-in 2 and fan-out 50 has instability 0.96 (fragile consumer). The metric captures a real property but has proved noisy enough at the file level that including it in the composite would dilute signals with better outcome correlations. It stays as reviewer context.

---

## Honest limits

- **Coupling ≠ correctness.** High-coupling files often receive the most attention and best maintenance precisely because they are load-bearing. A high score means "this file concentrates risk signals"; it does not mean "this file has more bugs." Score is a triage signal, not an audit.
- **Weights are heuristics, not calibrated estimates.** The default weights encode a plausible structural ordering but have not been fit against a labelled bug dataset. Until outcome calibration runs, treat weight differences as directional, not quantitative.
- **Bugs live at interfaces, not files.** The most common production bugs surface at the boundary between two files — a producer/consumer contract that changed on one side. A single file's score misses cross-file interaction; use the `coupling` signal and `edges` output to reason about pairs.
- **File-risk ≠ change-risk.** A file that is structurally load-bearing but untouched in a sprint carries lower per-change risk than a simpler file being actively rewritten. For per-PR gating, combine score with diff size rather than using score alone.
- **Coverage measures execution, not assertion quality.** A line-covered file may have shallow assertions that miss real logic errors. Coverage as a risk signal catches the "no test catches a change here" case; it does not vouch for test depth.
