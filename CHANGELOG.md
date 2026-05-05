# Changelog

All notable changes to memspec are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Pre-1.0.0, breaking changes to the `.memspec` grammar or analyzer behaviour may land in minor releases — `docs/grammar-v0.md` is the authoritative format reference.

## [Unreleased]

### Added

- `spec-synthesizer` agent and `/memspec-revise` slash command (Claude) plus the matching `memspec-synthesize` skill and `/memspec-revise` prompt (Codex). Drives `memspec experimental synthesize-revision` to append inline revisions hash-chained against the prior `result_hash`, with replay-validation on the rewritten file.

### Changed

- `spec-revisioner` agent and `/memspec-genesis` command (Claude) plus the `memspec-revisions` skill and `/memspec-genesis` prompt (Codex) tightened to genesis-only scope. Dropped the "experimental / prototype / debug-only" framing now that synthesize-revision is the production append flow; both surfaces now cross-reference each other (genesis seeds revision 1, synthesize appends N+1).

## [0.2.0] - 2026-05-05

### Added

- Inline `.memspec` revisions-block analysis behind the `experimental-revisions` cargo feature, including strict replay of semantic operations and terminal hash checks.
- `memspec experimental synthesize-revision`, which appends inline revisions for semantic changes in-place and is included in the binary release artifacts for MemPacer watcher integration.
- Codex plugin manifest and Codex workflow skills/prompts for authoring, scrutinizing, implementing, reviewing, slicing, and revision experiments.

### Changed

- Release packaging now builds `memspec-cli` with `experimental-revisions` so release binaries expose `experimental synthesize-revision`.
- Plugin manifests are versioned with the crate release.

## [0.1.0] - 2026-05-01

First public release.

### Added

- **Native `.memspec` grammar (v0).** Eight load-bearing slot kinds — `cell`, `derived`, `association`, `event` with nested `step`, `post_failure`, `forbidden_state`, `kill_test` — capture coupled mutable state and the failure paths through it. Format reference: `docs/grammar-v0.md`.
- **Pure-Rust toolchain.** Hand-rolled single-pass tolerant lexer, span-bearing AST, and a multi-pass analyzer split across structural / coherence / symmetric-failure / cross-slice / composition / loader / suggest / query / render / diff modules. Implemented in `crates/memspec-parser`.
- **`memspec` CLI** (`crates/memspec-cli`) with subcommands:
  - `walk` — full analyzer pass; exits non-zero on incomplete or incoherent slices. `--json` for structured output.
  - `query` — single-purpose queries (`--gaps`, `--by-id`, `--refs-to`, …).
  - `suggest` — proposes the next missing slot/clause as a fillable template.
  - `render` — human-readable views (markdown today; more formats later).
  - `diff` — per-walk provenance diffs.
  - `view` — lazygit-style TUI for browsing slices, slots, and the cross-slice import graph.
  - `schema` — emits the JSON schema for `--json` output.
- **Cross-slice imports.** `use "<path>" as <alias>` pulls cells from sibling slices; the loader walks the import DAG and the analyzer validates qualified refs across the working set.
- **Stable diagnostic codes** (`memspec/E####`, `memspec/W####`, `memspec/I####`) and **stable exit codes** (`0` clean, `1` walk-incomplete, `2` parse error, `3` semantic error, `4` I/O).
- **Per-walk provenance**: `walk_added` / `walk_changed` / `walk_killed` / `walk_superseded` fields on every clause.
- **Three-state kill-test semantics**: `declared` → `resolved` → `executed_passing` / `executed_failing`. The CLI never reports "killed: N/M" against unexecuted tests.
- **Agent layer** (`/.claude/agents/`, `.codex/skills/`):
  - `spec-writer` — authors a slice, iterates against `memspec walk` until walk-clean.
  - `spec-scrutinizer` — adversarial review, default REJECT; brutalises claim honesty against the codebase.
  - `spec-implementer` — turns a walk-clean slice into red-green code + tests; updates `kill_test.status`.
  - `spec-reviewer` — merge-gate review; bidirectional drift detection.
  - `spec-slicer` — decomposes oversized problems into N right-sized authorable slices.
  - `spec-revisioner` — experimental revision/genesis manifest workflow.
- **Slash commands** (`/.claude/commands/`): `/memspec-author`, `/memspec-slice`, `/memspec-implement`, `/memspec-review`, `/memspec-genesis` — orchestrate the agent layer end-to-end.
- **Claude Code plugin manifest** (`.claude-plugin/plugin.json`).
- **Experimental revision support** behind the `experimental-revisions` cargo feature.

### Notes

- Architecture inspired by [allium-tools](https://github.com/juxt/allium-tools) (JUXT, MIT 2026): hand-rolled lexer style, uniform `BlockDecl { kind, name, items }` AST shape, span-bearing diagnostic format. No code dependency; attribution preserved in `NOTICE`.

[Unreleased]: https://github.com/LogPacer/memspec/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/LogPacer/memspec/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/LogPacer/memspec/releases/tag/v0.1.0
