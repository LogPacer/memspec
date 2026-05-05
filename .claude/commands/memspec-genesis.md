---
description: Drive spec-revisioner to generate a revision-1 genesis manifest for an existing .memspec file. Source-preserving (does not modify the file). Run before /memspec-revise on files without an inline revisions block.
argument-hint: <file.memspec> [--reason "initial import"] [--author <name>]
---

# /memspec-genesis

Drive `spec-revisioner` to produce a revision-1 genesis manifest for an existing `.memspec` file.

## When to use

- The file has no inline `revisions { ... }` block and you want to seed one.
- Before `/memspec-revise` on a file that hasn't started its revision history yet.
- Prototyping import of an existing `.memspec` into an external append-only store.

Skip if the file already has a `revisions { ... }` block — use `/memspec-revise` instead; genesis is rev-1-only.

## Hard rules

- Source `.memspec` must not be modified. Confirm the file hash is unchanged after the run.
- The manifest's `patch_format_version` is `0.1-experimental`. Treat the JSON shape as not yet stable.

## Workflow

1. Run `memspec walk <file> --json` yourself; exit 0 required.
2. Invoke `spec-revisioner` with the file path + optional `--reason`/`--author`.
3. Confirm the source file hash is unchanged (`shasum -a 256 <file>`).
4. Surface the manifest summary to the user.

## Tools

- `Bash` — `memspec walk` + `memspec experimental genesis` (or `cargo run` source-build equivalent).
- `Read` — inspect the file.
- `Agent` — invoke `spec-revisioner`.

## Output

- File path.
- Manifest summary: `result_hash`, op count, reason, author.
- Source-byte-identical confirmation.
- Next step: transcribe into `revisions { revision 1 { ... } }` in the slice, then `/memspec-revise` for further edits.
