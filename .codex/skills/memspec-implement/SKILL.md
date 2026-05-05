---
name: memspec-implement
description: Use when implementing code and tests from a walk-clean .memspec slice; also for /memspec-implement, spec-implementer work, or kill_test status execution.
---

# Memspec Implement

Turn a walk-clean `.memspec` slice into code and tests. This is the Codex equivalent of Claude's `/memspec-implement` command plus `spec-implementer` role.

## Codex Adaptation

Implement in the current session unless the user explicitly asks for delegated agents. You may change implementation files and test files. In the spec, only update `kill_test.status` and test refs; cell/event/forbidden-state changes route back to `memspec-author`.

Command alias: treat `/memspec-implement <slice> [implementation-paths...]` as an explicit request to use this skill.

## Precondition

Run:

```bash
memspec walk <spec-file> --json
```

The full working set must exit `0`. If not, stop implementation and route to `memspec-author`.

Read the spec end-to-end, including imported slices. Use:

```bash
memspec render <spec-file> --format md --aggregate
memspec query <spec-file> --gaps
memspec query <spec-file> --by-id <id>
memspec query <spec-file> --refs-to <cell-id>
```

Be able to restate the full state graph before editing code.

## Read Discipline

- Prefer focused CLI queries over manual code search: `memspec query <file> --by-id <id>`, `--refs-to <id>`, `--gaps`. Spec-side facts come from the parser, not from grep.
- Read code with `offset`/`limit` line ranges — never whole files. Default Read pulls 2000 lines.
- Use `grep -l` to find files, then read targeted ranges. Never dump full file contents into context.
- If the working environment exposes a code-indexing tool (project codemap, language-server query, repo graph), prefer it for symbol/usage lookups before falling back to Grep/Read.

## Implementation Rules

- For each cell, enumerate the spec's declared mutation paths with `memspec query --refs-to`.
- For qualified refs like `<alias>.<cell>`, also inspect the owning slice.
- Do not add a new mutation path the working set does not enumerate. If one is necessary, stop and update the spec through `memspec-author` first.
- Implement event steps in declared order and with the declared atomicity wrapper.
- Verify `post_failure` content against actual code behavior. If the spec is wrong, stop and route back to authoring.

## Kill Tests

Every forbidden state needs a kill-test with honest status:

- `behavioural`: write or update a regression test that would fail if the forbidden state were reachable, then implement the prevention and run the test.
- `structural`: write a structural assertion, often a grep or AST-style test, that blocks bypass mutation paths.
- `type_shape`: prove the type-level shape prevents the split or invalid state.
- `property` / `model_check`: wire the stated invariant into the property checker or model checker.

Status progression:

```text
declared -> resolved -> executed_passing
declared -> resolved -> executed_failing
```

Never mark `executed_passing` without running the relevant command and seeing a pass.

## Done Criteria

All must hold:

- Every forbidden state in the working set has an executed passing kill-test or an explicitly accepted deferred guideline.
- Every declared event step is implemented.
- Every post-failure row has been checked against actual behavior.
- `memspec walk <spec-file> --json` exits `0`.
- The relevant test suite passes.
- A bypass scan finds no new mutation paths outside enumerated events.

## Output

Report:

- Spec path
- Kill-test status counts: declared, resolved, executed_passing, executed_failing
- Test commands run and status
- Files changed
- Any spec-side pushback
- Next technical route: `memspec-review` before merge
