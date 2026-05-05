# CLAUDE.md

Repo orientation for Claude Code working in this codebase.

## What this repo is

`memspec` — a discipline-enforcing spec framework with a native `.memspec` DSL, a pure-Rust toolchain (lexer/parser/analyzer/CLI/TUI), and an agent layer (writer/scrutinizer/implementer/reviewer/slicer) that authors and reviews slices.

The 8 load-bearing slot kinds (`cell`, `derived`, `association`, `event` with nested `step`, `post_failure`, `forbidden_state`, `kill_test`) capture coupled mutable state and the failure paths through it. A slice cannot reach `state=ready` with empty slots, dangling refs, or unkilled forbidden states — the analyzer refuses.

Read `README.md` for the user-facing overview, `docs/grammar-v0.md` for the full grammar.

## Layout

- `crates/memspec-parser/` — lexer, parser, AST, analyzer (structural / coherence / symmetric-failure / cross-slice / composition / loader / suggest / query / render / diff / revisions).
- `crates/memspec-cli/` — single binary `memspec`. Subcommands: `walk`, `query`, `suggest`, `render`, `diff`, `view` (TUI), `schema`.
- `docs/grammar-v0.md` — the on-disk format reference.
- `.claude/agents/` and `.claude/commands/` — the writer/scrutinizer/implementer/reviewer/slicer agents and their slash-command orchestrators.
- `.codex/skills/` and `.codex/prompts/` — Codex equivalents of the same agent layer.

## Role separation (load-bearing)

`.memspec` is a **reference of truth**, not a source of truth. Code and tests are the source of truth. The artifact's job is to make the *shape of the problem* explicit so different agent roles can reason about it. Each role does different work, and the parser must NOT absorb the work of the others:

- **Parser/analyzer** (this repo): structural completeness + internal coherence. Refuses incoherent specs. Does NOT verify claims against code, run tests, or resolve `ref:` strings into real source. If parser code starts opening source files, that's a layering violation.
- **`spec-writer` agent**: authors a slice. The 8-slot grammar forces the symmetric-failure question for any event with ≥2 fallible steps.
- **`spec-scrutinizer` agent** (default REJECT): brutalizes claim honesty. "You say only 3 mutation sites — show me the survey." Substrate-vs-code checks live here, not in the parser.
- **`spec-implementer` agent**: turns a walk-clean slice into red-green code + tests, per forbidden_state. Updates `kill_test.status`. Cannot author new cells/events.
- **`spec-reviewer` agent**: merge gate. Confirms the implementation honors every claim the spec makes AND that the implementer hasn't extended the system in ways the spec doesn't acknowledge.

The parser does not police roles 2–5. It just ensures the artifact they all reference is well-formed.

## Locked architecture decisions

Re-litigate only with concrete cause.

- **On-disk format: native `.memspec` DSL, span-first, single-file, tool-native.** Not human-skim-friendly by default; `memspec render` produces human-readable views.
- **Kill-test states are three, distinct.** `declared` (named obligation) → `resolved` (test ref points at real `file:line`) → `executed_passing` (test ran and passed). Never print "killed: N/M" unless those tests were `executed_passing`.
- **Agent-native CLI is the primary surface; LSP is deferred.** Stable diagnostic codes (`memspec/E####`, `memspec/W####`, `memspec/I####`) and stable exit codes (0 clean, 1 walk-incomplete, 2 parse error, 3 semantic error, 4 I/O) — the analysis crate has one consumer today; an LSP shell would reuse it later.
- **Per-walk provenance is inline.** `walk_added`/`walk_changed`/`walk_killed`/`walk_superseded` fields per clause; no sidecar log.
- **Citation resolution is a scrutinizer concern, not parser scope.** The parser checks `file:line` is well-formed. Verifying the line actually contains the named cell is language-specific and lives in scrutinizer/adapter territory.
- **Symmetric failure scope.** The "≥2 fallible steps requires `post_failure` rows for each (event, step) pair where N>1" rule applies to ≥2 reachable fallible steps on the same execution path.

## Conventions

- **Pure Rust, no Node.** Single CLI binary `memspec`.
- **The parser does NOT verify claims against code.** If you find yourself opening source files or running tests inside parser/analyzer code, stop — that's scrutinizer/adapter territory.
- **The 8 slots are structural.** A slice cannot reach walk-clean with empty slots, dangling refs, or unkilled forbidden states. When a tradeoff threatens this property, the discipline wins.
- **Allium-tools is read-only inspiration.** Architectural patterns only (lexer style, `BlockDecl { kind, name, items }` AST shape, span-bearing diagnostics). Never `Cargo.toml`-depend on it. MIT attribution preserved in `NOTICE`.
- **Agent dispatch via `general-purpose`: paste the agent's prompt INLINE.** Do NOT tell the dispatched agent to `Read` its own prompt file — `~/.claude/agents/<name>.md` resolves to a path outside the session's working roots and Read denies it. Native invocation via `Task(subagent_type=...)` or via slash commands loads the system prompt directly.

## Release process

Releases are driven by `mise.toml` tasks: `release:build` → `release:tag <ver>` → `release:publish <ver>`. Cargo `version`, `.claude-plugin/plugin.json` `version`, and `.codex-plugin/plugin.json` `version` must all match the release tag.

**Always bump the marketplace pin alongside.** memspec ships through `LogPacer/mempacer-marketplace`, which pins the tag users actually install. After publishing a memspec release, update the sibling repo (typically `/Users/mhl/projects/mempacer-marketplace/`):

- `.claude-plugin/marketplace.json` — `plugins[0].version` + `plugins[0].source.ref` + `metadata.version`. Update `plugins[0].description` if the agent surface changed (added/removed an agent).
- `.agents/plugins/marketplace.json` — `plugins[0].source.ref`.
- `README.md` — plugin table version + description.

Without that bump, plugin users on `/plugin marketplace upgrade` (Claude) or `codex plugin marketplace upgrade mempacer` (Codex) won't see the new release. The marketplace is the install surface, not this repo.

Semver discipline: agent surface or CLI changes are MINOR, prompt-only or doc-only fixes are PATCH. Pre-1.0 grammar changes may land in MINOR per `CHANGELOG.md` header.

## Required reading before non-trivial parser/analyzer work

- `README.md` — user-facing overview.
- `docs/grammar-v0.md` — full v0 grammar.
- `.claude/agents/spec-writer.md` and `.claude/agents/spec-scrutinizer.md` — the agent contracts the CLI must serve.
