# Attribution

The Rust port on this branch draws on prior work by Saugat Khadka in the
`ripple` project at <https://github.com/saugatkhadka/stud-finder/tree/ripple>.

When we lift or adapt a file, algorithm, or test fixture from ripple, the
source commit hash and the file path it came from are recorded at the
top of the receiving file. This document is the roll-up index of those
lifts.

Both ripple and stud-finder are MIT-licensed. When we lift code from
ripple, MIT REQUIRES the source's copyright notice and the MIT
permission notice to be preserved in all substantial copies. That is the
actual compliance bar. The provenance table below is extra discipline on
top — it makes lifts auditable as both projects evolve and makes
upstreaming improvements back to ripple straightforward — but the
copyright + permission notice preservation is non-optional.

**Per-file mechanic for each lift:**

1. Top-of-file comment with the source commit hash and source path.
2. Preserved copyright line and MIT permission notice from upstream
   (ripple-rooted, naming Saugat Khadka as copyright holder for lifted
   portions).
3. Whether the file is an adapted port or a verbatim lift.
4. A row added to the **Lifts** table below linking local path ↔ ripple
   source.

**Upstream hygiene note (2026-11-12):** ripple's repository declares
`license = "MIT"` in its workspace `Cargo.toml` but does not ship a
`LICENSE` text file at the repo root. When we lift, we supply the
standard MIT permission-notice body ourselves in the receiving file, with
Saugat Khadka named as copyright holder for that portion. If this
becomes a recurring friction point, ask Saugat to add a `LICENSE` file
to ripple so lifts can reference it directly.

## Lifts

None yet. Entries get added here as code is adapted. Each row names the
local path, the ripple source commit and path, and a one-line note on
what was kept or changed.

| Local path | ripple source (commit : path) | Notes |
|------------|-------------------------------|-------|
| _(none yet)_ | | |
