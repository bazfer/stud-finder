# stud-finder

**Find the files that will hurt you before they do.**

---

## The Problem

Every codebase has load-bearing walls. Files that dozens of other files depend on. Files that change every sprint. Files whose complexity means one wrong edit cascades into a day of debugging.

Most teams discover these files the hard way — after the incident.

stud-finder surfaces them before you touch them.

---

## What It Does

stud-finder analyzes a codebase and produces a ranked list of every file, scored by structural risk. Run it before a sprint, before a refactor, before a code review. Know which files deserve extra attention before anyone writes a line.

```
$ stud-finder ./my-rails-app

FILE                              SCORE  LABEL   FAN_IN  COMPLEXITY  CHURN_COMMITS  CHURN_LINES  CHURN_PCT  COVERAGE
app/models/user.rb                0.91   trunk   0.97    0.42        0.88           0.91         0.89       0.14
app/services/payment_service.rb   0.84   trunk   0.78    0.91        0.71           0.68         0.69       0.22
app/controllers/orders_controller 0.73   branch  0.61    0.65        0.74           0.77         0.75       0.31
...
```

Three labels, one decision framework:

- **Trunk** — load-bearing. Change with care. High review bar.
- **Branch** — meaningful coupling. Worth a second look.
- **Leaf** — isolated. Lower risk. Move fast here.

---

## The Four Signals

Each file is scored on up to four independently measured signals, each grounded in decades of software engineering research.

### 1. Fan-in — Blast Radius

*"How many files depend on this one?"*

Rooted in Robert Martin's **afferent coupling (Ca)** metric (1994) and graph theory in-degree analysis. A file with fan_in 60 means 60 other files break if it breaks. The Stable Dependencies Principle says: high-coupling files must be treated as infrastructure.

stud-finder builds the dependency graph via static analysis — Zeitwerk constant mapping for Rails, falling back to AST scanning. No runtime instrumentation required.

**Weight: 35% of total score** (highest signal — coupling is the multiplier that makes every other risk worse)

### 2. Complexity — Cognitive Load

*"How hard is this file to reason about?"*

Cyclomatic complexity, measured as the **maximum across any single method** in the file. A file with one function of complexity 12 is riskier than a file with ten functions of complexity 3 each — the hardest function determines how deep you have to go.

Computed via RuboCop's static analysis engine. No manual annotation.

**Weight: 25% of total score**

### 3. Churn — Change Velocity

*"How often is this file being touched, and how much?"*

A composite signal: 50% commit frequency + 50% lines changed, both percentile-ranked across the full codebase. A file touched in 40 commits but only for small fixes is different from a file touched in 40 commits with major rewrites each time.

Computed from git history over a configurable window (default: 180 days). Language-agnostic.

**Weight: 25% of total score**

### 4. Coverage — Safety Net

*"If this file breaks, will tests catch it?"*

Low coverage on a high-risk file is compounded danger — no blast-radius detection, no complexity safety net, no test catch. Coverage is measured as an inverse (0% coverage = maximum penalty), and files absent from the coverage report are handled via 3-factor fallback rather than penalized falsely.

Supports Cobertura XML (RSpec + SimpleCov), LCOV (Jest, lcov), and SimpleCov JSON resultsets. Auto-detected by file extension.

**Weight: 15% of total score** (optional — runs as 3-factor model when no coverage report provided)

---

## The Score

Each signal is percentile-ranked across the full codebase — so scores are always relative to the project itself, not an external benchmark. A file at the 90th percentile of fan_in has more incoming dependencies than 90% of its peers.

The composite score (0.0–1.0) weights the signals and produces the ranked output. Classification thresholds are configurable.

**3-factor formula (no coverage):**
```
score = 0.41 × fan_in_pct + 0.29 × complexity_pct + 0.29 × churn_pct
```

**4-factor formula (with coverage):**
```
score = 0.35 × fan_in_pct + 0.25 × complexity_pct + 0.25 × churn_pct + 0.15 × (1 − coverage)
```

---

## Use Cases

**Pre-sprint risk assessment** — before planning, run stud-finder against the files your team is about to touch. Trunk files get more review time budgeted.

**Refactor prioritization** — you have ten candidates for cleanup. stud-finder tells you which ones have the highest blast radius if the refactor goes wrong.

**Onboarding** — new engineer joining the team. Here's the trunk map. These are the files you ask before changing.

**PR review triage** — reviewer bandwidth is finite. Direct it at the files that matter.

**Architecture health monitoring** — run stud-finder weekly. Watch if trunk is growing or shrinking. Trunk growth is a coupling smell.

---

## Technical Foundation

- **Language:** Ruby gem, zero runtime instrumentation
- **Static analysis:** RuboCop (complexity), Zeitwerk + custom AST (fan_in), git log (churn)
- **Coverage formats:** Cobertura XML, LCOV, SimpleCov JSON — auto-detected
- **Output formats:** table (default), JSON, CSV, Markdown
- **Configuration:** CLI flags for weights, thresholds, excludes, churn window
- **Requires:** Ruby, RuboCop, git. Nothing else for Ruby analysis.

---

## Roadmap

**M1–M3 — Complete**
4-signal composite score (Ruby + JS/TS). `--diff-base` / `--only` filter for per-PR output. Per-PR CircleCI integration — stud-finder runs on every PR, posts ranked artifact and PR comment. Non-blocking.

**M4 — Next: fan-out, instability, `stud-finder edges`**
Fan-out (efferent coupling) and instability (`fan_out / (fan_in + fan_out)`) added to every row in the core output. New `stud-finder edges FILE` subcommand emits the actual dependency edge list for a specific file — dependents and dependencies, both sorted by risk score. Shifts the output from "this file scores high" to "here are the specific files in the blast radius."

**M5 — Sentry integration**
Connect to the Sentry REST API. Parse production stack traces, aggregate error frequency by source file. A runtime signal: not structural approximation but observed failure in production. `--sentry-token`, `--sentry-org`, `--sentry-project` flags. Percentile-ranked and added to the composite score.

**M6 — Temporal coupling**
Co-change frequency from git history: file pairs that change together more often than expected by chance. Surfaces hidden coupling that static analysis cannot see — implicit contracts, shared state, callback side effects. Observed behavior, not structural approximation.

**Pinned — Producer-consumer dependency mapping**
Explicitly surfacing which components consume data produced by other components, flagging pairs with high temporal coupling but low static coupling as candidates for explicit contract documentation.

**M7 — Merge-to-staging S3 timeline (lowest priority)**
Full stud-finder run on each merge to the mainline branch → JSON → S3, keyed by timestamp + commit SHA. Durable risk-over-time feed for trend analysis.

**Future — Toward a validated risk estimator**
Calibrated weights back-tested against bug history. Historical bug density as a direct input metric. Change-scope awareness (per-PR risk = file-risk × change-magnitude × change-type). Test quality beyond line coverage. See `VISION.md` for the full analysis.

---

## Why stud-finder?

In construction, a stud finder locates the load-bearing structure inside a wall before you drill. You don't guess — you know exactly where the structure is.

Same principle. Before you refactor, before you sprint, before you review — know where the load-bearing code is.

---

*Built by Artífice. Ruby gem. Open to collaboration.*
