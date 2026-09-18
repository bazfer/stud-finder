# Complete-pipeline incremental mode

This is a local development implementation on top of Stud Finder 0.7.0, not a published release. It retains the complete scoring pipeline, including whole-population percentile ranks and cross-language temporal coupling. It does not add method-level call resolution or an editor extension.

## One-shot CLI and CI

```sh
bundle exec bin/stud-finder /path/to/repo \
  --cache-dir /path/to/cache \
  --exclude '.claude/**' --exclude '.agents/**' \
  --no-auto-unshallow --output json
```

Restore the cache directory in CI as a disposable optimization. Source facts and metrics validate contents, tool versions and implementation versions. Git data validates repository identity, HEAD, effective Git configuration, replacement refs and shallow/graft boundaries. Parsed per-file history is also cached under that identity and the selected file population; age/evidence fields are recomputed using the current time. Time-windowed history expires when a commit leaves the window. Diff comparisons reuse committed-tree results keyed by the freshly resolved base commit and current history identity, so a moving base branch invalidates the comparison. Ages, coverage, scoring, ranks and output filters are recalculated.

Git and JavaScript cache identities are conservative: moving the checkout or reinstalling dependencies can require rebuilding them. Fresh-process startup, dependency installation, CI cache transfer and a new Git commit have costs that a resident warm session does not. Do not treat the interactive latency measurements as CI startup guarantees.

With `--cache-dir`, directory traversal prunes fully excluded subtrees. Excluded-file diagnostic counts therefore omit files inside pruned directories; table output explicitly reports that those files were not counted. The selected/scored file set retains the configured exclusion semantics. Resident sessions retain directory inventories and validate directory inode/mtime/ctime before reusing them. Source headers and contents are still read; this inventory does not replace source validation. Git metadata directories are now excluded by default. Nested agent worktrees still require explicit exclusions.

## Persistent local session

Start one process and send one JSON object per line:

```sh
bundle exec bin/stud-finder serve /path/to/repo \
  --cache-dir /path/to/cache --no-auto-unshallow \
  --exclude '.claude/**' --exclude '.agents/**'
```

Requests:

```json
{}
{"args":["--diff-base","HEAD~1"]}
{"args":["--only","app/models/user.rb"]}
```

Each response contains `generation` and `result`, or `error`. `result` is the normal complete JSON report (filtered only after whole-repository scoring when requested). Diagnostics use stderr; stdout is JSON-lines. `--diff-base` retains the original committed-range semantics (`BASE...HEAD`); it is not a working-tree diff. Use a full request or `--only` to inspect saved, uncommitted edits. Requests are sequential. EOF shuts down the session and its Node worker. This is an on-demand process, not an automatic save watcher or a network service.

The generation token identifies the observed source/configuration/coverage files and HEAD, including filesystem change stamps. It is not a complete report cache key: request options, analyzer versions and wall-clock-dependent ages also affect reports. The tool reads saved files, not unsaved editor buffers.

Selected source contents, directory stamps, relevant configuration/coverage inputs and HEAD are checked for changes during analysis. A session retries up to three times before returning an error instead of a mixed-source report. A one-shot cached CLI returns status 2 if this check fails. This is change detection, not a filesystem transaction or an atomic snapshot of arbitrary executable configuration and external services. Restart a session after dependency/tool installation changes; dependency watchers also invalidate the loaded JavaScript backend and force a rebuild/fallback.

Ruby API:

```ruby
require 'stud_finder/session'

session = StudFinder::Session.new([
  '/path/to/repo', '--cache-dir', '/path/to/cache', '--no-auto-unshallow'
])
begin
  report = session.scan
  diff = session.scan(extra_args: ['--diff-base', 'HEAD~1'])
  source_generation = session.generation
ensure
  session.close
end
```

## JavaScript/TypeScript fast path

The resident adapter currently supports ESLint 9, dependency-cruiser 18, TypeScript and `@typescript-eslint/parser`. The tools must resolve from the repository, or from the normal PATH/NODE_PATH environment. On macOS, optional `fsevents` (at the root or under `frontend/node_modules`) avoids allocating a watcher per dependency file. If a supported backend or reliable dependency watch is unavailable, analysis falls back to the existing CLI tools. Missing tools still produce the existing missing-signal warnings.

Incremental dependency updates require a declarative `.dependency-cruiser.json` with only these `options`: `exclude.path`, `doNotFollow.path` and `tsConfig.fileName`. Leaf TypeScript configurations may provide aliases through `compilerOptions.paths` and `baseUrl`; config inheritance/references and paths escaping the supported analysis scope use full analysis. Other dependency-cruiser options, custom rules and executable configs also use full dependency analysis. They are not silently ignored.

An analysis configuration may live outside the application checkout:

```sh
bundle exec bin/stud-finder serve /path/to/repo \
  --js-config /path/to/analysis/dependency-cruiser.json \
  --cache-dir /path/to/cache --no-auto-unshallow
```

`--js-config` is also honored by the uncached backend, so a fresh scan remains a meaningful comparison. For tools such as Vite, copy the actual resolver aliases into an analysis TypeScript config when they are absent from the application's TypeScript config. Do not equate a successful tool exit with complete alias resolution.

For a supported profile, unchanged JavaScript modules reuse the resolved graph. An edit to an existing file re-cruises that file's direct imports and updates its outgoing edges. File additions, deletions, renames or resolver-configuration changes trigger a full graph rebuild. ESLint results are cached per file content. Source hashes are checked on every request. Sorted configuration-directory listings are retained in memory and invalidated by fresh inode/mtime/ctime checks; input contents are still hashed each time. Dependency installations are fingerprinted on worker startup and watched during a session; unusual live dependency edits and watcher delivery latency are reasons to explicitly restart after installation changes.

The Ruby reference engine still re-resolves cached facts across all current owners. Ruby complexity uses the same RuboCop runner as the existing subprocess for cache misses. Its Ruby target is discovered relative to the analyzed repository, not the host process: this includes RuboCop’s environment/gemspec/version-file/lockfile precedence. The effective target is part of the complexity cache identity and source-generation validation. This corrects the earlier embedded-runner mismatch on newer Ruby syntax. The uncached CLI remains the comparison path. Parse failures are not persisted as successful Ruby complexity facts.

## Reproduce measurements

```sh
bundle exec ruby benchmark/complete_pipeline.rb /path/to/repo /path/to/cache \
  --exclude '.claude/**' --exclude '.agents/**' \
  --js-config /path/to/analysis/dependency-cruiser.json
```

This read-only source benchmark compares a full uncached scan with the first session scan and five warm scans. It raises if the complete JSON reports differ except their timestamps. It prints timings and warnings as JSON. Run on a quiet checkout; deliberately editing it during the comparison changes the expected report.

Measure ordinary Ruby and TypeScript edits, additions/deletions, configuration changes and cache restoration separately. A small-edit result is not a latency bound for a large refactor or a global configuration change. Supply actual coverage reports when available; coverage ingestion and scoring run each time. No cached risk score substitutes for current input data.

Optional real-tool regression checks use an isolated temporary repository:

```sh
STUD_NODE_MODULES=/absolute/path/to/node_modules \
  bundle exec ruby benchmark/mixed_pipeline_parity.rb
```

That installation must contain the supported ESLint/dependency-cruiser/TypeScript/parser packages and optionally `fsevents` on macOS. `STUD_FRONTEND_NODE_MODULES` may point to another installation containing `fsevents`. The check covers import edits with preserved mtimes, aliases, dynamic/CommonJS imports, package metadata changes without source edits, renames, additions, deletions, coverage changes, corrupt caches and unsupported-profile fallback. It does not install packages or change an application checkout.

## Public-codebase results

See [full-pipeline benchmarks and correctness verification](docs/full-pipeline-benchmarks.md) for all ten application measurements and their limitations.
