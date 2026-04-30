---
name: memspec-author
description: Use when authoring, completing, or iterating a .memspec slice; also for /memspec-author, spec-writer work, walk-clean specs, and writer/scrutinizer loops.
---

# Memspec Author

Author one walk-clean `.memspec` slice, then run an adversarial scrutiny pass. This is the Codex equivalent of Claude's `/memspec-author` command plus `spec-writer` and `spec-scrutinizer` roles.

## Codex Adaptation

Claude used named agents. In Codex, keep the roles separate as sequential passes in the same session unless the user explicitly asks for delegated agents. Do not approve writer work from the writer pass alone; a scrutiny artifact is required.

Command alias: treat `/memspec-author <slice-or-description>` as an explicit request to use this skill.

## Scope Gate

Use this workflow for non-trivial coupled-state specs. Skip or redirect when:

- Trivial single-cell change with no events: author directly without the loop.
- Known-shape bugfix with no new state graph: writer-only may be enough, but still run `memspec walk`.
- Oversized scope, roughly `>10` cells, `>5` events, or multiple subsystems: use `memspec-slice` first.

## Required Context

Before non-trivial authoring, read:

- `CLAUDE.md`
- `docs/grammar-v0.md`
- Actual code under `meta.under_spec` or the requested implementation scope

Use `memspec` from `PATH` when available. If it is not installed, use `cargo run -p memspec-cli -- <command>`.

## Writer Pass

Refuse to produce or revise a slice if the actual code under spec has not been read.

Hard rules:

- Every `cell` has a `ref:` pointing at a real `file:line`.
- Every `derived` has `derives_from:` and `derivation:`.
- Every event with multiple reachable fallible steps has symmetric `post_failure` coverage for each qualifying `(event, step)` pair.
- `transactional` and `db_transaction` events distinguish `cells_after_pre_rollback` from `cells_after_rollback`.
- Every `forbidden_state` names a `kill_test:`; use `TODO` only to acknowledge an unresolved obligation.
- No `kill_test.status: executed_passing` without test execution.
- Import sibling slices with `use "<relative-path>" as <alias>` rather than re-declaring owned cells.

Iterate with the CLI:

```bash
memspec suggest <file>
memspec query <file> --gaps
memspec query <file> --refs-to <id>
memspec query <file> --by-id <id>
memspec render <file> --format md
memspec walk <file> --json
```

`memspec walk` follows imports by default. The whole working set must exit `0`. W0273-W0275 are advisory.

## Scrutiny Pass

After walk-clean, switch stance to adversarial scrutiny:

- Verify every `cell.ref` points at the named state.
- Verify `enforced_by:` mechanisms exist and fire on the claimed path.
- Grep for bypass mutation sites the spec did not enumerate.
- Check enum values, derivation rules, and failure rows against real code.
- Verify kill-tests would fail if the forbidden state were reachable.
- For rollback events, ensure pre-rollback transient states are declared honestly.

Write the durable report to `<spec-path>-scrutiny.md` with verdict `APPROVE`, `REJECT`, or `APPROVE-WITH-DEFERRED-FINDINGS`.

## Loop

On `REJECT`, revise the spec against the findings, rerun `memspec walk --json`, then rerun scrutiny. Hard ceiling: five writer/scrutiny round trips. Convergence failure usually means the spec exposed a real unresolved design or implementation problem; surface the specific blocker.

## Output

On approval, report:

- Spec path
- `memspec walk` exit status
- Slot counts for cells, derived, associations, events, forbidden states, and kill-tests
- Scrutiny verdict and strongest findings or deferred items
- Next technical route: `memspec-implement` or manual handoff
