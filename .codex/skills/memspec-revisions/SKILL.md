---
name: memspec-revisions
description: Use when generating a revision-1 genesis manifest for an existing `.memspec` file. Source-preserving — proves the file can become revision 1 of an append-only chain without modifying it. Also for /memspec-genesis, base_hash/result_hash checks, and seeding the inline revisions block before `/memspec-revise` takes over.
---

# Memspec Revisions (Genesis)

Drive `memspec experimental genesis` against a single `.memspec` file. The command emits a JSON manifest declaring revision 1 — `base_revision: null`, `base_hash: null`, `result_hash` matching the file's projection hash, and a list of `add_*` ops reproducing every declaration in the slice from an empty baseline.

Genesis is **source-preserving** — it does not modify the file. To actually start an inline `revisions { ... }` block in the file, transcribe the genesis result into a `revisions { revision 1 { ... } }` clause inside the slice; subsequent revisions are appended via `memspec-synthesize`.

For appending revisions to a file that already has an inline `revisions { ... }` block, use the `memspec-synthesize` skill instead.

## Pre-conditions

- `memspec walk <file>` exits 0.

## Workflow

1. Confirm `memspec walk <file> --json` exit code is 0.
2. Generate the manifest:

   ```bash
   memspec experimental genesis <file> --reason "initial import" --author <name> --json
   ```

   Source build:

   ```bash
   cargo run -p memspec-cli --features experimental-revisions -- \
     experimental genesis <file> --reason "initial import" --author <name> --json
   ```

3. Verify the manifest shape:
   - `revision_number: 1`
   - `base_revision: null`
   - `base_hash: null`
   - `result_hash` equals `materialized_view.source_hash`
   - `patch_format_version: "memspec.semantic_patch/0.1-experimental"`
   - `ops[0]` is `genesis_from_materialized_view`; subsequent ops include `add_slice`, `add_import`, `add_walk`, and `add_*` for each declaration.

4. Confirm the source file is byte-identical to before (`shasum -a 256 <file>` matches pre-run hash).

## Rules

- The source `.memspec` MUST NOT be rewritten. Genesis is read-only.
- Do not present the manifest as a stable storage contract — `patch_format_version` is `0.1-experimental`.

## Output

- File path.
- Manifest summary: `result_hash`, op count, recorded reason and author.
- Source-byte-identical confirmation.
- Suggested next step: transcribe into `revisions { revision 1 { ... } }`, then use `memspec-synthesize` for further edits.
