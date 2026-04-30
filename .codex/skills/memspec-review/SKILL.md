---
name: memspec-review
description: Use when reviewing an implementation against a .memspec slice before merge; also for /memspec-review, spec-reviewer work, merge gates, or spec drift checks.
---

# Memspec Review

Review an implementation against its `.memspec` slice as the merge gate. This is the Codex equivalent of Claude's `/memspec-review` command plus `spec-reviewer` role.

## Codex Adaptation

Use a code-review stance: findings first, grounded in file/line references. Do not fix during the review pass unless the user explicitly asked for fix-and-review. Do not auto-loop after REJECT; the next action depends on whether the spec, implementation, or both are wrong.

Command alias: treat `/memspec-review <slice> [implementation-paths...]` as an explicit request to use this skill.

## Prechecks

Run:

```bash
memspec walk <spec-file> --json
```

Then run the implementation's relevant test suite. If either fails, verdict is REJECT before deeper review. W0273-W0275 are advisory; surface them but do not auto-reject.

## Review Checks

REJECT for any of these:

- Cell citation drift: every `cell.ref` must still point at the named state.
- Kill-test dishonesty: every `executed_passing` test must exist, pass, and fail if the forbidden state becomes reachable.
- Bypass mutation: any actual write to a spec'd cell is missing from enumerated events in the working set.
- Reverse drift: implementation added new mutable state, events, rollback windows, or state holders that no slice declares.
- Atomicity mismatch: events declared `db_transaction` or `transactional` lack the actual wrapper.
- Failure mismatch: code behavior after a fallible step contradicts `post_failure`.

For bypass checks, combine spec queries with grep:

```bash
memspec query <spec-file> --refs-to <cell-id>
memspec query <imported-slice> --refs-to <cell-id>
```

## Report

Write `<spec-path>-review.md`:

```markdown
# Review - <slice slug>

## Verdict
APPROVE | REJECT | APPROVE-WITH-DEFERRED-FINDINGS

## Pre-conditions
memspec walk: exit <N>
test suite: <pass/fail summary>

## Findings
- **[severity] Title** (spec block + code file:line)
  - What the spec says: ...
  - What the code does: ...
  - Why it matters: ...
  - Required revision: ...

## What the implementation got right
<one paragraph, real strengths only>

## Required revisions
- ...
```

If you want to approve with caveats, REJECT and list the work required to remove the caveats.

## Output

Surface:

- Verdict
- Findings count by severity
- Bypass scan summary, including spec-vs-code deltas
- `post_failure` verification summary
- Next route: merge, `memspec-implement`, `memspec-author`, or a new slice for a real bug the review exposed
