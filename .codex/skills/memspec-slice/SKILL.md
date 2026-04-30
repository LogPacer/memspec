---
name: memspec-slice
description: Use when decomposing an oversized coupled-state problem into right-sized .memspec slices; also for /memspec-slice, spec-slicer work, or slice-plan requests.
---

# Memspec Slice

Decompose an oversized problem into authorable `.memspec` slices with explicit imports. This is the Codex equivalent of Claude's `/memspec-slice` command plus `spec-slicer` role.

## Codex Adaptation

Do the decomposition in the current session unless the user explicitly asks for delegated agents. Surface the proposed decomposition before writing skeleton files; this checkpoint is technical approval of ownership boundaries, not a pacing checkpoint.

Command alias: treat `/memspec-slice <problem-or-file>` as an explicit request to use this skill.

## Use Or Skip

Use when:

- A feature or subsystem is too large for one slice.
- A writer signals the slice is past complexity caps.
- The problem crosses subsystem ownership boundaries.

Skip when the problem fits one slice, roughly fewer than `10` cells and fewer than `5` events. Route to `memspec-author`.

## Boundary Heuristics

Prefer boundaries in this order:

1. Independent state graphs.
2. Temporal scopes: config-load-time, request-time, migration-time.
3. Ownership boundaries: crate/module/app/frontend/backend.
4. Mode boundaries: bugfix vs feature vs refactor.
5. Complexity caps: `>10` cells, `>5` events, or `>6` forbidden states.

One cell has one owning slice. Downstream slices import it with `use "..." as <alias>` and qualified refs.

## Workflow

1. Read the problem description or oversized `.memspec`.
2. Read relevant code with `rg` and file reads to map actual subsystem boundaries.
3. Propose the decomposition and surface:
   - Slice slugs and one-liners
   - Owning cells/events per slice
   - Import DAG and authoring order
   - Cross-cutting concerns that do not fit

5. After technical approval, write the plan and skeletons.

Do not author slot contents in skeletons. Use comments for proposed cell/event/forbidden-state names; `memspec-author` fills the real slots.

## Plan Artifact

Write `<plan-dir>/slice-plan.md`:

```markdown
# Slice plan - <problem-name>

**Decomposition rationale:** <one paragraph>

## Slices

### 1. `<slug>` - <one-line>
- Owns: <cells/events>
- Imports: <aliases and what they provide>
- Imported by: <downstream slices>
- Authoring order: <position>
- File: `<path>/<slug>.memspec`

## Dependency graph
<ASCII or Mermaid DAG>

## Cross-cutting concerns NOT covered
- ...
```

## Skeleton Shape

Write one skeleton `.memspec` per approved slice:

```memspec
slice <slug> {
  use "./<dependency>.memspec" as <alias>

  meta {
    title: "<one-line>"
    memspec_version: "0.1"
    mode: <greenfield | brownfield | bugfix | refactor>
    under_spec: "<path/to/code>"
  }

  walk 1 { summary: "initial decomposition by memspec-slice" }

  // Cells the writer must declare
  // <cell-id> (proposed type: <type>)

  // Events likely to live here
  // <event-id>: <one-line>; mutates: [...]

  // Forbidden states the writer should consider
  // <fs-id>: <one-line>
}
```

## Failure Modes

- Premature decomposition: one slice was the correct answer.
- Hidden coupling: downstream slice imports too many ancestors or qualified refs; collapse slices.
- Re-declaration: a cell appears in multiple owner slices; pick one owner.
- Cyclic imports: collapse the cycle or move the shared ownership boundary.

## Output

Report plan path, skeleton paths in authoring order, dependency graph, flagged cross-cutting concerns, and the next `memspec-author <first-skeleton>` command.
