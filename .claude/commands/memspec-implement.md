---
description: Drive spec-implementer against a walk-clean .memspec slice. Stop when every kill_test reaches executed_passing OR the implementer pushes back on a spec gap.
argument-hint: <slice-file-path> [implementation-paths...]
---

# /memspec-implement

Drive `spec-implementer` to turn a walk-clean slice into red-green code + tests.

## Scope guidance — skip when

- **Spec is not walk-clean** — STOP, recommend `/memspec-author` first.
- **Trivial single-cell change with an obvious test path** — implement by hand.

## Workflow

1. **Pre-check.** Run `memspec walk <spec-file> --json`; exit 0 across the working set required. If not, surface and stop.

2. **Invoke `spec-implementer`** with: spec path + implementation root(s) + language hint. Implementer reads spec, implements red-green per forbidden_state, updates kill_test statuses, refuses new bypass paths.

3. **Verify implementer's signal.** Run yourself:
   - `memspec walk <spec-file> --json` — exit 0 (status updates didn't break the spec).
   - The test suite (`cargo test` / `bin/rails test`).

   If either fails, route findings back.

4. **Ask user: chain into `/memspec-review`?** Default yes.

5. **Hard ceiling: 5 implementer round trips.** Convergence-failure usually means a real bug the implementer is correctly refusing to mark killed without a fix.

## Hard rules

- **Spec must be walk-clean before implementer starts.**
- **Implementer can only update `kill_test.status` in the spec.** Cell/event/forbidden_state changes route back to `/memspec-author`.
- **No `executed_passing` without test execution.** Verify by running the suite.

## Tools

- `Bash` — `memspec walk` + test suite.
- `Read` — spec + impl files.
- `Agent` — invoke `spec-implementer`.
- `Skill` — recursively invoke `/memspec-review` if user opts in.

You do NOT write code or modify cells/events yourself.

## Output

- Spec path + final kill_test status counts (declared / resolved / executed_passing / executed_failing).
- Test suite status.
- Files added/changed.
- Deferred findings the user accepted.
- Next step (PR/commit, or `/memspec-review` if not yet run).

On convergence failure: round-trip log + outstanding push-back items + whether spec-side (route to author) or code-side (real bug).
