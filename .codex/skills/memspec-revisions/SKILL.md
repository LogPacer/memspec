---
name: memspec-revisions
description: Use when migrating existing .memspec files into experimental revision/genesis manifests or testing append-only memspec versioning. Also for /memspec-genesis, revision history prototypes, base_hash/result_hash checks, and debug-only versioning trials.
---

# Memspec Revisions

Build or inspect experimental revision manifests for existing `.memspec` files. This is a migration/prototype workflow, not a released storage contract.

## Safety Gate

This workflow is prototype-only for genesis import. It must not change default CLI behavior.

- The command exists only with `--features experimental-revisions`.
- Release binaries may be built with `experimental-revisions` so `experimental synthesize-revision` is available to downstream tools.
- Do not treat `experimental genesis` JSON as a stable storage contract.
- Do not rewrite the source `.memspec` during genesis import.
- Do not present the JSON shape as stable; it is `0.1-experimental`.

## Genesis Import

Use this command to prove an existing `.memspec` can become revision 1 unchanged:

```bash
cargo run -p memspec-cli --features experimental-revisions -- \
  experimental genesis <file.memspec> --reason "initial import" --author <agent> --json
```

Expected semantics:

- `revision_number: 1`
- `base_revision: null`
- `base_hash: null`
- `result_hash` equals `materialized_view.source_hash`
- `patch_format_version: "memspec.semantic_patch/0.1-experimental"`
- `ops` contains `genesis_from_materialized_view` plus semantic add ops for slice/import/walk/declaration/step entries.

## Verification

Run default behavior checks without feature flags:

```bash
cargo test --workspace
cargo build --workspace
cargo run -p memspec-cli -- --help
```

The default help must not expose `experimental`.

Run prototype checks explicitly:

```bash
cargo test --workspace --features experimental-revisions
cargo run -p memspec-cli --features experimental-revisions -- experimental genesis <file.memspec>
```

For bulk migration trials:

```bash
for f in $(find . -name '*.memspec' -print); do
  cargo run --quiet -p memspec-cli --features experimental-revisions -- \
    experimental genesis "$f" --reason "initial import" >/tmp/memspec_genesis.out
done
```

## Output

Report:

- File(s) tested
- Whether source files were unchanged
- Result hash and op count for representative files
- Default no-feature test/build status
- Explicit confirmation that genesis import did not rewrite source files
