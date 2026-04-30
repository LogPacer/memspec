---
description: Drive spec-slicer on an oversized problem; produces slice-plan.md + N skeleton .memspec files. Optionally chains into /memspec-author per slice.
argument-hint: <problem-description> [or path to an oversized in-progress .memspec to refactor]
---

# /memspec-slice

Drive `spec-slicer` to decompose a too-big problem into N right-sized authorable slices.

## When to use

- A user describes a feature or subsystem clearly too large for one `.memspec`.
- The spec-writer is mid-authoring and signals the slice is growing past complexity caps.
- A spec author asks for a decomposition plan.

If scope is small (one subsystem, ~10 cells or fewer), **don't slice — recommend `/memspec-author` directly.**

## Workflow

1. **Invoke `spec-slicer`** with: problem description (or path to oversized `.memspec`) + plan path (default: `<plan-dir>/slice-plan.md`) + skeleton path pattern (default: `<plan-dir>/<slug>.memspec`).

2. **Surface the plan to the user.** After `slice-plan.md` is written, present:
   - N slices proposed + slugs + one-liners
   - Authoring order (owners before importers — DAG traversal)
   - Dependency graph (which slice imports which via `use "..."`)
   - Cross-cutting concerns the slicer flagged as not-fitting

3. **Ask: "drive `/memspec-author` for each slice now?"** If yes:
   - Author in dependency order, owners first.
   - Wait for each to converge (APPROVE) before starting the next.
   - On convergence failure, STOP — don't pile failures.

4. **If user wants to author manually** (or just wanted the plan), stop after the plan. The user picks slices off `slice-plan.md` at their pace.

## Hard rules

- **Do not skip the surface-to-user step.** User must see and confirm decomposition before skeletons are committed.
- **Do not author slot contents in skeletons.** Slicer produces shells with proposed cell/event names as COMMENTS only; writer fills slots.
- **Do not invoke `/memspec-author` for >5 slices in one batch.** Break into rounds with check-ins.

## Tools

- `Read` — load `slice-plan.md`.
- `Agent` — invoke `spec-slicer`.
- `Skill` — recursively invoke `/memspec-author` per slice if user opts in.

You do NOT decide the decomposition.

## Output

When plan is approved (and skeletons written):
- `slice-plan.md` path.
- Skeleton file paths in authoring order.
- Dependency graph.
- Flagged cross-cutting concerns.
- Next-step suggestion (`/memspec-author <first-skeleton>`).

If user opted into batch authoring and convergence happened across all slices, surface a final consolidated summary.
