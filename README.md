# stud-finder

**Find the files that will hurt you before they do.**

A code risk scoring CLI for Ruby and JavaScript/TypeScript codebases. Ranks every file by structural risk so you know where to put your senior review effort, your refactoring time, and your test coverage — *before* the incident.

```
$ bundle exec bin/stud-finder ./my-rails-app

RANK  LANGUAGE  FILE                              SCORE  CLASS   FAN_IN  FAN_OUT  COMPLEXITY  CHURN_COMMITS  MAX_COUPLING  COUPLING_PARTNERS  COVERAGE
1     ruby      app/models/proficiency.rb         0.91   trunk   223     4        85          11             0.62          3                  0.99
2     ruby      app/services/payment_service.rb   0.84   trunk   78      12       91          42             0.71          5                  0.22
3     ruby      app/controllers/orders_controller 0.73   branch  61      9        65          74             0.48          2                  0.31
4     js        src/components/Dashboard.tsx      0.68   branch  44      18       56          18             0.00          0                  —
...
```

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

# With coverage signals (5-factor scoring)
bundle exec bin/stud-finder ./my-rails-app \
  --ruby-coverage ./coverage/resultset.json \
  --js-coverage ./coverage/lcov.info
```

---

## The Five Signals

Each file is scored on up to five independently measured signals. See [PRODUCT.md](PRODUCT.md) for the full theory and weighting math.

| Signal | What it measures | Weight |
|--------|------------------|--------|
| **fan_in** | How many other files depend on this one (blast radius) | 25% |
| **fan_out** | How many other files this one depends on (its own coupling burden) | 10% |
| **complexity** | Cyclomatic complexity of the hardest method in the file | 25% |
| **churn** | Commit frequency + line volume over a 180-day window | 25% |
| **coverage** | Inverse of line coverage (lower coverage = higher risk) | 15% |

When coverage isn't available, the remaining four signals (fan_in, fan_out, complexity, churn) re-normalize to 100% automatically (4-factor mode).

### Informational columns (not scored)

These ride alongside the score to give reviewers extra context, but do not contribute to it:

- **instability** / **instability_pct** — `fan_out / (fan_in + fan_out)`, and its percentile rank across the repo. High instability = depends on a lot while little depends on it.
- **max_coupling** / **max_coupling_partner** / **coupling_partners** / **coupling_pct** — temporal coupling from git history: the strongest co-change ratio with any partner file, the path of that strongest partner, how many partners cross the threshold, and the percentile rank of `max_coupling`. The analysis produces co-change pairs; each file's row keeps the strongest pair's ratio (`max_coupling`), that partner's path (`max_coupling_partner`), and the count of pairs (`coupling_partners`). On ties the strongest partner is chosen deterministically: highest coupling, then highest co-change count, then alphabetical path; `max_coupling_partner` is an empty string when a file has no qualifying partners. Computed once over the full file set in the main scan (one extra `git log` pass), so cross-language co-change is captured. By default, commits touching more than 50 scored files are skipped as bulk commits; use `--coupling-max-commit-files 0` for unlimited/legacy behavior. Same thresholds as the `edges` subcommand (`--coupling-threshold`, `--coupling-min-commits`).

Files are classified into three labels based on their **fan_in percentile** (not the total score):

- **trunk** — fan_in in the top 15% (default `trunk_threshold: 85`). Load-bearing. High review bar, change with care.
- **branch** — fan_in between the 50th and 85th percentile (default `branch_threshold: 50`). Meaningful coupling.
- **leaf** — everything below the 50th percentile. Isolated. Move fast here.

The total score still drives the ranking. The class label is a separate coupling-based signal.

### Newness rules (AI-generated code)

History-based signals can under-protect brand-new files: a fresh AI-generated file may have little churn, low fan-in, and no established blast radius yet, even though it is often the least proven code in the change. Stud Finder therefore applies post-scoring newness rules that change only `class`, `new_file`, `age_days`, and `escalation`; the numeric `score` stays honest and unchanged.

A file is considered new when its first commit is within `--new-file-days` days (default 30), or when it has fewer than `--new-file-min-commits` commits in full git history (default 3). New files cannot classify below `branch`; those rows show `escalation=recency_floor`.

A stronger rule runs first: if a new file depends on a structurally `trunk` file through its fan-out edges, it escalates to `trunk` with `escalation=trunk_adjacent`. This highlights new code consuming critical interfaces, where contract-violation risk is highest. Use `--no-newness` to disable both newness rules.

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

Stud Finder first runs dependency-cruiser with the target project's normal config so path aliases,
TypeScript config, and bundler resolution can be honored. If that fails because no usable
configuration is available, it retries once with `--no-config` and reports
`js_depcruise_no_config`. The fallback keeps analysis running, but aliases such as
`tsconfig` paths and webpack aliases will not resolve, so JS/TS `fan_in` may be undercounted.
For alias-heavy TypeScript projects, run `npx depcruise --init` in the target repo for accurate
results.

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
| `--no-rails-inference` | Disable Rails association/string fan-in inference |
| `--churn-days N` | Commit lookback window in days (default: 180). Churn uses git rename detection; within the window, rename commits are attributed to the new path when git pairs the rename. History before that paired rename stays with the old path unless it is also paired within the window. |
| `--weights WEIGHTS` | Custom weights as fractions, e.g. `fan_in:0.25,fan_out:0.10,complexity:0.25,churn:0.25,coverage:0.15`. Defaults shown. All five keys are required. |
| `--trunk-threshold N` | fan_in percentile cutoff for trunk classification (default: 85) |
| `--branch-threshold N` | fan_in percentile cutoff for branch classification (default: 50) |
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
- **[VISION.md](VISION.md)** — project vision and positioning

---

## License

MIT. See [LICENSE](LICENSE).
