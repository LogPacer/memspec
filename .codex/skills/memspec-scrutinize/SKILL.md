---
name: memspec-scrutinize
description: Use when adversarially reviewing a walk-clean .memspec slice before it drives implementation; also for spec-scrutinizer or scrutiny report requests.
---

# Memspec Scrutinize

Adversarially review a walk-clean `.memspec` slice for claim honesty. This is the Codex equivalent of Claude's `spec-scrutinizer` agent.

## Codex Adaptation

Do the scrutiny as a separate stance from authoring. If you authored the spec in the same session, still write the scrutiny report and preserve the default REJECT bias.

Command alias: direct requests for `spec-scrutinizer` map to this skill.

## Precondition

Run:

```bash
memspec walk <file> --json
```

`memspec walk` follows imports by default. Exit `0` across the working set is required. If the walk is not clean, write or report `REJECT`: writer did not reach walk-clean before requesting scrutiny. W0273-W0275 are advisory; surface them but do not auto-reject.

The parser/analyzer already covers structural completeness, ID uniqueness, ref resolution, derivation acyclicity, kill-test matching, symmetric-failure row presence, cross-slice resolution, and composition warnings. Do not spend the review re-proving those checks.

## Blocking Findings

REJECT for any of these:

- Substrate drift: `cell.ref` points at the wrong line, stale code, comments instead of state, or a refactored-away shape.
- Phantom enforcement: `enforced_by:` claims a callback, transaction, schema constraint, or event handler that does not exist or does not fire.
- Incomplete bypass survey: actual writes to a spec'd cell are missing from enumerated events.
- Semantic mismatch: enum values, derivations, mutability, or state names differ from the code.
- Wrong failure contents: `post_failure` rows exist but do not match actual cell states after the failing step.
- Kill-test dishonesty: a cited test would pass against the broken behavior, does not exist, was not run, or cannot kill the forbidden state kind it claims.
- Hidden pre-rollback state: `db_transaction` or `transactional` pre-rollback states are not declared when they are materially observable inside the rollback window.

## Search Patterns

Use language-specific grep searches for bypasses:

- Ruby/Rails: `update!(state:`, `update_columns(state:`, `update_attribute(:state`, `create!(state:`, `update_all`, raw SQL, `params.permit(:state)`.
- Rust: direct field assignments, `.swap()`, `.store()`, `.compare_exchange()`, or split mutable cells the spec claims are co-published.

For cross-slice cells, query both the importing slice and the owning slice:

```bash
memspec query <spec-file> --refs-to <cell-id>
memspec query <owning-slice> --refs-to <cell-id>
```

## Read Discipline

- Prefer focused CLI queries over manual code search: `memspec query <file> --by-id <id>`, `--refs-to <id>`, `--gaps`. Spec-side facts come from the parser, not from grep.
- Read code with `offset`/`limit` line ranges — never whole files. Default Read pulls 2000 lines.
- Use `grep -l` to find files, then read targeted ranges. Never dump full file contents into context.
- If the working environment exposes a code-indexing tool (project codemap, language-server query, repo graph), prefer it for symbol/usage lookups before falling back to Grep/Read. Bypass-site surveys especially benefit — a codemap can enumerate write-sites the language-specific grep patterns above are designed to catch.

## Report

Write `<spec-path>-scrutiny.md`:

```markdown
# Scrutiny - <slice slug>

## Verdict
APPROVE | REJECT | APPROVE-WITH-DEFERRED-FINDINGS

## Pre-condition
memspec walk: exit <N>, <M> diagnostics

## Findings
- **[severity] Title** (spec block + code file:line)
  - What the spec says: ...
  - What the code does: ...
  - Why it matters: ...
  - Required revision: ...

## What the slice got right
<one paragraph, real strengths only>

## Required revisions
- ...
```

If you want to approve with caveats, REJECT and list the required revisions.
