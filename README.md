# memspec

A discipline-enforcing spec framework for software changes that involve coupled mutable state. Native `.memspec` DSL, pure-Rust toolchain, CLI plus interactive TUI, and agent workflow layers for Claude and Codex.

## Format

A `.memspec` file declares one **slice**: a coherent piece of state and the events that mutate it. A slice is built from eight slot kinds:

| Slot | What it declares |
|---|---|
| `cell` | A mutable state holder: DB column, `ArcSwap<T>` field, jsonb attribute, append-only relation, file, etc. |
| `derived` | State computed from other cells, with the derivation rule named. |
| `association` | An invariant that holds between cells. Names what enforces it (schema constraint, callback, event handler, construction-time wiring, convention, etc.). |
| `event` | An operation that mutates one or more cells. Has a trigger, atomicity, and an ordered list of `step`s. |
| `step` (nested in `event`) | One operation inside an event; flagged `fallible: true | false`. |
| `post_failure` | What every cell holds if a fallible step fails after prior steps succeeded. For events with rollback (`db_transaction`, `transactional`), distinguishes pre-rollback from post-rollback cell state. |
| `forbidden_state` | A cell-value combination that must never be observed. Has a `reachability` (`currently_reachable | runtime_unreachable | structurally_unreachable`). |
| `kill_test` | An assertion that proves a forbidden state is unreachable. Has a `kind` (`behavioural | structural | type_shape | property | model_check`) and a `status` (`declared | resolved | executed_passing | executed_failing`). |

Slots reference each other by ID. The analyzer validates that every reference resolves and that the structural relationships are coherent.

Slices import other slices with `use "<relative-path>" as <alias>` and reference imported declarations as `<alias>.<id>`. The analyzer follows imports transitively, validates qualified refs, and emits warnings for unused imports, duplicate-target imports, and imported-id-shadowed-by-local-id.

Per-walk provenance fields (`walk_added`, `walk_changed`, `walk_killed`, `walk_superseded`) live inline on each clause. `memspec diff <slice> --from N --to M` reports per-walk deltas.

Full grammar reference: [`docs/grammar-v0.md`](docs/grammar-v0.md).

## Slice shapes

The same eight slots cover three shapes. Conventions differ per shape:

| Shape | Examples | Conventions |
|---|---|---|
| **Runtime coupled cells** | `Arc<T>` / `ArcSwap<T>` / atomic publishing across coupled state | `atomicity: transactional`; `post_failure` distinguishes pre/post-rollback; `kill_test.kind` mostly `behavioural` + `type_shape` |
| **Lifecycle coupled cells** | DB-backed state with audited transitions (Rails AR-style) | `atomicity: db_transaction`; `kill_test.kind` mostly `behavioural` + `structural` (grep-based bypass policy) |
| **Deploy-coupled cells** | Schema/contract drift across repositories or services | `atomicity: none` (no cross-repo transaction); `post_failure` collapses to "consumer sees runtime error at boundary"; `kill_test.kind` overwhelmingly `structural`; `enforced_by: convention` or `structural_test` |

`spec-writer.md` documents shape selection. The grammar accepts the same syntax for all three; the conventions are guidance, not enforcement.

## Toolchain

Single Rust workspace, two crates: `memspec-parser` (lexer / parser / analyzer) and `memspec-cli` (binary + TUI).

Three analyzer passes run per file, in order:

1. **Structural** — required-field presence per slot kind. Diagnostic codes `memspec/E0200-0249` + `I0220`.
2. **Coherence** — ID uniqueness, ref resolution, derivation acyclicity, bipartite kill-test ↔ forbidden-state matching, plus warnings (unused-cell, empty-mutates, redundant kill-test). Codes `memspec/E0250-0259` + `W0270-0272`.
3. **Symmetric-failure** — for events with ≥2 fallible steps, `post_failure` rows must exist for every `(event, step)` pair where the step is fallible and follows a fallible-or-mutating prior step. When `atomicity ∈ {transactional, db_transaction}`, the row must distinguish `cells_after_pre_rollback` from `cells_after_rollback`. Codes `memspec/E0300-0301`.

After per-file passes, a cross-slice resolution pass validates qualified refs (`memspec/E0400-0403`), and a composition pass emits cross-slice warnings (unused imports, duplicate-target imports, imported-id-shadowed-by-local-id; codes `W0273-0275`).

Lex codes (`E0001-0006`) and parse codes (`E0100-0110`) round out the diagnostic vocabulary. All codes are stable; once allocated, codes never get reused.

Exit codes: `0` walk-clean, `1` walk-incomplete (recoverable; gaps in JSON), `2` parse error, `3` semantic error, `4` I/O error.

## CLI

| Command | Output |
|---|---|
| `memspec walk <path> [--json] [--single-file]` | Full walk. Multi-file by default (follows imports); `--single-file` opts out. |
| `memspec query <path> --list-ids` | Every declared ID, grouped by slot kind. JSON. |
| `memspec query <path> --by-id <id>` | Full block AST for one ID. JSON. |
| `memspec query <path> --refs-to <id>` | Every site that references an ID. JSON. |
| `memspec query <path> --gaps` | Unkilled forbidden states, kill-tests with non-passing status, missing post_failure rows, unused cells. JSON. |
| `memspec render <path> --format md\|graph [--aggregate]` | Markdown or Mermaid. `--aggregate` walks imports and renders the entire working set. |
| `memspec diff <path> --from N --to M` | Per-walk delta (added / changed / killed / superseded clauses). JSON. |
| `memspec suggest <path>` | Single highest-priority gap with a fill-in template. Deterministic — same input, same output. JSON. |
| `memspec view [path]` | Interactive TUI viewer (lazygit-style). Path may be a file or directory; defaults to the current directory. Read-only. |
| `memspec schema --json-schema` | JSON Schema for all CLI JSON outputs (`walk_single`, `walk_multi`, `query_*`, `diff`, `suggest`, plus shared `diagnostic` / `severity` / `span`). |

## Agent Layer

Six agents under `.claude/agents/`, five orchestration skills under `.claude/commands/`. Symlinked into `~/.claude/` for global availability across projects.

| Agent | Role |
|---|---|
| `spec-slicer` | Decompose a too-large problem into N authorable slices with imports between them. Outputs `slice-plan.md` + skeleton `.memspec` files. |
| `spec-writer` | Author one slice. Iterates against `memspec walk --json` until walk-clean. Refuses to output without reading the actual code under spec. |
| `spec-scrutinizer` | Adversarial review of a walk-clean slice. Default REJECT. Brutalizes claim honesty: claims-vs-code, bypass-site survey completeness, kill-test honesty. Does not re-derive what the parser already checks. |
| `spec-implementer` | Given a landed spec, write tests + code red-green per forbidden state. Updates `kill_test.status` as proof lands. Refuses to introduce new mutation paths the spec does not enumerate. |
| `spec-reviewer` | Merge-gate review of a spec + its implementation. Default REJECT. Verifies the implementation honors the spec; detects drift in both directions. |
| `spec-revisioner` | Debug-only genesis revision import for existing `.memspec` files. Proves migration into append-only versioning without rewriting source files. |

| Skill | What it does |
|---|---|
| `/memspec-slice "<problem>"` | Drives `spec-slicer`. Optionally chains into `/memspec-author` per slice. |
| `/memspec-author <slice>` | Drives `spec-writer` ↔ `spec-scrutinizer` until walk-clean + APPROVE. |
| `/memspec-implement <slice> <impl/>` | Drives `spec-implementer`; optionally chains into `/memspec-review`. |
| `/memspec-review <slice> <impl/>` | Drives `spec-reviewer` as the merge gate. Does not auto-loop on REJECT — user decides which side to fix. |
| `/memspec-genesis <slice>` | Drives `spec-revisioner` to emit an experimental revision-1 manifest. Requires `--features experimental-revisions`; no release build. |

Codex equivalents live under `.codex/skills/` as project-local skills and `.codex/prompts/` as command shims:

| Codex skill | Equivalent |
|---|---|
| `memspec-slice` | `/memspec-slice` + `spec-slicer` |
| `memspec-author` | `/memspec-author` + writer/scrutinizer loop |
| `memspec-scrutinize` | direct `spec-scrutinizer` pass |
| `memspec-implement` | `/memspec-implement` + `spec-implementer` |
| `memspec-review` | `/memspec-review` + `spec-reviewer` |
| `memspec-revisions` | `/memspec-genesis` + debug-only revision import |

Use the skills by name, or invoke the command shims as `/prompts:memspec-author`, `/prompts:memspec-slice`, `/prompts:memspec-implement`, `/prompts:memspec-review`, `/prompts:memspec-scrutinize`, and `/prompts:memspec-genesis`.

## Experimental revisions

Revision import is intentionally debug-only:

```sh
cargo run -p memspec-cli --features experimental-revisions -- \
  experimental genesis path/to/file.memspec --reason "initial import" --json
```

The command emits a revision-1 genesis manifest without rewriting the source file. It records `base_hash: null`, `revision_number: 1`, a SHA-256 `result_hash`, the current materialized view hash, projection counts, and semantic add ops. This does not change default CLI behavior; the `experimental` command is absent unless the feature flag is enabled. Do not use release-profile builds for `experimental-revisions`.

## Relationship to other tools

memspec runs against any codebase in any language. It has zero hard dependencies on other tools. A `.memspec` slice maps cleanly onto external workflow trackers — spec ↔ goal/epic, slice ↔ milestone, obligation-inside-slice ↔ task — but no specific tracker is assumed and no integration ships in this repo.

## Build

```sh
cargo build --release
ln -s "$PWD/target/release/memspec" ~/.local/bin/memspec
```

For Claude agents/commands to be available globally:

```sh
mkdir -p ~/.claude/agents ~/.claude/commands
ln -sf "$PWD/.claude/agents/"*.md ~/.claude/agents/
ln -sf "$PWD/.claude/commands/"*.md ~/.claude/commands/
```

For Codex, the project-owned skill files live in `.codex/skills/` and command shims live in `.codex/prompts/`. If your Codex installation only scans personal paths, expose these globally by symlinking `.codex/skills/*` into `~/.codex/skills/` and `.codex/prompts/*.md` into `~/.codex/prompts/`.

## Reference docs

- [`docs/grammar-v0.md`](docs/grammar-v0.md) — full v0 grammar.
- [`CLAUDE.md`](CLAUDE.md) — repo orientation for Claude Code.
- [`NOTICE`](NOTICE) — MIT attribution for allium-tools (JUXT, 2026), the architectural inspiration.

## License

MIT (see [`LICENSE`](LICENSE)).
