# stud-finder

**Find the files that will hurt you before they do.**

A code risk scoring CLI for Ruby and JavaScript/TypeScript codebases. Ranks every file by structural risk so you know where to put your senior review effort, your refactoring time, and your test coverage — *before* the incident.

```
$ bundle exec bin/stud-finder ./my-rails-app

Ruby
 rank  language    file                              score  evidence  class   new  age_days  escalation  ...
   1   ruby        app/models/proficiency.rb         0.7304   1.0000  branch  false  842                 ...
   2   ruby        app/services/payment_service.rb   0.6890   1.0000  branch  false  611                 ...
   3   ruby        app/controllers/orders_ctlr.rb    0.5721   0.6667  branch  false  520                 ...
```

*Scores and evidence are illustrative. Row 3 has no coverage data (evidence capped at `0.6667 = (1.0 + 1.0 + 0.0) / 3`); rows 1–2 have full coverage (evidence `1.0000`). Real runs on well-distributed repos typically top out around 0.70–0.75 composite score — see Classification.*

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

Every row is labelled with a `class`:

- **trunk** — composite score ≥ `trunk_threshold / 100` (default `trunk_threshold: 85`, so score ≥ 0.85). Load-bearing. High review bar.
- **branch** — composite score ≥ `branch_threshold / 100` and below the trunk cutoff (default `branch_threshold: 50`, so 0.50 ≤ score < 0.85). Meaningful coupling.
- **leaf** — score below the branch cutoff. Isolated. Move fast here.

`--trunk-threshold` and `--branch-threshold` take integer values 1–99 that set the composite-score cutoff (a threshold of 85 means score ≥ 0.85). Classification is driven by the full composite, not by fan-in percentile — a file with modest fan-in but very high complexity + churn can still classify as `trunk`.

Because `score` is a weighted sum of percentile-ranked signals, not itself a percentile, a threshold of 85 does not mean "top 15% of files." Well-distributed projects rarely have any file scoring above 0.85; in tighter distributions the cutoffs may need tuning per-project.

### Absolute floors

After the score threshold runs, safety floors escalate anything visibly dangerous that percentile ranking flattened. A file with raw complexity ≥ 15 or raw fan-in ≥ 25 cannot classify as `leaf` — those files escalate to `branch` regardless of score. Floors escalate only; they never downgrade a `branch` or `trunk`.

The floors exist because tiny or uniform-signal codebases can produce a composite score below the branch threshold even when the raw signals are visibly high — the percentile pass collapses everyone to the middle. Absolute floors catch this failure mode without altering the numeric score.

A floor escalation sets `escalation=complexity_floor` (raw complexity ≥ 15) or `escalation=fan_in_floor` (raw fan-in ≥ 25) on the output row, so consumers can distinguish a floor-escalated branch from a threshold-classified one. When a file meets a floor condition and is also considered new, `escalation=recency_floor` takes precedence — the newness marker wins.

### Newness rules

History-based signals can under-protect brand-new files: a fresh AI-generated file may have little churn, low fan-in, and no established blast radius yet, even though it is often the least proven code in the change. Post-scoring newness rules therefore change only `class`, `new_file`, `age_days`, and `escalation`; the numeric `score` stays honest and unchanged.

A file is considered new when its first commit is within `--new-file-days` days (default 30), or when it has fewer than `--new-file-min-commits` commits in full git history (default 3). New files cannot classify below `branch`; those rows show `escalation=recency_floor`.

A stronger rule runs first: if a new file depends on a structurally `trunk` file through its fan-out edges, it escalates to `trunk` with `escalation=trunk_adjacent`. This highlights new code consuming critical interfaces, where contract-violation risk is highest. Use `--no-newness` to disable both newness rules.

**CI usage:** newness rules require full git history. In GitHub Actions, set `fetch-depth: 0` before running Stud Finder. If Stud Finder detects a shallow clone, it auto-disables both newness rules and emits `shallow_clone_newness_disabled` in `warnings` so classifications match `--no-newness` instead of misclassifying mature files as new.

---

## Evidence

Every row carries an `evidence` value (0.0–1.0) alongside `score`. Score is the weighted signal composite. Evidence is a metadata confidence: how much history + coverage-data backing does that score have?

Evidence combines file age, commit count, and whether coverage data was explicitly provided. A high score with low evidence means "structural signals concentrated risk here, but we're not certain because the file is young or the history is thin." A high score with high evidence is a strong claim.

In shallow clones (git fetch-depth < full history), evidence is `null` on every row because file-metadata history is unavailable. The `shallow_clone_newness_disabled` warning also fires. See the CI note above.

**Gate consumers should threshold `class` for verdicts and `evidence` for confidence, not raw `score` alone.** Output is sorted by `(class_rank, score)` so trunks group above branches above leaves, and `--top N` no longer drops newness-escalated `trunk_adjacent` files behind high-score branches.

---

## Warnings

`analysis.warnings` (available in JSON output) surfaces conditions the run detected that a consumer should know about:

- **`shallow_clone_newness_disabled`** — shallow git clone detected; newness rules auto-disabled.
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
| `--trunk-threshold N` | Composite-score threshold for trunk classification: score ≥ N/100 (integer 1–99, default: 85) |
| `--branch-threshold N` | Composite-score threshold for branch classification: score ≥ N/100 (integer 1–99, default: 50) |
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
- `json` — machine-readable with `meta`, `warnings`, `ruby`, `javascript` sections. `meta.formula` labels the active mode (`5-factor + coupling`, `5-factor`, `4-factor + coupling`, `4-factor`). `meta.weights` reports the normalized weights actually used (with `null` for signals that were unavailable).
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
