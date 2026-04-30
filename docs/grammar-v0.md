# `.memspec` v0 — Grammar Paper

Status: paper sketch. Locks the language enough to start the lexer. Adapters, runner integration, source-code coupling are explicitly v1+.

## What this is

A formal-ish definition of the `.memspec` file format and the structural+coherence checks the parser/analyzer perform. The grammar exists to make agent authoring of behavioural specs *structural*, not vibes-driven. The 8 load-bearing slots are mandatory shapes; the analyzer refuses incomplete or incoherent specs.

## What this is not

- Not a verifier of claims against source code (that's the scrutinizer agent role + later adapters).
- Not a test runner or test-result validator.
- Not a substrate-tied artifact (no commit anchors, no mandatory `file:line` resolution; both are optional advisory metadata).
- Not a multi-language source format — `.memspec` is one syntax. Implementations of the spec'd system can be in any language; renderers and adapters are what bridge to specific runners.

## The graph model (what the parser produces)

A `.memspec` file declares a **slice graph**:

```
                 ┌──────────────┐
                 │  slice meta  │  (id, title, walk: N, optional code_revision, mode)
                 └──────────────┘
                        │
       ┌────────────────┼────────────────┬─────────────────┬─────────────────────┐
       ▼                ▼                ▼                 ▼                     ▼
   ┌───────┐      ┌──────────┐    ┌─────────────┐   ┌───────────┐         ┌──────────────┐
   │ cells │◀─────│ derived  │    │associations │   │  events   │────┐    │  forbidden_  │
   │       │◀─────│          │    │             │   │           │    │    │    states    │
   │       │◀─────│          │    │             │   │           │    │    │              │
   └───────┘      └──────────┘    └─────────────┘   └───────────┘    │    └──────────────┘
       ▲              │                  │                │           │           │
       │              │ derives_from     │ over           │ mutates   │ steps[]   │ cells:/predicate:
       │              │                  │                │           ▼           │
       │              ▼                  ▼                ▼      ┌─────────┐      ▼
       │          (cell-ids)         (cell-ids)       (cell-ids) │  step   │  ┌─────────────┐
       │                                                         │  ...    │  │ kill_tests  │
       │                                                         └─────────┘  │             │
       │                                                              │       └─────────────┘
       │                                                              │             ▲
       │                                                              │             │ forbidden:
       │                                                              ▼             │
       │                                                       ┌──────────────┐    │
       └───────────────────────────────────────────────────────│ post_failure │    │
                                                               └──────────────┘    │
                                                                                   │
                                                                          (kill-test → forbidden)
                                                                            (forbidden → kill-test)
                                                                            (bipartite matching)
```

Edges are typed (`derives_from`, `over`, `mutates`, `forbidden`, …); analyzer walks them.

## File structure

One slice per file. File extension `.memspec`. UTF-8.

```ebnf
file        = slice_decl
slice_decl  = "slice" IDENT "{" slice_body "}"
slice_body  = use_decl* slice_meta? walk_decl* slot_decl+
use_decl    = "use" STRING "as" IDENT
slice_meta  = "meta" "{" meta_field* "}"
meta_field  = ("title"  ":" STRING
              | "mode"  ":" mode_value         // greenfield | brownfield | bugfix | refactor
              | "memspec_version" ":" VERSION  // e.g. "0.1"
              | "code_revision"   ":" STRING   // optional advisory hint to scrutinizer
              | "under_spec"      ":" STRING   // optional advisory; relative root for ref:
              )
walk_decl   = "walk" INT "{" walk_field* "}"   // top-level walk record (optional but recommended)
walk_field  = "summary" ":" STRING
            | "added"   ":" "[" IDENT* "]"
            | "killed"  ":" "[" IDENT* "]"
            | "changed" ":" "[" IDENT* "]"
slot_decl   = cell_decl | derived_decl | association_decl | event_decl
            | post_failure_decl | forbidden_decl | kill_test_decl
```

Every declaration carries an `IDENT` (unique within the slice) and may carry per-walk provenance fields (`walk_added`, `walk_changed`, `walk_killed`, `walk_superseded`).

## Cross-slice imports

A slice may import other slices to reference declarations across files. Imports appear at the top of the slice body, before `meta`:

```memspec
slice rule_audit {
  use "./rule_lifecycle.memspec" as lc
  use "../shared/changelog.memspec" as ch

  meta { title: "Rule audit composed over rule_lifecycle" }

  cell rule_changelogs {
    type: append_only_relation<RuleChangelog>
    mutable: true
  }

  derived audit_completeness {
    derives_from: [lc.rule_state, rule_changelogs]   // qualified ref
    derivation: "every change to lc.rule_state is paired with a row in rule_changelogs"
  }

  forbidden_state fs_unaudited_state_change {
    description: "..."
    cells: { lc.rule_state: any, rule_changelogs: empty }
    reachability: currently_reachable
    kill_test: kt_audit_completeness
  }

  // ...
}
```

Reference syntax: `<alias>.<id>` resolves to a declaration in the imported slice. The qualified id may appear anywhere a local id can — `derives_from`, `over`, `mutates`, `cells:`, `predicate:` (as text inside the string).

**Loader semantics:** `memspec walk <file>` follows imports transitively, walks each file independently, then runs cross-slice resolution. Use `--single-file` to opt out.

- Paths in `use "..."` are relative to the importing file's directory.
- Cycles are detected and reported (E0401).
- Missing imports report against the importing file's `use` span (E0400).
- Diamond imports (A → B + C, both → D) are loaded once; the cross-slice analyzer doesn't double-count.

**Diagnostic codes:**
- `E0108` `use` declaration after another decl (must be at top of slice body)
- `E0109` missing `as` keyword
- `E0110` duplicate import alias
- `E0400` import path cannot be resolved (file not found)
- `E0401` import cycle detected
- `E0402` qualified ref uses unknown alias (no `use` declaration introduces it)
- `E0403` qualified ref name doesn't resolve in the imported slice

**v0 scope:** imports resolve cell/event/forbidden_state/etc. ids by name. There is no notion of "re-export" — to expose B's cell through A, A must declare its own derived/association on top of `b.cell`. Step ids inside events remain scoped to their parent event; cross-slice step references are not supported (write a derived or post_failure entry instead).

## Lexical conventions

- **Identifiers**: `[a-z][a-z0-9_]*`. Convention: `snake_case`. IDs are unique within a slice.
- **Block syntax**: `keyword name { items }` (allium-inspired; clean to lex).
- **Field syntax** inside blocks: `field_name: value` per line.
- **Strings**: double-quoted, `"..."`, with `\n`, `\"`, `\\` escapes. Triple-quoted `"""..."""` for multi-line.
- **Comments**: `//` to end of line. `/* ... */` for blocks.
- **Lists**: `[a, b, c]` or multi-line `[\n  a,\n  b,\n]`.
- **Maps**: `{ key: value, key: value }` or multi-line.
- **Spans**: every token carries a byte-range. AST nodes inherit. Diagnostics emit `file:line:col` and `byte_start..byte_end`.

## The 8 slots

### `cell` — mutable state holder

```memspec
cell rule_state {
  type: enum<draft | published | archived>
  mutable: true
  default: draft
  ref: "rule.rb:14"           // optional advisory; not parser-validated
  cfg: production              // optional: production | test
  consumers: [                  // optional: aliasing/identity assertions
    "Rule#promote!",
    "Rule#deprecate!",
  ]
  aliasing: identity            // optional: triggers an extra structural-kill-test obligation
  co_published_with: [other_cell_id]  // optional: atomicity-by-construction (slice 1 scrutiny F13)
  impl_hints: { rust: "ActiveRecord enum column" }   // adapter-only; parser ignores
}
```

**Required fields**: `type`, `mutable`. Everything else optional.

### `derived` — state computed from other cells

```memspec
derived rule_is_live {
  derives_from: [rule_state, rule_active]
  derivation: "rule_state == published AND rule_active == true"
  materialised: false           // false = computed-on-call (e.g. `def active?`); true = stored
}
```

**Required**: `derives_from`, `derivation`. `materialised:` defaults to `true`.

### `association` — invariant between cells

```memspec
association no_archived_active {
  invariant: "NOT (rule_state == archived AND rule_active == true)"
  over: [rule_state, rule_active]
  enforced_by: convention       // schema | callback | method | construction_only
                                // | event_handler(event_id) | derivation
                                // | structural_test | convention | db_foreign_key_cascade | none
}
```

**Required**: `invariant`, `over`, `enforced_by`. `enforced_by: event_handler(<id>)` and similar take a parenthesised arg.

### `event` — an operation that mutates cells

```memspec
event promote {
  trigger: "Rule#promote! called by reviewer"
  mutates: [rule_state, rule_active, rule_changelogs]
  atomicity: db_transaction     // none | trivial | db_transaction | transactional
  serialization: single_mutator // optional: single_mutator | concurrent | unspecified
  construction_only: false       // true for events that only fire at row construction (Rule.create!)

  step s1_update {
    op: "rule.update!(state: :published, active: true)"
    fallible: true
    failure_modes: [
      "ActiveRecord::RecordInvalid (validation)",
      "ActiveRecord::StaleObjectError (optimistic lock)",
    ]
    mutates: [rule_state, rule_active]
  }

  step s2_changelog {
    op: "RuleChangelog.create!(...)"
    fallible: true
    failure_modes: ["ActiveRecord::RecordInvalid"]
    mutates: [rule_changelogs]
    precondition: "s1_update succeeded"
  }
}
```

**Required**: `mutates`, at least one `step`. Event-level fields default sensibly. Steps are ordered.

### `post_failure` — what cells look like after a fallible step fails

```memspec
post_failure pf_promote_s2 {
  event: promote
  step:  s2_changelog
  outcome: "Err(ActiveRecord::RecordInvalid)"

  cells_after_pre_rollback {
    rule_state:     published       // s1_update committed
    rule_active:    true
    rule_changelogs: <unchanged>
  }
  cells_after_rollback {
    rule_state:     <unchanged-from-pre-event>   // AR transaction rolled back
    rule_active:    <unchanged-from-pre-event>
    rule_changelogs: <unchanged>
  }
  result: rejected
  invariants_held_after_rollback: [no_archived_active, /* ... */]
}
```

**Required**: `event`, `step`, `outcome`, `cells_after` (or `cells_after_pre_rollback` + `cells_after_rollback` when atomicity is `transactional`/`db_transaction`).

### `forbidden_state` — cell-value combinations that must never be observed

```memspec
forbidden_state fs_archived_active {
  description: "An archived rule that still claims active=true is logically broken."
  predicate: "rule_state == archived AND rule_active == true"
  // OR: cells: { rule_state: archived, rule_active: true }
  reachability: currently_reachable    // currently_reachable | runtime_unreachable | structurally_unreachable
  reachable_via_audited_path: false    // optional; for "the only path out is unaudited" shapes
  kill_test: kt_archived_active
}
```

**Required**: `description`, `predicate` or `cells`, `reachability`, `kill_test` (forward ref).

### `kill_test` — assertion that proves a forbidden state is unreachable

```memspec
kill_test kt_archived_active {
  forbidden: fs_archived_active
  kind: structural   // behavioural | structural | type_shape | property | model_check
  assertion: "No code path produces (rule_state=archived, rule_active=true). Verified by code-search adapter against UPDATABLE_FIELDS and direct update! sites."
  ref: "test/models/rule_test.rb:202-214"   // optional advisory
  status: declared   // declared | resolved | executed_passing | executed_failing
  parts: [           // optional: composite kill-test (slice 2 scrutiny F20)
    { ref: "test/models/rule_test.rb:202-214", proves: "no in-model path produces it" },
    { ref: "db/migrate/2026_..._add_check.rb", proves: "DB CHECK constraint" },
  ]
}
```

**Required**: `forbidden`, `kind`, `assertion`. `status:` defaults to `declared`. The CLI must NEVER print "killed" unless `status: executed_passing`.

## Type vocabulary (abstract, language-agnostic)

```
boolean
enum<value1 | value2 | ...>
set<X>
list<X>
map<K, V>
opaque<name>                    // "an external thing we don't model further"
append_only_relation<X>         // log/audit-trail cells (e.g. RuleChangelog rows)
row_existence<X>                // "the existence of a row" — for construction-only events
reference<cell_id>              // pointer/handle to another cell
```

These are domain types. Implementation types (`ArcSwap<T>`, `ActiveRecord::Relation`, etc.) live in `impl_hints:` metadata the parser ignores.

## Predicate mini-language

Used in `derivation:`, `invariant:`, `forbidden_state.predicate:`, `kill_test.assertion:` (when machine-checkable).

```
expr        = literal | ident | binary | unary | quantifier | call
literal     = INT | STRING | "true" | "false" | enum_value
ident       = cell_id | step_id | "self"
binary      = expr ("==" | "!=" | "<" | "<=" | ">" | ">=" | "AND" | "OR" | "in" | "subset_of") expr
unary       = "NOT" expr
quantifier  = ("forall" | "exists") IDENT "in" expr ":" expr
call        = ident "." ident                    // field access, e.g. cell.field
            | "len" "(" expr ")"
            | "union" "(" expr_list ")"
```

Snapshot operator `expr@step_id` for the value of a cell at the moment a step ran (used in symmetric-failure reasoning, e.g. `rule_state@s1_update.before == draft`).

For v0, `assertion:` text not parseable as a predicate is permitted as free-form — adapters/scrutinizer can interpret. Parser warns but doesn't reject.

## Per-walk provenance

Top-level `walk: N` on the slice. Per-clause provenance fields:

```memspec
forbidden_state fs_archived_active {
  walk_added: 1
  walk_killed: 3        // populated when a kill-test of sufficient kind lands
  ...
}
```

Defaults: `walk_added` defaults to slice's `walk:`. `walk_killed` only meaningful on forbidden-state nodes. `walk_changed` may appear on any node; lists the field(s) that changed in that walk via a `changed_fields:` neighbour or in the optional top-level `walk N { ... }` record.

A second walk of the same slice authors a new `walk 2 { ... }` block at the top + updates per-clause provenance fields. The diff between walks is computable from these fields alone.

## Analyzer checks

Run in this order; later checks may assume earlier ones passed.

### Structural (slot completeness)

- Every required field present per slot.
- Every event with ≥2 fallible steps has a `post_failure` row for **each** (event, step) pair where step is `fallible: true` and follows ≥1 prior fallible-or-mutating step on the same execution path.
- Every `forbidden_state` has a `kill_test:` ref (TODO with reason permitted; analyzer surfaces but doesn't reject).
- Every `kill_test` has a `forbidden:` back-ref to a declared forbidden state.

### Coherence (allium-style; spec is internally sensible)

- All `IDENT`s unique within the slice.
- Every cell-id reference (`derives_from`, `over`, `mutates`, `cells:`, `cells_after:`) resolves to a declared cell.
- Cell-value references in `forbidden_state.cells:` and `post_failure.cells_after:` are within the cell's declared `type` domain.
- `post_failure.event` resolves to a declared event; `post_failure.step` resolves to a step inside that event AND that step has `fallible: true`.
- Derivation graph is acyclic (`derives_from` is a DAG).
- No cell declared but never read by any derived/event/forbidden/kill_test (warn, don't reject — sometimes useful).
- No event declared with empty `mutates:` (warn).
- Kill-test for a `forbidden_state` whose `reachability: structurally_unreachable` is `kind: behavioural` — warn ("redundant behavioural test against type-shape invariant; consider `kind: type_shape`").
- Predicate references inside `predicate:` / `derivation:` / `invariant:` are parseable expressions (when text matches the mini-language) and reference declared identifiers.

### Symmetric-failure (the founding-incident discipline)

- For every event E with steps `[s1, ..., sN]` where ≥2 of the `si` are `fallible: true`:
  - For every fallible step `si` (i > 1) where ≥1 prior step is `fallible: true` OR `mutates: [...]` (non-empty):
    - There MUST be a `post_failure` row with `event: E, step: si` answering "what is the state of all cells if `si` fails after the prior steps succeeded?"
  - If `event.atomicity` ∈ `{transactional, db_transaction}`, the `post_failure` row MUST distinguish `cells_after_pre_rollback` from `cells_after_rollback`.

### Walk-coherence

- Per-clause `walk_added: N` and `walk_killed: M` satisfy `N ≤ M ≤ slice.walk`.
- A `walk K { ... }` record's `added: [...]` matches the IDs whose `walk_added: K`. Same for `killed`.

## CLI surface (recap)

- `memspec walk <slice>` — full structural + coherence + symmetric-failure walk; prints state + gaps; nonzero exit if walk-incomplete or coherence-broken. `--json` emits structured diagnostics.
- `memspec query <slice> [--gaps | --unkilled-forbidden-states | --missing-post-failure | --by-id ID | --reaches FORBIDDEN]` — focused queries returning JSON.
- `memspec render <slice> [--md | --html | --terminal | --graph]` — human/LLM-readable views; `--graph` emits Graphviz DOT or Mermaid.
- `memspec diff <slice> --from walk-N --to walk-M` — per-walk diff in JSON or markdown.
- `memspec schema --json-schema` — emits the JSON schema for `--json` output.

Diagnostic codes namespaced `memspec/E####` (error), `memspec/W####` (warn), `memspec/I####` (info). Codes never reused. Stable exit codes: `0` walk-complete, `1` walk-incomplete (recoverable, gaps in JSON), `2` parse error, `3` schema/coherence error, `4` I/O.

## Out of scope for v0 (explicit)

- Verifying `kill_test.assertion` actually holds against source code.
- Verifying `ref:` strings point at real `file:line` locations in real files.
- Running tests / parsing test results / mapping `kind: behavioural` kill-tests to test framework output.
- ~~Cross-slice `use` / imports / shared cell vocabularies.~~ **Landed in v0** — see "Cross-slice imports" above.
- LSP server. (Agent-native CLI is the only consumer.)
- Persistence binding to an external work-tracker schema.
- Format mutations (CRDT-style merging of concurrent walks).

These all remain valuable; they're adapter/v1+ work. The v0 parser MUST stay structural+coherence-only or it will rot into Sauron's Eye.

## Open questions for v0.5

- **Mode-specific defaults**: should `mode: bugfix` default `walk:` to 2 and require a `walk 1` block describing the broken state? Or stay declarative?
- **Composite kill-tests** (`parts:`) — needs a worked example before the schema is final.
- **`reachable_via_audited_path:`** — this captures FS2-shape (the only escape from this state is unaudited). Useful or premature?
- **Predicate-language ergonomics**: `forall r in resolvers: r.current.bundle == published.bundle` — quantifier syntax is borrowed from math. Is `for x in s where p` more idiomatic? Defer to v0.5; both lex.

## Why this is enough for `cargo new`

The lexer has a clear token vocabulary. The parser has a clear block-item grammar (allium-style). The AST has clear node types per slot. The analyzer has three named check passes with clear ordering. The CLI has six commands with clear contracts. Diagnostic codes and exit codes are pinned. Every piece of the v0 build sequence in CLAUDE.md ("Locked Architecture Decisions" → v0 build sequence) maps to a clear next implementation task.

What's deliberately undecided — the predicate-language exact ergonomics, mode-specific defaults, composite kill-test schema — can be locked after the first real `.memspec` files round-trip through the parser. Those are v0.5 concerns, not v0 blockers.
