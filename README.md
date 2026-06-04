# stud-finder

**Find the files that will hurt you before they do.**

A code risk scoring CLI for Ruby and JavaScript/TypeScript codebases. Ranks every file by structural risk so you know where to put your senior review effort, your refactoring time, and your test coverage — *before* the incident.

```
$ bundle exec bin/stud-finder ./my-rails-app

RANK  LANGUAGE  FILE                              SCORE  CLASS   FAN_IN  COMPLEXITY  CHURN_COMMITS  CHURN_LINES  COVERAGE
1     ruby      app/models/proficiency.rb         0.91   trunk   223     85          11             842          0.99
2     ruby      app/services/payment_service.rb   0.84   trunk   78      91          42             1240         0.22
3     ruby      app/controllers/orders_controller 0.73   branch  61      65          74             910          0.31
4     js        src/components/Dashboard.tsx      0.68   branch  44      56          18             612          —
...
```

---

## Install

Add to your Gemfile:

```ruby
gem 'stud-finder', git: 'https://github.com/bazfer/stud-finder'
```

Then `bundle install`. Or clone and run directly.

**Requirements:** Ruby >= 3.1. For JavaScript support: `dependency-cruiser` and `eslint` installed in the target project (`npm install -D dependency-cruiser eslint`).

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

# With coverage signals (4-factor scoring)
bundle exec bin/stud-finder ./my-rails-app \
  --ruby-coverage ./coverage/resultset.json \
  --js-coverage ./coverage/lcov.info
```

---

## The Four Signals

Each file is scored on up to four independently measured signals. See [PRODUCT.md](PRODUCT.md) for the full theory and weighting math.

| Signal | What it measures | Weight |
|--------|------------------|--------|
| **fan_in** | How many other files depend on this one (blast radius) | 35% |
| **complexity** | Cyclomatic complexity of the hardest method in the file | 25% |
| **churn** | Commit frequency + line volume over a 180-day window | 25% |
| **coverage** | Inverse of line coverage (lower coverage = higher risk) | 15% |

When coverage isn't available, the remaining three signals re-normalize to 100% automatically (3-factor mode).

Files are classified into three labels based on their **fan_in percentile** (not the total score):

- **trunk** — fan_in in the top 15% (default `trunk_threshold: 85`). Load-bearing. High review bar, change with care.
- **branch** — fan_in between the 50th and 85th percentile (default `branch_threshold: 50`). Meaningful coupling.
- **leaf** — everything below the 50th percentile. Isolated. Move fast here.

The total score still drives the ranking. The class label is a separate coupling-based signal.

---

## Language Support

**Ruby:**
- fan_in via Zeitwerk constant mapping (Rails-aware), AST fallback
- complexity via RuboCop
- coverage: SimpleCov resultset JSON, Cobertura XML

**JavaScript / TypeScript (.js, .jsx, .ts, .tsx):**
- fan_in via `dependency-cruiser` (must be installed in the target project)
- complexity via `eslint` (`--rule '{"complexity":["error",0]}'`)
- coverage: LCOV (`.info` format)

Each language gets its own ranking section in the output — Ruby and JS are not pooled.

---

## Flag Reference

| Flag | Description |
|------|-------------|
| `--output FORMAT` | `table` (default), `json`, `markdown`, `csv` |
| `--ruby-coverage PATH` | Ruby coverage report (SimpleCov `.json` or Cobertura `.xml`) |
| `--js-coverage PATH` | JavaScript coverage report (LCOV `.info`) |
| `--coverage PATH` | Deprecated alias for `--ruby-coverage` |
| `--js-timeout N` | dependency-cruiser timeout in seconds (default: 60) |
| `--churn-days N` | Commit lookback window in days (default: 180) |
| `--weights WEIGHTS` | Custom weights as fractions, e.g. `fan_in:0.35,complexity:0.25,churn:0.25,coverage:0.15`. Defaults shown. |
| `--trunk-threshold N` | fan_in percentile cutoff for trunk classification (default: 85) |
| `--branch-threshold N` | fan_in percentile cutoff for branch classification (default: 50) |
| `--exclude PATTERN` | Exclude glob pattern (repeatable). `spec/` and `test/` excluded by default. |
| `--top N` | Emit only the top N results |
| `--diff-base REF` | Score the whole repo but emit only the files changed on `HEAD` vs the merge-base with `REF` (e.g. `origin/staging`). Ranks and scores stay relative to the full repo. Ideal for per-PR runs. |
| `--only PATHS` | Emit only these comma-separated repo-relative paths. Like `--diff-base` but with an explicit list instead of a git diff. Mutually exclusive with `--diff-base`. |
| `--min-files N` | Advisory minimum file count to trust percentiles (default: 20) |
| `--verbose` | Print suppressed per-file warnings to stderr |
| `--version`, `--help` | Self-explanatory |

---

## Output Formats

- `table` — human-readable, aligned columns
- `csv` — spreadsheet-friendly, pipe to a file
- `json` — machine-readable with `meta`, `warnings`, `ruby`, `javascript` sections
- `markdown` — drop directly into a PR comment or issue

---

## What It's For

Run it:
- Before a sprint, to see what the team is about to touch
- Before a major refactor, to identify the load-bearing walls
- Before a code review, to know which PRs deserve extra scrutiny
- On every PR in CI, as a risk-tagged diff context

Don't run it as a gate — risk isn't a binary blocker. Run it as input to human judgment.

---

## Documentation

- **[PRODUCT.md](PRODUCT.md)** — theory, formulas, and the research behind each signal
- **[TRD.md](TRD.md)** — technical requirements document

---

## License

MIT. See [LICENSE](LICENSE).
