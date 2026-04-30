---
description: Drive spec-reviewer against a spec + implementation pair. Returns APPROVE/REJECT with concrete findings. Merge gate.
argument-hint: <slice-file-path> [implementation-paths...]
---

# /memspec-review

Drive `spec-reviewer` to confirm an implementation HONORS its `.memspec` slice. Default REJECT.

## When to use

- After `/memspec-implement` converged.
- Before opening a PR or merging implementation work covered by a `.memspec`.
- Drift detection on existing code against an existing spec.

Skip if there is no spec covering the change — review by hand.

## Workflow

1. **Pre-check.** Run yourself:
   - `memspec walk <spec-file> --json` — exit 0 across the working set required.
   - The implementation's test suite — passing required (or surface failures upfront).
   - W0273-W0275 are advisory.

   If pre-checks fail, surface and stop — don't burn reviewer cycles on a half-baked submission.

2. **Invoke `spec-reviewer`** with: spec path + implementation root(s) + language hint.

3. **Read `<spec-path>-review.md`** and surface verdict:
   - **APPROVE** — short summary + green-light merge.
   - **APPROVE-WITH-DEFERRED-FINDINGS** — list deferred items; ask user.
   - **REJECT** — extract findings, ask user how to proceed:
     - Implementation issues → `/memspec-implement` for another pass.
     - Spec issues → `/memspec-author` for revision (then re-implement, re-review).
     - Real bug the spec exposed → user decides whether to capture as a new forbidden_state walk or fix immediately.

4. **Do NOT loop reviewer cycles automatically.** Each REJECT requires a user decision about which side to fix.

## Hard rules

- **Default REJECT bias is preserved.** Don't soften the verdict — surface findings verbatim.
- **A passing test suite is necessary but not sufficient.** Reviewer can REJECT even when all tests pass (e.g., behavioural kill-test asserts the broken behavior).
- **Bypass scan is non-negotiable.** Every cell's actual write sites must be enumerated in the spec.

## Tools

- `Bash` — `memspec walk` + test suite.
- `Read` — spec + impl + review report.
- `Agent` — invoke `spec-reviewer`.

You do NOT write code or modify the spec.

## Output

- Verdict.
- Findings count by severity.
- Bypass scan summary (cells with spec-vs-grep deltas).
- post_failure verification summary.
- Next step (merge / user choice / which command to invoke).

`<spec-path>-review.md` is the durable artifact for code-review tools and future reference.
