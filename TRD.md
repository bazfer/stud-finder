# stud-finder — Technical Requirements Document

## Overview

`stud-finder` is a Ruby gem CLI that analyzes a Rails codebase and outputs a ranked file risk list. It combines static coupling, complexity, churn, and optionally coverage into a composite risk score per file. It also provides a dependency edge explorer (`stud-finder edges`) for blast-radius analysis.

**Repo:** `CovalentNetworks/stud-finder` (source) · `bazfer/stud-finder` (fork)

**Current state — Ruby + JavaScript analysis active:**

| Signal | Default weight | Source |
|--------|---------------|--------|
| fan_in | 0.35 | rubocop-ast (Ruby) / dependency-cruiser (JS) static analysis |
| complexity | 0.25 | rubocop Metrics/CyclomaticComplexity (Ruby) / eslint complexity rule (JS) |
| churn | 0.25 | git log commit + line frequency |
| coverage | 0.15 | SimpleCov / lcov / cobertura report (optional) |

**Score formula (4-factor, when coverage is available):**
```
risk_score = w_fan_in × fan_in_pct + w_complexity × complexity_pct + w_churn × churn_pct + w_coverage × (1 − coverage_fraction)
```

**Score formula (3-factor, no coverage — default):**
The coverage weight is dropped and the remaining three weights are renormalized to sum to 1.0:
```
w′_x = w_x / (w_fan_in + w_complexity + w_churn)
risk_score = w′_fan_in × fan_in_pct + w′_complexity × complexity_pct + w′_churn × churn_pct
```
Default renormalized: fan_in ≈ 0.41, complexity ≈ 0.29, churn ≈ 0.29.

All percentile inputs are normalized to [0.0, 1.0]. Result: [0.0, 1.0], rounded to 4 decimal places.

**Churn signal** is a composite of commit count percentile and changed-lines percentile, weighted 50/50:
```
churn_pct[file] = 0.5 × commit_count_pct[file] + 0.5 × changed_lines_pct[file]
```

**Additional per-file metrics (not in score formula):**
- `fan_out` — efferent coupling: how many files in the scored set this file depends on
- `instability` — Robert Martin's I metric: `fan_out / (fan_in + fan_out)`. Range [0.0, 1.0]. 0.0 = maximally stable; 1.0 = maximally unstable. Isolated files (fan_in = fan_out = 0) get 0.0.

---

## CLI Interface

### Main command

```
stud-finder [PATH] [OPTIONS]
```

**Arguments:**
- `PATH` — path to the repository root. Defaults to `.` (current directory).

**Options:**

```
--output table|json|markdown|csv
    Output format. Default: table.

--churn-days N
    Commit lookback window in days. Default: 180.

--weights fan_in:F,complexity:C,churn:H,coverage:V
    Override all four weights. Values are floats in [0.0, 1.0].
    All four keys must be present. Values must sum to 1.0 (±0.001 tolerance).
    When no coverage data is provided, coverage must be 0.0 — non-zero
    exits 1 with: "Error: coverage weight must be 0.0 when no coverage data is provided."
    Example (no coverage): --weights fan_in:0.5,complexity:0.3,churn:0.2,coverage:0.0
    Example (with coverage): --weights fan_in:0.4,complexity:0.2,churn:0.2,coverage:0.2

--ruby-coverage PATH
    Path to a Ruby coverage report (.xml cobertura, .info lcov, .json resultset).

--js-coverage PATH
    Path to a JavaScript coverage report.

--coverage PATH
    Deprecated alias for --ruby-coverage. Emits warning.

--js-timeout N
    dependency-cruiser and eslint subprocess timeout in seconds. Default: 60.

--trunk-threshold N
    fan_in percentile cutoff for trunk classification (integer, 1–99). Default: 85.

--branch-threshold N
    fan_in percentile cutoff for branch classification (integer, 1–99). Default: 50.
    Must be strictly less than trunk-threshold — exit 1 if violated.

--exclude PATTERN
    File glob pattern (repeatable). Evaluated via File.fnmatch with
    File::FNM_PATHNAME | File::FNM_DOTMATCH against each file's path relative
    to PATH (no leading ./).

--min-files N
    Minimum file count before advisory warning. Default: 20.

--top N
    Emit only the top N results.

--diff-base REF
    Score the full repo but emit only files changed vs REF (merge-base).
    Example: --diff-base origin/staging. Mutually exclusive with --only.

--only PATHS
    Emit only these comma-separated repo-relative paths (full repo still scored —
    fan_in counts and percentiles are unaffected). Mutually exclusive with --diff-base.

--verbose
    Print suppressed per-file warnings to stderr.

--version
    Print gem version and exit.

--help
    Print help and exit.
```

### Edges subcommand

```
stud-finder edges FILE [PATH]
```

Shows the full dependency edge list for a single file — its dependents (blast radius), its dependencies (fan-out sources), and its temporal coupling partners (co-change partners from git history).

- `FILE` — repo-relative path to the file (e.g. `app/models/user.rb`)
- `PATH` — repo root. Defaults to `.`

Exits 1 if FILE is not in the scored set, or if FILE is omitted.

---

## Default Excludes

Always applied, cannot be disabled:

- `db/schema.rb`
- `db/migrate/**`
- `vendor/**`
- `**/node_modules/**`
- `**/*.min.js`
- `tmp/**`
- `log/**`
- Files whose first non-blank line matches `/\A\s*#\s*This file is auto-generated/i`

---

## Scoring Pipeline

### Step 1 — File Discovery

Walk `PATH` recursively. Collect `.rb`, `.js`, `.ts`, `.jsx`, `.tsx` files. Apply default excludes, then `--exclude` patterns. Partition into Ruby files and JS/TS files by extension.

Glob matching: `File.fnmatch(pattern, relative_path, File::FNM_PATHNAME | File::FNM_DOTMATCH)`. `relative_path` is relative to `PATH` with no leading `./`.

**Post-collection checks:**
- `files.length < 5` → exit 1: `"Error: only N files found after excludes. Too few for meaningful analysis."`
- `files.length < min_files` → stderr warning, continue.

### Step 2 — fan_in + fan_out via rubocop-ast (Ruby)

#### 2a. Constant ownership

Each Ruby file owns one constant — its **primary constant**. Determination order:

1. **Zeitwerk path mapping:** for files under `app/`, `lib/`, or `test/`, derive from file path:
   - Strip leading segment up to and including `app/`, `lib/`, or `test/`
   - Strip the `concerns` segment if present
   - Strip `.rb` extension
   - Split on `/`, CamelCase each segment, join with `::`
2. **AST fallback:** parse with rubocop-ast, find the first top-level `class` or `module` node (not nested inside another class/module). Use its resolved constant name.
3. **No ownership:** files outside app/lib/test with no top-level class/module get `fan_in = 0`.

#### 2b. Reference scanning

For each file, parse with rubocop-ast. Walk all `const` nodes. Resolve fully-qualified names via `cbase` nodes. Build `references[file] = Set<String>`.

#### 2c. fan_in and fan_out

```
fan_in[file]  = count of files f where f != file AND constant_for[file] ∈ references[f]
fan_out[file] = count of files f where f != file AND constant_for[f] ∈ references[file]
```

#### 2d. Dependency edges

For each file, record:
```ruby
edges[file] = {
  dependents:   [files that reference this file's constant],
  dependencies: [files whose constants this file references]
}
```

This is the static graph used by `stud-finder edges`.

**Known limitation:** dynamic references (`Object.const_get`, `send`, metaprogramming) are not detected. fan_in and fan_out are undercounted for files with heavy metaprogramming. Noted in output footers.

### Step 3 — fan_in + fan_out via dependency-cruiser (JavaScript)

Run as subprocess:
```bash
npx --no dependency-cruiser --output-type json --no-config <files...>
```

Parses the JSON dependency graph. Computes fan_in and fan_out from the module import graph. Builds the same `edges` structure as Step 2d.

**Graceful degradation:** if `npx` or `dependency-cruiser` is not available (timeout, not found, non-zero exit), emit warning `js_tools_missing` to stderr, set fan_in = fan_out = 0 for all JS files, continue. Do not exit 1.

### Step 4 — Complexity (Ruby)

Run as subprocess:
```bash
rubocop --no-config --only Metrics/CyclomaticComplexity --format json <PATH>
```

`--no-config` is mandatory. Sum all method scores per file. Files with no methods get `complexity = 0`.

**Parse errors:** skip file, emit to stderr, continue.

**rubocop not in PATH:** exit 1 with install instructions.

### Step 5 — Complexity (JavaScript)

Run eslint subprocess with the complexity rule. Same graceful degradation as dependency-cruiser.

### Step 6 — Churn via git log

Run as subprocess:
```bash
git -C <PATH> log \
  --since="<N> days ago" \
  --diff-filter=ACDMR \
  --name-only \
  --format=tformat: \
  -z
```

Flags: `--diff-filter=ACDMR` prevents double-counting renames. `--format=tformat:` suppresses commit headers. `-z` NUL-delimits filenames for correct parsing of names with spaces.

Build `churn_commits[file]` (commit count) and `churn_lines[file]` (total changed lines).

**Zero-inflation check:** if `>50%` of files have zero churn, warn to stderr:
```
Warning: X% of files have zero churn in the last N days. Churn signal is weak. Consider --churn-days to widen the window.
```

### Step 7 — Temporal Coupling via git log (M6)

Run as subprocess:
```bash
git -C <PATH> log \
  --since="<N> days ago" \
  --diff-filter=ACDMR \
  --name-only \
  --format="%H"
```

This yields commit SHAs as delimiters followed by filenames, grouped per commit.

**Algorithm:**

1. Parse into commits: each commit is a SHA line followed by filenames until the next SHA or EOF.
2. For each commit, collect the set of scored files that changed (filter to `files[]`).
3. For each pair `(A, B)` in that set: `co_changes[A][B] += 1`.
4. Compute `changes[file]` = total commits where this file appeared (same as `churn_commits` — reuse).

**Coupling formula:**
```
coupling(A, B) = co_changes(A, B) / min(changes(A), changes(B))
```

Range: [0.0, 1.0]. 1.0 = A and B always change together relative to the less-active file.

**Filtering:** only emit pairs where `co_changes(A, B) >= coupling_min_commits` (default: 5) AND `coupling(A, B) >= coupling_threshold` (default: 0.3). This suppresses noise from rare coincidental co-edits.

**Output:** temporal coupling is surfaced only in `stud-finder edges`. The main ranked table is unchanged — coupling is additive context, not a score signal.

**CLI options for M6:**
```
--coupling-threshold FLOAT
    Minimum coupling ratio to show in edges output. Default: 0.3.

--coupling-min-commits N
    Minimum co-change count required to show a pair. Default: 5.
```

### Step 8 — Normalization

For each signal `s` in `{fan_in, complexity, churn}`:

Lower-bound percentile:
```
pct[file] = count(v in values where v < raw[file]) / (|files| - 1)
```

Ties receive the same rank. Result: [0.0, 1.0].

Edge cases: `|files| == 1` → 0.0. All values equal → 0.0 for all.

Coverage is used directly as `(1.0 − coverage_fraction)`, not percentile-ranked.

### Step 9 — Composite Score

See Overview formula. Result [0.0, 1.0], 4 decimal places.

### Step 10 — Classification

Based on `fan_in_pct` only:
```
fan_in_pct >= trunk_threshold / 100.0  → trunk
fan_in_pct >= branch_threshold / 100.0 → branch
otherwise                               → leaf
```

Defaults: trunk=85, branch=50.

### Step 11 — Sort and Emit

Sort by `risk_score` descending (stable — ties preserve discovery order). Apply `--top N`. Pass to reporter.

---

## Output Formats

### Table (default)

```
stud-finder — /path/to/repo (180-day churn, 3-factor score)
Note: coverage data not available. Score uses fan_in 0.41, complexity 0.29, churn 0.29.
Filtered to --only paths (ranks are against the full repo).

Ruby
 rank  language    file                                            score  class   fan_in  fan_out  instability  complexity  churn_commits  churn_lines  churn_pct  coverage
    1  ruby        app/models/user.rb                             0.9774  trunk       88       15       0.1456          31             22          841     0.9898       n/a
    2  ruby        app/models/employee.rb                         0.9465  trunk      179       12       0.0628           0             16          262     0.9831       n/a

JavaScript/TypeScript
 rank  language    file                                            score  class   fan_in  fan_out  instability  complexity  churn_commits  churn_lines  churn_pct  coverage
   27  javascript  app/javascript/components/ObjectiveView/api/objectiveViewApi.js  0.5494  leaf  0  0  0.0000  5  11  212  0.9658  n/a

4088 files analyzed. 2 files skipped (parse errors). 116748 files excluded by default rules.
fan_in is a static approximation — dynamic references (const_get, send, metaprogramming) not counted.
```

Columns: `rank`, `language`, `file` (relative path), `score` (4dp), `class`, `fan_in`, `fan_out`, `instability` (4dp), `complexity`, `churn_commits`, `churn_lines`, `churn_pct` (4dp), `coverage`.

### Edges output

```
stud-finder edges — app/models/user.rb

  score: 0.9774  class: trunk   fan_in: 88  fan_out: 15  instability: 0.1456

  ── Dependents (88 files) (files that depend on this file — blast radius) ──

  rank  file                                                 score  class   fan_in  fan_out  instability
     1  app/models/proficiency.rb                           0.9774  trunk       88       15       0.1456
     2  app/interactors/proficiency_workflow/create.rb      0.8923  branch       3       13       0.8125
     ...

  ── Dependencies (15 files) (files this file depends on — fan-out) ──

  rank  file                                                 score  class   fan_in  fan_out  instability
     1  app/models/application_record.rb                    0.7951  trunk      305        2       0.0065
     ...

  ── Temporal Coupling (180-day window, min 5 co-changes, threshold 0.30) ──

  coupling  co_changes  file
    0.8750          14  app/controllers/users_controller.rb
    0.6667           8  app/services/auth/token_service.rb
    0.4000           6  app/models/role.rb

  (none) if no pairs meet the threshold.

Edges are statically computed — dynamic references not counted.
Temporal coupling from git history (180-day window).
```

Temporal coupling rows are sorted by `coupling` descending. No row cap (unlike static edges which cap at 50).

### JSON

**Updated schema — normative:**

```json
{
  "meta": {
    "repo": "<string>",
    "analyzed_at": "<ISO 8601 UTC>",
    "churn_days": "<integer>",
    "file_count": "<integer>",
    "files_skipped": "<integer>",
    "formula": "<'3-factor (no coverage)' | '4-factor'>",
    "weights": {
      "fan_in": "<float>",
      "complexity": "<float>",
      "churn": "<float>",
      "coverage": "<float | null>"
    },
    "warnings": ["<string>"]
  },
  "warnings": ["<string>"],
  "ruby": [
    {
      "rank": "<integer>",
      "language": "ruby",
      "path": "<string>",
      "score": "<float — 4dp>",
      "class": "<'trunk' | 'branch' | 'leaf'>",
      "fan_in": "<integer>",
      "fan_in_pct": "<float — 4dp>",
      "fan_out": "<integer>",
      "instability": "<float — 4dp>",
      "complexity": "<integer>",
      "complexity_pct": "<float — 4dp>",
      "churn_commits": "<integer>",
      "churn_lines": "<integer>",
      "churn_pct": "<float — 4dp>",
      "coverage": "<float | null>"
    }
  ],
  "javascript": [ "<same shape>" ]
}
```

**Warning codes:**

| Code | Condition |
|------|-----------|
| `coverage_unavailable` | No coverage data (always present when no --ruby-coverage / --js-coverage) |
| `zero_churn_majority` | >50% of files have zero churn in window |
| `small_repo` | file_count < min_files |
| `files_skipped` | One or more files dropped due to parse errors |
| `js_tools_missing` | dependency-cruiser or eslint not available |
| `coverage_flag_deprecated` | --coverage used instead of --ruby-coverage |
| `diff_filter_empty` | --diff-base produced no changed files |

### Markdown

```markdown
## stud-finder — 2026-06-03

> 3-factor score (no coverage). Churn window: 180 days. 4088 files analyzed.

### Ruby

| rank | language | file | score | class | fan_in | fan_out | instability | complexity | churn_commits | churn_lines | churn_pct | coverage |
|------|----------|------|-------|-------|--------|---------|-------------|------------|---------------|-------------|-----------|----------|
| 1 | ruby | app/models/user.rb | 0.9774 | trunk | 88 | 15 | 0.1456 | 31 | 22 | 841 | 0.9898 | n/a |

### JavaScript/TypeScript

...

*fan_in is a static approximation — dynamic references not counted.*
```

### CSV

Columns in order: `rank`, `language`, `file`, `score`, `class`, `fan_in`, `fan_in_pct`, `fan_out`, `instability`, `complexity`, `complexity_pct`, `churn_commits`, `churn_lines`, `churn_pct`, `coverage`.

---

## Error Handling

| Condition | Exit code | Behavior |
|-----------|-----------|----------|
| `PATH` does not exist | 1 | Clear error message |
| `PATH` is not a git repository | 1 | Clear error message |
| `rubocop` not in PATH | 1 | Error + install instructions |
| `git` not in PATH | 1 | Clear error message |
| File count < 5 (post-exclude) | 1 | "Too few files" error |
| File count < min_files | 0 | Warning to stderr, continue |
| >50% zero-churn files | 0 | Warning to stderr, continue |
| `--weights` values don't sum to 1.0 | 1 | Validation error with actual sum |
| `--weights coverage:N` where N > 0 and no coverage data | 1 | Validation error |
| `--branch-threshold >= --trunk-threshold` | 1 | Validation error |
| `--diff-base` and `--only` both set | 1 | Validation error |
| `--diff-base REF` where REF doesn't exist | 1 | Validation error |
| RuboCop parse error on a file | 0 | Skip file, log to stderr, continue |
| AST parse error on a file | 0 | Skip file, log to stderr, continue |
| git subprocess non-zero exit | 1 | Propagate stderr, exit 1 |
| dependency-cruiser not available | 0 | Warning, zero JS fan_in/fan_out, continue |
| eslint not available | 0 | Warning, zero JS complexity, continue |
| `stud-finder edges FILE` — FILE not in scored set | 1 | Error to stderr |
| `stud-finder edges` — no FILE given | 1 | Usage to stderr |

---

## Gem Structure

```
stud-finder/
├── bin/
│   └── stud-finder
├── lib/
│   └── stud_finder/
│       ├── version.rb
│       ├── cli.rb                    # Argument parsing, subcommand routing, emit
│       ├── file_collector.rb         # File discovery, glob excludes
│       ├── fan_in.rb                 # Ruby: rubocop-ast constant ownership + reference graph
│       ├── js_fan_in.rb              # JS: dependency-cruiser graph parsing
│       ├── complexity.rb             # Ruby: rubocop subprocess, per-file aggregation
│       ├── js_complexity.rb          # JS: eslint subprocess, per-file aggregation
│       ├── churn.rb                  # git log, NUL-delimited, commit + line counts
│       ├── temporal_coupling.rb      # git log co-change matrix, coupling ratio computation
│       ├── normalizer.rb             # Lower-bound percentile rank
│       ├── scorer.rb                 # Weight application, renormalization, instability, classification
│       ├── edges.rb                  # stud-finder edges subcommand: dependents, dependencies, coupling
│       ├── diff.rb                   # --diff-base: merge-base changed paths
│       └── coverage/
│           ├── detector.rb
│           ├── cobertura.rb
│           ├── lcov.rb
│           └── resultset.rb
├── spec/
│   └── stud_finder/
│       ├── fan_in_spec.rb
│       ├── js_fan_in_spec.rb
│       ├── complexity_spec.rb
│       ├── churn_spec.rb
│       ├── temporal_coupling_spec.rb
│       ├── normalizer_spec.rb
│       ├── scorer_spec.rb
│       ├── edges_spec.rb
│       └── fixture_js_repo_spec.rb
├── spec/fixtures/
│   └── sample_app/
├── stud-finder.gemspec
├── Gemfile
├── README.md
└── TRD.md
```

---

## Dependencies

**Runtime:**
- `rubocop-ast` — AST parsing for fan_in
- `rubocop` — complexity analysis; must be in PATH

**Development:**
- `rspec`
- `rubocop`

**External binaries (must be in PATH at runtime):**
- `git` — always required
- `rubocop` — always required
- `npx dependency-cruiser` — JS analysis; graceful degradation if absent
- `npx eslint` — JS complexity; graceful degradation if absent

---

## Testing Requirements

### Unit tests

- **`fan_in_spec.rb`** — Zeitwerk mapping, concerns stripping, nested namespace, AST fallback, fan_in and fan_out counting, edge graph construction
- **`churn_spec.rb`** — NUL-delimited parsing, filenames with spaces, rename deduplication, zero-inflation threshold, commit + line counts
- **`temporal_coupling_spec.rb`** — commit grouping from `%H`-formatted git log, co-change matrix construction, coupling formula `co_changes / min(changes_A, changes_B)`, min_commits threshold filter, coupling_threshold filter, empty result when no pairs meet threshold
- **`normalizer_spec.rb`** — lower-bound percentile, tie handling, single-file clamp (0.0), all-same-value (all 0.0)
- **`scorer_spec.rb`** — 3-factor renormalization, 4-factor formula, instability formula (isolated → 0.0), classification thresholds
- **`edges_spec.rb`** — known file output, sort dependents by score desc, header metrics, error for missing file, usage for nil target, none-in-scored-set, temporal coupling section present with mock data, temporal coupling "(none)" when below threshold

### Integration test

`spec/fixture_js_repo_spec.rb`: run against fixture. Assert top file has highest artificial fan_in, all scores in [0.0, 1.0], JSON matches schema, exit 0.

---

## M6 — Temporal Coupling: Implementation Spec

### `TemporalCoupling` class

**Location:** `lib/stud_finder/temporal_coupling.rb`

**Interface:**
```ruby
result = TemporalCoupling.new(
  repo_path:          path,
  files:              files,          # scored file paths (filter applied inside)
  days:               churn_days,
  min_co_changes:     5,
  coupling_threshold: 0.30
).call
# result.pairs => Hash: { file_path => [{path:, coupling:, co_changes:, own_changes:}, ...] }
# result.warnings => Array<String>
```

**git command:**
```bash
git -C <repo_path> log \
  --since="<days> days ago" \
  --diff-filter=ACDMR \
  --name-only \
  --format="%H"
```

Output structure: SHA line, blank line, file names (one per line), blank line, next SHA, ... Parse into commit groups.

**Algorithm:**
1. Collect commit groups: for each group, filter filenames to those in `files[]` → `changed_in_commit[]`
2. For each commit group, for each ordered pair (A, B) where A < B (lexicographically): `co_changes[A][B] += 1`
3. `own_changes[file]` = number of commits where file appeared (equivalent to `churn_commits` — computed independently here for self-containment)
4. For each pair (A, B) where `co_changes[A][B] >= min_co_changes`:
   - `coupling = co_changes[A][B].to_f / [own_changes[A], own_changes[B]].min`
   - If `coupling >= coupling_threshold`: add to result for both A and B
5. Sort each file's partner list by `coupling` descending

**Result shape per file:**
```ruby
[
  { path: "app/models/role.rb", coupling: 0.8750, co_changes: 14, own_changes: 16 },
  ...
]
```

Empty array if no partners meet threshold.

### Integration into `stud-finder edges`

`CLI#run_edges` computes a `TemporalCoupling` result using the same `--churn-days` window plus the two new options, then passes it to `Edges`.

`Edges` receives a new `coupling:` keyword argument (hash: `{ file => [{path:, coupling:, co_changes:, own_changes:}, ...] }`).

After the Dependencies section, emit:
```
  ── Temporal Coupling (N-day window, min M co-changes, threshold T) ──

  coupling  co_changes  file
    0.8750          14  app/controllers/users_controller.rb
    ...
```

Or if empty:
```
  ── Temporal Coupling (N-day window, min M co-changes, threshold T) ──

    (none above threshold)
```

### CLI changes

Add to `option_parser`:
```
--coupling-threshold FLOAT   Minimum coupling ratio for edges output (default: 0.30)
--coupling-min-commits N     Minimum co-change count for edges output (default: 5)
```

Add to `DEFAULT_OPTIONS`:
```ruby
coupling_threshold:   0.30,
coupling_min_commits: 5
```

Pass through `run_edges`.

---

## Acceptance Criteria (M6)

- [ ] `stud-finder edges FILE` shows a "Temporal Coupling" section
- [ ] Coupling formula is `co_changes / min(own_changes_A, own_changes_B)`
- [ ] Pairs below `--coupling-min-commits` are suppressed
- [ ] Pairs below `--coupling-threshold` are suppressed
- [ ] Partners sorted by coupling descending
- [ ] Section shows "(none above threshold)" when empty
- [ ] `--coupling-threshold` and `--coupling-min-commits` flags accepted and passed through
- [ ] `TemporalCoupling` unit tests cover: grouping, formula, both filters, empty result
- [ ] `Edges` unit tests cover: coupling section present, coupling "(none)" case
- [ ] Main ranked table output is unchanged (no coupling column added)
- [ ] Score formula unchanged
