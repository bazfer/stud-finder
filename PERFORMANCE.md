# Historical reference-phase measurements

The measurements below describe the first implementation round. Full-pipeline sessions, complexity/LOC/history caching, directory pruning and source-change checks have since been added; see [PIPELINE.md](PIPELINE.md). Historical remaining-work notes are retained as the original investigation record, not the current feature list.

# Incremental analysis progress

This implementation starts from commit `fbfdd201c3901eb8b28259affa87f323c43c50ef`.
It is the first implementation round toward interactive analysis, not a complete
watch server or a promise of sub-500 ms scans.

## Implemented

- Replaced quadratic Ruby fan-in counting with reference frequencies and explicit
  self-reference subtraction. Preserved duplicate-ownership behavior.
- Reused each file's AST for fallback ownership and reference extraction, then
  released source/AST intermediates. The retained facts are unresolved candidates.
- Added opt-in `--cache-dir PATH`. Stored JSON facts use content fingerprints,
  relative paths, runtime/parser versions, implementation hashes and inference
  settings. Cache writes replace a complete file atomically; malformed entries
  fall back to extraction. Failed/incomplete extraction is not persisted.
- Re-resolve candidates against the current ownership set on every call. This
  handles additions/deletions and newly shadowing namespaces without parsing
  unchanged callers again. It intentionally avoids a resolved-graph cache yet.
- Added cache hit/miss statistics and a replay benchmark checking each result
  against clean analysis. Reduced the internal candidate representation from
  named JSON fields to compact arrays to reduce decoding/allocation overhead.
- Forwarded Ruby reference warnings into reports and fixed physical ESLint path
  mapping for symlinked repository roots.

## Measurements

`bundle exec ruby benchmark/incremental_references.rb 10000`

Synthetic corpus: 10,000 small Ruby files, each initially defining one method and
referencing five neighbors. Ten unchanged runs and ten one-file edits. Local
arm64 macOS, Ruby 4.0.5, locked RuboCop 1.88.1 and rubocop-ast 1.49.1.

| Ruby reference analysis | Measured time |
|---|---:|
| Fresh analysis, no fact cache | 1.569 s (one sample) |
| First run creating the cache | 1.562 s (one sample) |
| Unchanged run p50 / p95 | 277 / 312 ms |
| One-file edit p50 / p95 | 321 / 334 ms |

Each edit reused 9,999 facts and extracted one file. All replay results matched
fresh analysis, including dependency lists and counts. Timing includes cache
load, file reading/hashing, extraction, resolution, graph construction and cache
save. It excludes Ruby/library startup, file discovery, complexity, Git history,
coverage, scoring, report serialization and editor transport. A small synthetic
corpus is not representative evidence for every application. Ten samples are
exploratory measurements, not a production latency SLO.

The initial named-field cache run measured 449 ms warm and 564 ms edit p95. That
run overlapped a test suite; the compact-format run did not. Treat these as
iteration observations, not an isolated causal speedup measurement. A separate
1,000-file run of the initial format measured 37 ms warm / 41 ms edit p95.

## Correctness coverage

Regression cases cover unchanged and relocated workspaces, same-size edits with
unchanged mtime, added/deleted constants, namespace shadowing, renames, inference
settings, repeated calls on one instance, duplicate ownership/self references,
corrupt caches, extraction failures, atomic replacement failure, CLI report
parity and warnings, plus symlinked JS roots. Existing analysis specifications
remain the compatibility oracle.

## Remaining work and discoveries

1. Full CLI latency is still dominated by work outside this cache, particularly
   external complexity processes and history in large repositories. Add phase
   timings and benchmark a representative application before setting SLOs.
2. Cache complexity and history with their full inputs, then introduce resident
   workers. Source facts are deliberately separate from resolved graph state.
3. The current cache reloads its JSON and hashes all files each run. For larger
   repositories, move to a resident index and measured database/adjacency storage;
   this is still O(files + references), not O(changed files) overall.
4. Empty or unparsable ASTs are conservatively re-extracted rather than persisted.
   A future extraction-status model can distinguish valid empty source from
   incomplete/failed analysis.
5. There is no atomic workspace snapshot, editor-overlay input or watcher yet.
   The source state can change during a scan, as in the original CLI. Do not
   advertise a generation/freshness guarantee until those contracts exist.
6. File discovery still traverses excluded trees. Pruning needs an explicit
   decision about the existing excluded-file counts reported by the collector.
7. Method-level resolution, multiple definitions per file, test impact, typed
   cascade queries and language/framework provenance remain separate milestones.
8. CLI `edges` currently omits coupling from its scoring pass, unlike normal scan.
   Automatic unshallow occurs after some earlier history signals. Both remain
   recorded correctness issues for the shared orchestration/history work.
9. The JS symlink failure surfaced during verification: physical ESLint paths
   previously could leave relative file complexities at zero. It is fixed here;
   parse-error versus zero-complexity distinctions still need a richer status model.
10. CI snapshots should be disposable and restored only from appropriate trusted
    cache scopes. Concurrent writers can overwrite useful facts, reducing hit
    rate, but retained records still require matching content fingerprints.

Next acceptance milestone: a complete small-edit CLI run that matches a clean
run and reports separate phase timings, followed by snapshot-bound library
queries. Retain whole-population scoring until profiling proves it is a bottleneck.
