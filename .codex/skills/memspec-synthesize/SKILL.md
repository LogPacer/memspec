---
name: memspec-synthesize
description: Use when appending a new inline revision to a `.memspec` file capturing the semantic changes since the last revision. Source-rewriting; append-only; replay-validated. Also for /memspec-revise, MemPacer watcher integration, and recording forbidden-state / kill-test status changes into the file's revision chain.
---

# Memspec Synthesize

Drive `memspec experimental synthesize-revision` against a single `.memspec` file. The command appends a new `revision N+1 { ... }` entry inside the slice's `revisions { ... }` block, hash-chained against the prior `result_hash`, then re-parses and replays the rewritten file to verify integrity. No-op when nothing semantic changed.

## Pre-conditions

- `memspec walk <file>` exits 0. Synthesis records changes; it does not heal a broken slice.
- The existing revision chain (if any) replays cleanly. Synthesis refuses on a broken chain rather than papering over it.

## Workflow

1. Read the file. Note the existing `revisions { ... }` block, if present: revision count, last `result_hash`, last `reason`.
2. Confirm `memspec walk <file> --json` exit code is 0.
3. Run synthesis:

   ```bash
   memspec experimental synthesize-revision <file> --reason "<concrete reason>" --author <name> --json
   ```

   Source build (`memspec` not on PATH):

   ```bash
   cargo run -p memspec-cli --features experimental-revisions -- \
     experimental synthesize-revision <file> --reason "..." --json
   ```

4. Read the JSON report:
   - `appended: true` with a new `revision_number` → file rewritten with the new block; new `result_hash` recorded.
   - `appended: false, no_op: true` → projection hash unchanged; nothing semantic to record. Correct outcome for cosmetic edits.
5. Re-run `memspec walk <file>` to confirm walk-clean.

## Rules

- Pass a concrete `--reason`. The CLI default `"automated edit via watcher"` is for unattended runs.
- Do not hand-edit the `revisions { ... }` block. Entries are hash-chained — manual edits break replay. To correct a recorded reason, append a new revision documenting the correction.
- Do not synthesize on every save. The flow is for *semantic* deltas (cells / events / forbidden_states / kill_test status). Whitespace and comment edits are no-ops by design.
- Do not synthesize on a walk-broken file. Fix the slice first; then synthesize.

## Failure modes

- `existing revision chain is invalid` → a prior revision's `result_hash` doesn't replay. Investigate; do not append.
- `freshly written file failed replay check` → CLI wrote the file but replay failed. Synthesis bug — capture the diff and report.

## Output

- File path.
- Prior revision count + last `result_hash` (or "no prior revisions").
- Outcome: `appended` (new revision number, `result_hash`, recorded reason) or `no_op`.
- Walk-clean status of the rewritten file.
