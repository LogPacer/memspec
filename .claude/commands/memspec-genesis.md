---
description: Drive spec-revisioner to generate or test an experimental genesis revision manifest for an existing .memspec file. Prototype-only; source-preserving.
argument-hint: <file.memspec> [--reason "initial import"] [--author <name>]
---

# /memspec-genesis

Drive `spec-revisioner` against an existing `.memspec` file.

## Hard rules

- This is experimental migration work only.
- Do not run release-profile commands.
- Do not rewrite the source `.memspec`.
- Do not claim the manifest JSON is stable.
- Default no-feature CLI behavior must remain unchanged.

## Workflow

1. Run `memspec walk <file> --json`; exit 0 required.
2. Invoke `spec-revisioner` with the file path and optional reason/author metadata.
3. Verify default behavior yourself:
   - `cargo test --workspace`
   - `cargo build --workspace`
   - `cargo run -p memspec-cli -- --help` does not expose `experimental`

## Output

- Source `.memspec` path.
- Manifest `result_hash` and op count.
- Default no-feature test/build status.
- Confirmation that source files were not rewritten.
