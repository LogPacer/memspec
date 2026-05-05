---
description: Drive spec-synthesizer to append a new inline revision to a .memspec file capturing the semantic changes since the last revision. Source-rewriting; append-only; replay-validated.
argument-hint: <file.memspec> [--reason "<concrete reason>"] [--author <name>]
---

# /memspec-revise

Drive `spec-synthesizer` against a `.memspec` file to append the next inline revision.

## When to use

- After a semantic change to a slice (added cell, mutated event, killed forbidden state, status update on a kill_test) when you want that change recorded in the file's append-only revision history.
- Before merging or sharing a slice if the workflow expects every semantic change to land as an inline revision entry.

Skip if:
- The file has no `revisions { ... }` block AND you do not want to start one. Use `/memspec-genesis` first to seed revision 1.
- The edit was cosmetic (whitespace, comments). Synthesis will no-op anyway, but don't burn the cycles.

## Workflow

1. **Pre-check.** Run `memspec walk <file> --json` yourself; exit 0 required. If the slice is walk-broken, route to `/memspec-author` first.
2. **Invoke `spec-synthesizer`** with: file path + `--reason "<concrete>"` + optional `--author`.
3. **Read the JSON report** the synthesizer surfaces:
   - `appended: true` → new revision N+1 in the file. Re-walk to confirm walk-clean.
   - `appended: false, no_op: true` → no semantic change since last revision. Stop.
   - error → surface verbatim; do not retry blindly.

## Hard rules

- A concrete `--reason` is required when human-driven. The CLI default is for watcher integrations.
- Do not edit the `revisions { ... }` block by hand to "fix" what synthesis recorded — append another revision documenting the correction.
- Synthesis does not heal a broken slice. Walk-broken file → fix via writer → then synthesize.

## Tools

- `Bash` — `memspec walk` + `memspec experimental synthesize-revision` (or `cargo run` source-build equivalent).
- `Read` — inspect the file before and after.
- `Agent` — invoke `spec-synthesizer`.

You do NOT edit the revisions block by hand.

## Output

- File path.
- Outcome: `appended` (with new revision number + `result_hash` + reason) or `no_op`.
- Walk-clean status of the rewritten file.
- Next step: continue editing, or hand off (review / commit / push).
