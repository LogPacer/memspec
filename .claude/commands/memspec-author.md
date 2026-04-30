---
description: Drive the writer ↔ scrutinizer loop for one .memspec slice. Stop when walk-clean AND scrutinizer APPROVE.
argument-hint: <slice-file-path> [— or describe the slice to author from scratch]
---

# /memspec-author

Orchestrate `spec-writer` and `spec-scrutinizer` against one `.memspec` slice.

## Scope guidance — skip the loop when

- **Trivial single-cell change** (one cell, no events): author by hand, skip both agents.
- **Bugfix in a known shape** (well-understood subsystem, no new state graphs): writer-only is often enough — skip the scrutinizer round trip unless the bypass-survey is non-obvious.
- **Scope clearly too large** (>~10 cells, >~5 events, multiple subsystems): STOP and recommend `/memspec-slice` first.

## Workflow

1. **Invoke `spec-writer`** with: slice file path + scope description. Writer iterates against `memspec walk` until walk-clean across the working set.

2. **Verify walk-clean.** Run `memspec walk <file> --json` yourself; check exit 0 across the JSON `files:` array. W0273-W0275 are advisory. If not clean, route diagnostics back to writer.

3. **Invoke `spec-scrutinizer`** (skip per scope guidance above). Default REJECT.

4. **Triage the verdict:**
   - **APPROVE** — summary + stop.
   - **APPROVE-WITH-DEFERRED-FINDINGS** — surface deferred items; ask user.
   - **REJECT** — extract findings from `<spec-path>-scrutiny.md`, re-invoke writer with the findings list, loop to step 2.

5. **Hard ceiling: 5 round trips.** Convergence-failure usually means a real bug the spec is correctly refusing to whitewash — surface to user.

## Tools

- `Bash` for `memspec walk --json`.
- `Read` for spec + scrutiny report.
- `Agent` for `spec-writer` / `spec-scrutinizer`.

You do NOT author or scrutinize yourself.

## Output on APPROVE

Final spec path + walk exit status + slot counts (cells/derived/associations/events/forbidden_states/kill_tests) + scrutinizer's "what the slice got right" + deferred findings + suggested next step (`/memspec-implement` or hand-off).

## Output on convergence failure

Round-trip log + scrutinizer's persistent findings + open items + recommended user decisions to unblock.
