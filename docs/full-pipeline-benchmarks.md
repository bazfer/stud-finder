# Stud Finder: corrected full-pipeline results

Verified on 18 September 2026 against upstream `fbfdd201c3901eb8b28259affa87f323c43c50ef`.

**All ten repositories now match the original reports for full scans, diffs, a Ruby edit, and restoration.** The corrected implementation also improves warm full-pipeline performance further. GitLab falls from the previous optimized 7.75 seconds to 3.88 seconds. Two smaller repositories have warm full scans below 500 ms. Two-digit-millisecond full-pipeline updates have not been achieved.

## Correctness defect and repair

An intermediate version of this change embedded RuboCop in the running process, where it inferred Ruby 3.2 from the host working directory. The original command ran inside each application and inferred that application's Ruby version. This changed how newer syntax was parsed and scored. Gumroad uses Ruby 3.4; OpenProject uses Ruby 4.0. The previous comparison found 52 shared-file complexity differences and one omitted OpenProject file across five applications. Those values also changed rankings for other files.

The fix resolves the effective Ruby target in the repository context and passes it explicitly to embedded RuboCop. The target is part of the cache namespace and snapshot verification: changing it invalidates cached complexity, and changing it during a scan forces a retry. This preserves the original tool's configuration behavior without changing the process working directory or importing the application's RuboCop rules.

Report equivalence is evidence of compatibility with upstream, not independent proof that upstream discovers every runtime dependency or computes perfect semantic impact.

## Entire-pipeline timings

All values are seconds. “Previous warm” is the earlier experimental resident-session measurement (before the parser-target fix); “corrected warm” is this build. Diff includes the available analysis pipeline and report generation. Edit changes one Ruby file and produces the full updated report.

| Application | Previous warm full | Corrected warm full | Corrected diff | After Ruby edit |
| --- | ---: | ---: | ---: | ---: |
| GitLab | 7.747 | 3.876 | 3.909 | 3.949 |
| Canvas | 19.040 | 17.456 | 17.441 | 17.743 |
| Discourse | 3.036 | 1.840 | 1.876 | 1.956 |
| OpenProject | 3.377 | 1.852 | 2.020 | 1.840 |
| Zammad | 1.405 | 0.841 | 0.942 | 0.897 |
| Katello | 1.069 | 0.725 | 0.707 | 0.749 |
| CartoDB | 1.063 | 0.533 | 0.596 | 0.601 |
| Coursemology | 0.754 | 0.484 | 0.646 | 0.551 |
| Gumroad | 0.836 | 0.569 | 0.657 | 0.896 |
| Mastodon | 0.816 | 0.468 | 0.514 | 0.506 |

These are resident-session timings, not fresh CLI startup times. Eight applications have three warm samples; Canvas and Discourse have two. Warm and diff columns show medians; edit is one observation. The first session restored existing caches and must not be interpreted as an empty-cache indexing benchmark. The machine was not otherwise controlled for host activity, so these are observations rather than latency guarantees or p95 estimates.

The original GitLab full scan took approximately 100.84 seconds in the earlier benchmark. The current 3.88-second result is approximately 26 times faster, while still far from an instant editor update.

### Sleep correction

The initial overnight replay overlapped macOS maintenance sleep. Canvas and some Discourse observations included hundreds of seconds asleep. System sleep logs confirmed this. Both applications were rerun while a scoped sleep inhibitor kept the benchmark awake; all corresponding parity checks passed. The initial observations remain in the raw evidence, alongside the awake rerun. They were excluded from the performance table because they do not represent active execution time.

## Further optimizations implemented

Parsed Git history is now cached under the validated Git identity, selected file population, and implementation version. Newness and age are still recomputed from the current clock. Git identity changes invalidate the derived history. In a GitLab phase profile, repeated Ruby and JavaScript history work fell from approximately 1.98 seconds to 0.015 seconds.

A resident directory inventory reuses unchanged directory entries using fresh directory identity and timestamp checks. Additions, removals and renames invalidate the relevant inventory. Source content and snapshot validation remain fresh; this is not a cache of old source text. Profiled GitLab discovery fell from approximately 1.06 seconds to 0.59 seconds.

A final GitLab profile took 3.95 seconds overall. Initial and final snapshots still cost approximately 1.17 seconds together, Ruby fan-in 0.62 seconds, and file discovery 0.59 seconds. These individual profiles are diagnostic samples, not the medians above. Remaining work includes repeated fingerprinting, dependency resolution, scoring and full report construction.

Canvas still spends approximately 15 seconds in JavaScript dependency analysis. Its existing configuration uses whole-graph rules and options outside the bounded incremental mode. Retaining the complete analysis preserves behavior; simply bypassing those rules would not satisfy parity.

## Verification and limits

- All ten full reports match the saved pristine upstream reports after the parser fix.
- All ten final full, diff, edited and restored reports match their upstream reference states. Edited states were checked against fresh runs of pristine upstream; the awake replay reused the matching edited-state goldens.
- Comparison fixes the report clock and ignores only `meta.analyzed_at`.
- 378 tests pass on both Ruby 4.0.5 and Ruby 3.3.12.
- 15 real Node integration scenarios pass.
- RuboCop checks 77 files with zero offenses.
- Regression tests cover Ruby target changes, newer syntax, target changes during a scan, history invalidation and clock changes, and directory/source-header mutations.

The application benchmarks ran in isolated copies; the original applications were not edited. Temporary edits in the benchmark copies were restored. Coverage reports were absent, and recent Git activity was sparse in these archived snapshots. Application-specific JavaScript dependency installations were not complete in the large benchmark harness. Matching upstream under those conditions does not establish complete JavaScript alias or runtime dependency resolution.

The benchmark is for Stud Finder's existing available pipeline. It does not establish method-level or line-level cascading impact, runtime call-graph completeness, or correctness of an editor's unsaved buffers.

## Toward two-digit milliseconds

Pierre provides useful ideas about indexed data, invalidating only affected structures, and serving slices of a larger result. Its published rendering speedups are not measurements of this semantic-analysis pipeline. The [Pierre investigation](pierre-performance-notes.md) links the primary sources and separates applicable techniques from their original workloads.

The next substantial step is a resident, versioned analysis engine that accepts explicit file updates, maintains reverse dependencies and global scoring aggregates, and exposes separate update, query and full-export operations. It needs a defined freshness contract, recovery from missed filesystem events, configuration and Git invalidation, and tests against fresh analysis after each mutation. A small impact query may then be much cheaper than reconstructing and serializing every report row. A complete report must still account for global rank changes and output size.

That engine and API are proposed, not implemented in this build. This iteration establishes a corrected baseline and identifies the remaining costs; it does not claim that every full report can finish in 10–99 ms.

## Comparison with unmodified upstream

These original full/diff measurements are separate single observations from the preceding benchmark. The new columns reuse the resident samples above. This comparison includes amortizing initialization into a long-running process; it is not an apples-to-apples comparison of fresh CLI startup. No empty-cache cold timing is claimed for the final build.

| Application | Original full | Original diff | New warm full | New warm diff | After Ruby edit |
| --- | ---: | ---: | ---: | ---: | ---: |
| gitlabhq | 100.836 | 100.373 | 3.876 | 3.909 | 3.949 |
| canvas-lms | 68.203 | 62.300 | 17.456 | 17.441 | 17.743 |
| discourse | 46.627 | 43.392 | 1.840 | 1.876 | 1.956 |
| openproject | 42.535 | 44.459 | 1.852 | 2.020 | 1.840 |
| zammad | 18.228 | 18.494 | 0.841 | 0.942 | 0.897 |
| katello | 13.720 | 11.795 | 0.725 | 0.707 | 0.749 |
| cartodb | 14.643 | 13.936 | 0.533 | 0.596 | 0.601 |
| coursemology2 | 10.247 | 9.873 | 0.484 | 0.646 | 0.551 |
| gumroad | 10.921 | 10.648 | 0.569 | 0.657 | 0.896 |
| mastodon | 10.994 | 9.738 | 0.468 | 0.514 | 0.506 |

## Reproduction and evidence

[Raw timing samples, repository commit IDs, parity outcomes and sleep-affected observations](full-pipeline-results.json) accompany this report. The scripts in [benchmark/](../benchmark/) cover reference replay, a complete uncached-versus-session comparison and real JavaScript integration scenarios. See [PIPELINE.md](../PIPELINE.md#reproduce-measurements) for commands and tool prerequisites.

The large-suite replay used isolated copies at the recorded commits, `--no-auto-unshallow --js-timeout 120`, per-repository cache directories, and each recorded diff reference. Report comparisons fixed `Time.now` at `2026-09-17T12:00:00Z` and omitted only `meta.analyzed_at`. Each edit appended a comment followed by `::Object` to the recorded Ruby file, compared with a fresh pristine-upstream scan, then restored the file and compared again. A real filesystem edit is required: this is not a simulation using cached output.
