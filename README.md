# stud-finder

**Find the files that will hurt you before they do.**

A code risk scoring CLI for Ruby and JavaScript/TypeScript codebases. Ranks every file by structural risk so you know where to put your senior review effort, your refactoring time, and your test coverage — *before* the incident.

```
$ bundle exec bin/stud-finder ./my-rails-app

Ruby
 rank  language    file                              score  evidence  class   new  age_days  escalation  ...
   1   ruby        app/models/proficiency.rb         0.7304   1.0000  trunk   false  842                 ...
   2   ruby        app/services/payment_service.rb   0.6890   1.0000  trunk   false  611                 ...
   3   ruby        app/controllers/orders_ctlr.rb    0.5721   1.0000  branch  false  520                 ...
```

*Scores and evidence are illustrative. Row 3 has full history but no coverage data (evidence `1.0000` — coverage is a bonus, not a requirement); rows 1–2 have full coverage (evidence `1.0000`). The `class` column reflects percentile rank of composite score within the repo — see Classification.*

The full table adds `fan_in`, `fan_out`, `instability`, `complexity`, `churn_commits`, `churn_lines`, `churn_pct`, `loc`, `loc_pct`, `max_coupling`, `max_coupling_partner`, `coupling_partners`, `coupling_pct`, and `coverage`. Use `--output json` for machine-readable output including a `warnings` section and full `meta`.

---

## Install

After the gem is published:

```bash
gem install stud-finder
```

Or add it to your Gemfile:

```ruby
gem 'stud-finder'
```

Then run `bundle install`.

For edge or unreleased changes, install from git:

```ruby
gem 'stud-finder', git: 'https://github.com/bazfer/stud-finder'
```

Or clone and run directly.

**Requirements:** Ruby >= 3.2. For JavaScript support, install `dependency-cruiser` and `eslint` in the target project (`npm install -D dependency-cruiser eslint`).

---

## Usage

The path is positional. Everything else is optional flags.

```bash
bundle exec bin/stud-finder PATH [options]
```

### Common runs

```bash
# Basic: rank every file in the project
bundle exec bin/stud-finder ./my-rails-app

# CSV output for spreadsheet review
bundle exec bin/stud-finder ./my-rails-app --output csv > risk.csv

# Top 50 highest-risk files, markdown for a PR comment
bundle exec bin/stud-finder ./my-rails-app --top 50 --output markdown

# With coverage signals (activates the five-factor formula and interaction term)
bundle exec bin/stud-finder ./my-rails-app \
  --ruby-coverage ./coverage/resultset.json \
  --js-coverage ./coverage/lcov.info
```

---

## Signals and weights

Each file is scored on up to seven inputs — six direct signals plus one cross-term (`interaction`) that fires when coverage data is present. See [SIGNALS.md](SIGNALS.md) for the theory behind each signal; this section lists the current defaults.

| Signal | Default weight | Notes |
|--------|---------------:|-------|
| **fan_in** | 0.19 | Blast radius — incoming dependencies |
| **fan_out** | 0.095 | Coupling burden — outgoing dependencies |
| **complexity** | 0.2375 | Max cyclomatic complexity of any method in the file |
| **churn** | 0.2375 | Commit frequency + line volume, both percentile-ranked and averaged, then re-ranked |
| **coverage** | 0.095 | Inverse of line coverage (`1 − coverage`), percentile-ranked. Optional. |
| **interaction** | 0.095 | Cross-term: `fan_in_pct × coverage_risk_pct`. Only active when coverage is present. |
| **coupling** | 0.05 | Percentile-rank of `max_coupling` from temporal-coupling analysis. Requires git history. |

Weights sum to 1.00 when all signals are available. The **base-four ratios** (fan_in : fan_out : complexity : churn = 4:2:5:5) are preserved across all availability modes, so removing optional signals and re-normalizing does not distort the relative weighting of the base structural signals.

**Availability modes** — the composite drops unavailable signals and re-normalizes the rest to sum to 1.0:

| coverage | coupling | Active signals | Formula label |
|----------|----------|----------------|---------------|
| ✓ | ✓ | all 7 | `5-factor + coupling` |
| ✓ | ✗ | 6 (drop coupling) | `5-factor` |
| ✗ | ✓ | 5 (drop coverage + interaction) | `4-factor + coupling` |
| ✗ | ✗ | 4 (base only) | `4-factor` |

The formula label appears in JSON output at `meta.formula` and in the stderr scoring note.

### The score

Every signal is percentile-ranked across the full codebase, so scores are relative to the project itself. The composite score is the weighted sum of the active signals plus the interaction cross-term when coverage is present:

```
score = Σ (weight_i × signal_i_pct)  +  (weight_interaction × fan_in_pct × coverage_risk_pct)
```

Result is clamped to `[0.0, 1.0]` and rounded to four decimal places.

---

## Classification

Files are classified into three tiers based on the **percentile rank of their composite score** within the repo:

- **trunk** — top 15% by composite score (default `--trunk-threshold 85`). Load-bearing. High review bar, change with care.
- **branch** — top 50% but below top 15% (default `--branch-threshold 50`). Meaningful coupling.
- **leaf** — everything below the 50th percentile. Isolated. Move fast here.

This means every repo of meaningful size has trunks: a file at score 0.55 is trunk if the rest of the repo scores below it. The absolute floors below provide a safety net for tiny repos.

**Absolute floors:** raw complexity ≥ 15 or raw fan-in ≥ 25 escalates a `leaf` to `branch` regardless of percentile. This ensures that structurally dangerous files in tiny repos (where percentile spread is minimal) still receive elevated attention.

**Tiny repos:** In repos with very few files and uniform scores, the percentile spread may place all files at the same score_pct (0.0), resulting in 0 trunks. The absolute floor is the escape hatch for dangerous files in this case.

Note: `--trunk-threshold 85` and `--branch-threshold 50` now mean "top 15% / top 50% of files by composite score" — not "score value ≥ 0.85 / 0.50". This is a BREAKING semantic change from 0.3.0. Trunk was previously unreachable at defaults (max observed composite score ~0.717 in real repos); this change restores the guarantee that some files are always classified trunk-tier relative to their codebase.

### Absolute floors

After the score threshold runs, safety floors escalate anything visibly dangerous that percentile ranking flattened. A file with raw complexity ≥ 15 or raw fan-in ≥ 25 cannot classify as `leaf` — those files escalate to `branch` regardless of score. Floors escalate only; they never downgrade a `branch` or `trunk`.

The floors exist because tiny or uniform-signal codebases can produce a composite score below the branch threshold even when the raw signals are visibly high — the percentile pass collapses everyone to the middle. Absolute floors catch this failure mode without altering the numeric score.

A floor escalation sets `escalation=complexity_floor` (raw complexity ≥ 15) or `escalation=fan_in_floor` (raw fan-in ≥ 25) on the output row, so consumers can distinguish a floor-escalated branch from a threshold-classified one. When a file meets a floor condition and is also considered new, `escalation=recency_floor` takes precedence — the newness marker wins.

### Newness rules

History-based signals can under-protect brand-new files: a fresh AI-generated file may have little churn, low fan-in, and no established blast radius yet, even though it is often the least proven code in the change. Post-scoring newness rules therefore change only `class`, `new_file`, `age_days`, and `escalation`; the numeric `score` stays honest and unchanged.

A file is considered new when its first commit is within `--new-file-days` days (default 30), or when it has fewer than `--new-file-min-commits` commits in full git history (default 3). New files cannot classify below `branch`; those rows show `escalation=recency_floor`.

A stronger rule runs first: if a new file depends on a structurally `trunk` file through its fan-out edges, it escalates to `trunk` with `escalation=trunk_adjacent`. This highlights new code consuming critical interfaces, where contract-violation risk is highest. Use `--no-newness` to disable both newness rules.

**CI usage:** newness rules require full git history. In GitHub Actions, set `fetch-depth: 0` before running Stud Finder. If Stud Finder detects a shallow clone it automatically runs `git fetch --unshallow` (45 second timeout) to recover full history before computing metadata. If the fetch fails (network error, timeout, offline runner), Stud Finder falls back to disabled newness and emits both `shallow_clone_newness_disabled` and `shallow_clone_unshallow_failed` in `warnings`. Pass `--no-auto-unshallow` to skip the fetch attempt entirely — on shallow clones only `shallow_clone_newness_disabled` fires and classifications match `--no-newness`.

---

## Evidence

Every row carries an `evidence` value (0.0–1.0) alongside `score`. Score is the weighted signal composite. Evidence is a metadata confidence: how much history + coverage-data backing does that score have?

Evidence combines file age and commit count as its base, with coverage data as a bonus signal. The formula is `max(history_only, with_coverage)`, where `history_only = (age + commits) / 2` and `with_coverage = (age + commits + 1.0) / 3`. A mature file with full history always reaches `1.0` regardless of whether a coverage report is present — coverage can only raise evidence above the history baseline, never suppress it. A high score with low evidence means "structural signals concentrated risk here, but we're not certain because the file is young or the history is thin." A high score with high evidence is a strong claim.

In shallow clones where auto-unshallow fails (or `--no-auto-unshallow` is set), evidence is `null` on every row because file-metadata history is unavailable. See the CI note above.

**Gate consumers should threshold `class` for verdicts and `evidence` for confidence, not raw `score` alone.** Output is sorted by `(class_rank, score)` so trunks group above branches above leaves, and `--top N` no longer drops newness-escalated `trunk_adjacent` files behind high-score branches.

---

## Warnings

`analysis.warnings` (available in JSON output) surfaces conditions the run detected that a consumer should know about. Starting in schema version `1`, every warning is an object with `code` and human-readable `message` fields; bare warning strings are no longer emitted.

- **`shallow_clone_newness_disabled`** — shallow git clone detected; newness rules auto-disabled (auto-unshallow also failed, or `--no-auto-unshallow` was passed).
- **`shallow_clone_unshallow_failed`** — `git fetch --unshallow` was attempted but failed (network error or timeout); evidence is unavailable. Use `fetch-depth: 0` in CI or pass `--no-auto-unshallow` to suppress the attempt.
- **`insufficient_dispersion_<signal>`** — every file in the codebase has the same non-zero raw value for `<signal>`, so its percentile-ranked contribution collapsed to `0.0`. The score is unchanged; the warning flags that the signal is silently uninformative rather than genuinely absent. One per affected signal: `fan_in`, `fan_out`, `complexity`, `churn`, `coverage`, `interaction`, `coupling`.
- Language-specific warnings such as `js_depcruise_no_config` when the JS pipeline had to fall back.

---

## Informational columns

These ride alongside the score to give reviewers extra context, but do not contribute to it directly:

- **`instability`** / **`instability_pct`** — `fan_out / (fan_in + fan_out)`, and its percentile rank across the repo. High instability = depends on a lot while little depends on it.
- **`max_coupling`** / **`max_coupling_partner`** / **`coupling_partners`** — temporal coupling from git history. The strongest co-change ratio with any partner file, the path of that strongest partner, and how many partners cross the threshold. `coupling_pct` (the percentile rank of `max_coupling`) does contribute to the score at weight `0.05` — the raw fields are informational.

On ties the strongest partner is chosen deterministically: highest coupling, then highest co-change count, then alphabetical path; `max_coupling_partner` is an empty string when a file has no qualifying partners. Coupling is computed once over the full file set in the main scan (one extra `git log` pass), so cross-language co-change is captured. By default, commits touching more than 50 scored files are skipped as bulk commits; use `--coupling-max-commit-files 0` for unlimited/legacy behavior.

---

## Language Support

**Ruby:**
- fan_in via Zeitwerk constant mapping (Rails-aware), AST fallback
- complexity via RuboCop
- coverage: SimpleCov resultset JSON, Cobertura XML

### Rails inference

Ruby fan-in includes conservative Rails-style implicit references by default. Association calls such as `belongs_to :user`, `has_one :profile`, `has_many :comments`, and `has_and_belongs_to_many :tags` are treated as references to their likely model constants. Literal `class_name: 'Foo::Bar'` overrides the symbol; dynamic `class_name:` values are ignored rather than guessed. Disable this with `--no-rails-inference`.

**JavaScript / TypeScript (.js, .jsx, .ts, .tsx):**
- fan_in via `dependency-cruiser` (must be installed in the target project)
- complexity via `eslint` (`--rule '{"complexity":["error",0]}'`)
- coverage: LCOV (`.info` format)

Stud Finder first runs dependency-cruiser with the target project's normal config so path aliases, TypeScript config, and bundler resolution can be honored. If that fails because no usable configuration is available, it retries once with `--no-config` and reports `js_depcruise_no_config`. The fallback keeps analysis running, but aliases such as `tsconfig` paths and webpack aliases will not resolve, so JS/TS `fan_in` may be undercounted. For alias-heavy TypeScript projects, run `npx depcruise --init` in the target repo for accurate results.

Each language gets its own ranking section in the output — Ruby and JS are not pooled.

---

## Flag Reference

`stud-finder --help` is the authoritative reference; this table is a summary.

| Flag | Description |
|------|-------------|
| `--output FORMAT` | `table` (default), `json`, `markdown`, `csv` |
| `--ruby-coverage PATH` | Ruby coverage report (SimpleCov `.json` or Cobertura `.xml`) |
| `--js-coverage PATH` | JavaScript coverage report (LCOV `.info`) |
| `--coverage PATH` | Deprecated alias for `--ruby-coverage` |
| `--js-timeout N` | dependency-cruiser timeout in seconds (default: 60) |
| `--no-rails-inference` | Disable Rails association/string fan-in inference |
| `--churn-days N` | Commit lookback window in days (default: 180). Churn uses git rename detection; within the window, rename commits are attributed to the new path when git pairs the rename. |
| `--weights WEIGHTS` | Custom weights as fractions, e.g. `fan_in:F,fan_out:O,complexity:C,churn:H,coverage:V[,interaction:I][,coupling:P]`. The five base keys (`fan_in`, `fan_out`, `complexity`, `churn`, `coverage`) are required. `interaction` and `coupling` are optional: when omitted, `interaction` defaults to `0.0` (custom weights opt-in) and `coupling` defaults to `0.05`. Each value must be in `[0.0, 1.0]`. When no coverage data is provided, `coverage` must be `0.0`. |
| `--interaction-weight N` | Sugar flag for setting only the interaction weight. |
| `--coupling-weight N` | Sugar flag for setting only the coupling weight. Bounds-checked `[0.0, 1.0]`. |
| `--trunk-threshold N` | composite-score percentile cutoff for trunk classification; top (100-N)% of files by score (default: 85) |
| `--branch-threshold N` | composite-score percentile cutoff for branch classification; top (100-N)% of files by score (default: 50) |
| `--exclude PATTERN` | Exclude glob pattern (repeatable). `spec/` and `test/` excluded by default. |
| `--top N` | Emit only the top N results |
| `--diff-base REF` | Score the whole repo but emit only the files changed on `HEAD` vs the merge-base with `REF` (e.g. `origin/staging`). Ranks and scores stay relative to the full repo. Ideal for per-PR runs. |
| `--only PATHS` | Emit only these comma-separated repo-relative paths. Like `--diff-base` but with an explicit list instead of a git diff. Mutually exclusive with `--diff-base`. |
| `--min-files N` | Advisory minimum file count to trust percentiles (default: 20) |
| `--coupling-threshold FLOAT` | Minimum temporal-coupling ratio for edges output and main-scan coupling columns (default: 0.30) |
| `--coupling-min-commits N` | Minimum co-change count for temporal-coupling edges/columns (default: 5) |
| `--coupling-max-commit-files N` | Skip temporal-coupling commits touching more than N scored files (default: 50; `0` = unlimited) |
| `--new-file-days N` | Treat files first committed within N days as new (default: 30; `0` disables the age floor) |
| `--new-file-min-commits N` | Treat files with fewer than N full-history commits as new (default: 3; `0` disables the commit-count floor) |
| `--no-newness` | Disable new-file classification rules |
| `--verbose` | Print suppressed per-file warnings to stderr |
| `--version`, `--help` | Self-explanatory |

---

## Output Formats

- `table` — human-readable, aligned columns
- `csv` — spreadsheet-friendly, pipe to a file
- `json` — machine-readable with `meta`, `warnings`, `ruby`, `javascript` sections. `meta.schema_version` is the integer JSON schema version (`1` as of Stud Finder 0.6.0). `meta.formula` labels the active mode (`5-factor + coupling`, `5-factor`, `4-factor + coupling`, `4-factor`). `meta.weights` reports the normalized weights actually used (with `null` for signals that were unavailable).
- `markdown` — drop directly into a PR comment or issue

---

## What It's For

Run it:
- Before a sprint, to see what the team is about to touch
- Before a major refactor, to identify the load-bearing walls
- Before a code review, to know which PRs deserve extra scrutiny
- On every PR in CI, as a risk-tagged diff context

Don't run it as a hard blocker on raw `score` — `score` is evidence, not a decision. Threshold `class` for verdicts and `evidence` for confidence.

---

## Documentation

- **[SIGNALS.md](SIGNALS.md)** — theory behind each signal, and the score / class / evidence separation
- **[CHANGELOG.md](CHANGELOG.md)** — per-version changes, weight-shift history, breaking notes

---

## License

MIT. See [LICENSE](LICENSE).
