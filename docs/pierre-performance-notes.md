# Pierre’s techniques and the next Stud Finder architecture

Inspected Pierre’s public source at commit `c5b1e58203fa5c59dd99353cbcac09ee653a24f6`, plus its rendering article and Code Storage description. This is an architectural investigation, not a benchmark showing Stud Finder runs in two-digit milliseconds.

## What Pierre actually accelerates

Pierre’s public Trees package is a file-tree interface. Its internal path store separates canonical paths from the visible view and favors reading a slice over constructing the complete list. The private engine is not presented as an independently installable public product. Its invariants are documented in the [path-store README](https://github.com/pierrecomputer/pierre/blob/c5b1e58203fa5c59dd99353cbcac09ee653a24f6/packages/path-store/README.md).

The diff-rendering article describes virtualization, cached position checkpoints, avoiding retained giant strings, reusing containers, shared options and deferred highlighting. It reports about an 80% reduction in parsing time for one large Linux diff and memory dropping from 2.4 GB to about 1.15 GB. Those measurements concern patch processing/rendering, not semantic impact analysis across a repository. The useful lesson is to measure individual costs and eliminate repeated work while preserving behavior. [On Rendering Diffs](https://pierre.computer/writing/on-rendering-diffs)

Code Storage describes distributed Git storage and repository operations. Its public page does not establish a two-digit-millisecond bound for Stud Finder’s metrics, dependency resolution or scoring. [Code Storage](https://code.storage/)

## Concrete ideas worth transferring

| Observed technique | Relevant Stud Finder application |
|---|---|
| Internal numeric node IDs; paths at the boundary | Keep file and symbol IDs in graph edges and metric columns; resolve paths when returning results. |
| Local mutations with explicit invalidation metadata | Apply edits to versioned facts and report which ownership, dependency and ranking inputs became invalid. |
| Prepared input reused across operations | Retain decoded facts, parsed history and directory inventories in the resident process. |
| Slice-first reads and lazy lookup structures | Offer impact queries, pages and changed-row responses alongside complete report export. |
| Aggregate counts and indexed traversal | Maintain reverse references, metric histograms and dependency indexes instead of rescanning unrelated entries for each query. |

These are backed by the [engine invariants](https://github.com/pierrecomputer/pierre/blob/c5b1e58203fa5c59dd99353cbcac09ee653a24f6/packages/path-store/README.md), [child indexes](https://github.com/pierrecomputer/pierre/blob/c5b1e58203fa5c59dd99353cbcac09ee653a24f6/packages/path-store/src/child-index.ts), and [visible projections](https://github.com/pierrecomputer/pierre/blob/c5b1e58203fa5c59dd99353cbcac09ee653a24f6/packages/path-store/src/visible-tree-projection.ts).

Pierre also has an optional structure-of-arrays representation with typed integer arrays and a compressed child table for count-only tree sweeps. Its comments explicitly say the default object representation is unchanged. This is evidence for testing compact layouts on a measured hot path, not justification for immediately rewriting the entire tool. [SoA implementation](https://github.com/pierrecomputer/pierre/blob/c5b1e58203fa5c59dd99353cbcac09ee653a24f6/packages/path-store/src/soa-node-store.ts)

## Why another cache alone will not reach 10–99 ms

A profile of the correctness-fixed GitLab run took 6.55 seconds. Roughly 1.98 seconds was repeated history parsing, 1.06 seconds discovery, 1.20 seconds initial/final source validation, and 0.68 seconds Ruby reference resolution. Scoring itself was about 0.21 seconds for Ruby. These are measurements of individual phases from one run, not stable percentile bounds.

This round removes repeated history parsing and directory enumeration where the corresponding inputs remain valid. It retains whole-source validation and complete output semantics. It therefore still has repository-sized work. The final benchmark report quantifies the improvement rather than assuming these savings add up perfectly.

Two-digit milliseconds is a reasonable hypothesis to test for a versioned edit or small impact query against a resident index. It is not established for returning every scored row, and it cannot be promised for arbitrary refactors, global resolver changes or worst-case cascading dependencies. A full JSON report also has a cost proportional to the number of returned rows.

## Proposed contract for the next engine

Keep the existing full scan as a reconciliation path and correctness oracle. Add a managed workspace model with distinct operations:

```text
open(root, analysis_options) -> generation
apply(base_generation, versioned_file_changes) -> generation + invalidations
query(generation, scope, fields, page) -> result for that exact generation
export(generation) -> complete report
reconcile_saved_files() -> validated disk generation
```

This is a proposed API, not implemented in the delivered build. An edit must identify its previous content/version; stale updates are rejected. Editors can provide buffer contents explicitly. A disk watcher needs overflow handling, reconciliation and a published processed-event boundary. An unsaved-buffer generation and the current saved filesystem must not be silently presented as the same state.

The shared model would retain file identities, content fingerprints, parser/resolver context, definitions, unresolved references, resolved edges, complexity, LOC and diagnostics. It should retain facts rather than assuming one AST is interchangeable across tools. Ruby’s reference parser and complexity parser currently have different version-selection semantics; ESLint and dependency-cruiser also use different parsing contracts. Unifying them requires new parity evidence.

On an edit:

1. Parse only the changed document under its explicit language/version context.
2. Update its definitions, references, imports and per-file metrics.
3. Re-resolve affected candidates, including previously unresolved names and shadowing namespaces.
4. Update direct/reverse edge indexes and metric distributions.
5. Recompute affected global ranking state before claiming an exact result.
6. Publish one generation, then materialize only the requested output.

Global ranks are the hard part: a changed metric can shift many percentile values, weighted scores and classifications. Tracking only the edited file’s direct dependents is insufficient. Changes to Git history, coverage, configuration and time-based age/window boundaries must also invalidate the relevant state. Force-pushes, watcher gaps and unsupported configuration should trigger explicit rebuilding rather than guessed reuse.

## Evidence required before claiming two-digit milliseconds

Measure managed-update time, exact impact-query time, complete export time and disk reconciliation separately. Replay additions, deletions, renames, parser-version changes, newly resolved references, shadowing, JS configuration changes, Git commits, coverage changes and time boundaries. Compare complete exported reports with a fresh original/defined reference after every state transition. Include real application resolver dependencies and active histories, then report p50/p95/p99 and memory with the workload and machine specified.

The current implementation fixes a demonstrated parity defect and removes measured repeated work. The resident index described here is the next architecture to test; Pierre provides useful techniques, not a drop-in semantic-analysis replacement or a latency guarantee.
