//! Pass 2 — coherence (allium-style internal sensibility).
//!
//! Builds a per-slice [`SymbolTable`] and verifies the spec is internally
//! consistent. Per the role-separation lock, this pass operates ENTIRELY
//! on the AST — it never opens source files or runs anything.
//!
//! Checks (per `docs/grammar-v0.md`):
//! - Top-level slot IDs are globally unique within the slice.
//! - Step IDs are unique within their containing event.
//! - Cell-id refs in `derives_from` / `over` / `mutates` / `cells:` resolve
//!   to a declared cell (or derived, where allowed).
//! - `post_failure.event` resolves to a declared event; `post_failure.step`
//!   resolves to a step inside that event AND that step is `fallible: true`.
//! - `forbidden_state.kill_test` ↔ `kill_test.forbidden` is bipartite-consistent.
//! - `derives_from` graph is acyclic.
//!
//! Warnings:
//! - Cell declared but never referenced.
//! - Event with empty `mutates:` list.
//! - Behavioural kill_test for a `reachability: structurally_unreachable`
//!   forbidden_state (suggests `kind: type_shape` instead).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::{BlockDecl, BlockItem, FieldValue, File, Ident, MapEntry};
use crate::diagnostic::{Diagnostic, codes};
use crate::span::Span;

pub fn run(file: &File, out: &mut Vec<Diagnostic>) {
    let Some(slice) = &file.slice else { return };
    let symbols = build_symbol_table(slice, out);
    check_refs(&symbols, out);
    check_bipartite(&symbols, out);
    check_derivation_cycles(&symbols, out);
    check_warnings(&symbols, out);
}

// ---------------------------------------------------------------------------
// Symbol table
// ---------------------------------------------------------------------------

/// Where an ID was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdKind {
    Cell,
    Derived,
    Association,
    Event,
    PostFailure,
    ForbiddenState,
    KillTest,
}

impl IdKind {
    fn label(self) -> &'static str {
        match self {
            Self::Cell => "cell",
            Self::Derived => "derived",
            Self::Association => "association",
            Self::Event => "event",
            Self::PostFailure => "post_failure",
            Self::ForbiddenState => "forbidden_state",
            Self::KillTest => "kill_test",
        }
    }
}

#[derive(Debug)]
struct Decl<'a> {
    id: &'a str,
    span: Span,
    /// Retained for future query-pass use (e.g. ID look-up by kind).
    #[allow(dead_code)]
    kind: IdKind,
    block: &'a BlockDecl,
}

#[derive(Debug, Default)]
struct SymbolTable<'a> {
    cells: BTreeMap<&'a str, Decl<'a>>,
    derived: BTreeMap<&'a str, Decl<'a>>,
    associations: BTreeMap<&'a str, Decl<'a>>,
    events: BTreeMap<&'a str, EventEntry<'a>>,
    post_failures: BTreeMap<&'a str, Decl<'a>>,
    forbidden_states: BTreeMap<&'a str, Decl<'a>>,
    kill_tests: BTreeMap<&'a str, Decl<'a>>,
    /// All declared IDs, indexed by name, for cross-kind duplicate detection.
    by_name: HashMap<&'a str, Vec<(IdKind, Span)>>,
}

#[derive(Debug)]
struct EventEntry<'a> {
    decl: Decl<'a>,
    /// step-id → step BlockDecl
    steps: BTreeMap<&'a str, &'a BlockDecl>,
}

impl<'a> SymbolTable<'a> {
    fn cell_or_derived(&self, id: &str) -> Option<IdKind> {
        if self.cells.contains_key(id) {
            Some(IdKind::Cell)
        } else if self.derived.contains_key(id) {
            Some(IdKind::Derived)
        } else {
            None
        }
    }
}

fn build_symbol_table<'a>(
    slice: &'a crate::ast::SliceDecl,
    out: &mut Vec<Diagnostic>,
) -> SymbolTable<'a> {
    let mut symbols = SymbolTable::default();

    for item in &slice.items {
        let BlockItem::Block(block) = item else { continue };
        let kind = match block.kind.name.as_str() {
            "cell" => IdKind::Cell,
            "derived" => IdKind::Derived,
            "association" => IdKind::Association,
            "event" => IdKind::Event,
            "post_failure" => IdKind::PostFailure,
            "forbidden_state" => IdKind::ForbiddenState,
            "kill_test" => IdKind::KillTest,
            _ => continue, // meta, walk, etc.
        };
        let Some(name_ident) = block_name_ident(block) else {
            // Structural pass already flagged missing name; skip here.
            continue;
        };
        let id = name_ident.name.as_str();
        let span = name_ident.span;

        // Cross-kind duplicate detection.
        if let Some(prior) = symbols.by_name.get(id).and_then(|v| v.first()) {
            out.push(
                Diagnostic::error(
                    codes::E_COH_DUPLICATE_ID,
                    span,
                    format!(
                        "id `{id}` is already declared as a {} in this slice",
                        prior.0.label()
                    ),
                )
                .with_hint("each top-level slot ID must be unique within a slice"),
            );
            // Don't insert duplicate into per-kind tables; first wins.
            continue;
        }
        symbols.by_name.entry(id).or_default().push((kind, span));

        let decl = Decl { id, span, kind, block };
        match kind {
            IdKind::Cell => {
                symbols.cells.insert(id, decl);
            }
            IdKind::Derived => {
                symbols.derived.insert(id, decl);
            }
            IdKind::Association => {
                symbols.associations.insert(id, decl);
            }
            IdKind::Event => {
                let mut steps = BTreeMap::new();
                for inner in &block.items {
                    if let BlockItem::Block(step_block) = inner {
                        if step_block.kind.name == "step" {
                            if let Some(step_name) = block_name_ident(step_block) {
                                if steps.contains_key(step_name.name.as_str()) {
                                    out.push(
                                        Diagnostic::error(
                                            codes::E_COH_DUPLICATE_STEP_ID,
                                            step_name.span,
                                            format!(
                                                "step `{}` is declared twice in event `{id}`",
                                                step_name.name
                                            ),
                                        )
                                        .with_hint("step IDs must be unique within their event"),
                                    );
                                } else {
                                    steps.insert(step_name.name.as_str(), step_block);
                                }
                            }
                        }
                    }
                }
                symbols.events.insert(id, EventEntry { decl, steps });
            }
            IdKind::PostFailure => {
                symbols.post_failures.insert(id, decl);
            }
            IdKind::ForbiddenState => {
                symbols.forbidden_states.insert(id, decl);
            }
            IdKind::KillTest => {
                symbols.kill_tests.insert(id, decl);
            }
        }
    }

    symbols
}

// ---------------------------------------------------------------------------
// Ref resolution
// ---------------------------------------------------------------------------

fn check_refs(symbols: &SymbolTable<'_>, out: &mut Vec<Diagnostic>) {
    // derived.derives_from: cells OR derived
    for d in symbols.derived.values() {
        if let Some(value) = field_value(d.block, "derives_from") {
            for (id, span) in collect_idents(value) {
                if symbols.cell_or_derived(id).is_none() {
                    out.push(unresolved_cell(id, span));
                }
            }
        }
    }

    // association.over: cells OR derived
    for a in symbols.associations.values() {
        if let Some(value) = field_value(a.block, "over") {
            for (id, span) in collect_idents(value) {
                if symbols.cell_or_derived(id).is_none() {
                    out.push(unresolved_cell(id, span));
                }
            }
        }
    }

    // event.mutates + step.mutates: cells only
    for e in symbols.events.values() {
        if let Some(value) = field_value(e.decl.block, "mutates") {
            for (id, span) in collect_idents(value) {
                if !symbols.cells.contains_key(id) {
                    out.push(unresolved_cell(id, span));
                }
            }
        }
        for step in e.steps.values() {
            if let Some(value) = field_value(step, "mutates") {
                for (id, span) in collect_idents(value) {
                    if !symbols.cells.contains_key(id) {
                        out.push(unresolved_cell(id, span));
                    }
                }
            }
        }
    }

    // forbidden_state.cells (field or anonymous block)
    for fs in symbols.forbidden_states.values() {
        if let Some(value) = field_value(fs.block, "cells") {
            for (id, span) in collect_map_keys(value) {
                if !symbols.cells.contains_key(id) {
                    out.push(unresolved_cell(id, span));
                }
            }
        }
        // Also accept `cells { ... }` as an anonymous block of fields.
        if let Some(cells_block) = nested_block(fs.block, "cells") {
            for item in &cells_block.items {
                if let BlockItem::Field(f) = item {
                    let key = f.key.name.as_str();
                    if !symbols.cells.contains_key(key) {
                        out.push(unresolved_cell(key, f.key.span));
                    }
                }
            }
        }
    }

    // post_failure.cells_after / pre_rollback / post_rollback (anonymous blocks
    // OR field map). Each key must resolve to a declared cell.
    for pf in symbols.post_failures.values() {
        for variant in ["cells_after", "cells_after_pre_rollback", "cells_after_rollback"] {
            if let Some(value) = field_value(pf.block, variant) {
                for (id, span) in collect_map_keys(value) {
                    if !symbols.cells.contains_key(id) {
                        out.push(unresolved_cell(id, span));
                    }
                }
            }
            if let Some(b) = nested_block(pf.block, variant) {
                for item in &b.items {
                    if let BlockItem::Field(f) = item {
                        let key = f.key.name.as_str();
                        if !symbols.cells.contains_key(key) {
                            out.push(unresolved_cell(key, f.key.span));
                        }
                    }
                }
            }
        }

        // post_failure.event / step
        if let Some(FieldValue::Ident(ev_ref)) = field_value(pf.block, "event") {
            let ev_name = ev_ref.name.as_str();
            match symbols.events.get(ev_name) {
                None => out.push(Diagnostic::error(
                    codes::E_COH_UNRESOLVED_EVENT_REF,
                    ev_ref.span,
                    format!("post_failure references unknown event `{ev_name}`"),
                )),
                Some(ev) => {
                    if let Some(FieldValue::Ident(step_ref)) = field_value(pf.block, "step") {
                        let step_name = step_ref.name.as_str();
                        match ev.steps.get(step_name) {
                            None => out.push(Diagnostic::error(
                                codes::E_COH_UNRESOLVED_STEP_REF,
                                step_ref.span,
                                format!(
                                    "post_failure references unknown step `{step_name}` in event `{ev_name}`"
                                ),
                            )),
                            Some(step) => {
                                if !field_is_true(step, "fallible") {
                                    out.push(Diagnostic::error(
                                        codes::E_COH_STEP_NOT_FALLIBLE,
                                        step_ref.span,
                                        format!(
                                            "post_failure targets step `{step_name}` in event `{ev_name}`, but that step is not declared `fallible: true`"
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn unresolved_cell(id: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::E_COH_UNRESOLVED_CELL_REF,
        span,
        format!("reference to unknown cell `{id}`"),
    )
    .with_hint("declare it as `cell <id> { ... }` or fix the typo")
}

// ---------------------------------------------------------------------------
// Bipartite kill_test ↔ forbidden_state consistency
// ---------------------------------------------------------------------------

fn check_bipartite(symbols: &SymbolTable<'_>, out: &mut Vec<Diagnostic>) {
    // forward: forbidden_state.kill_test → must point at a real kill_test
    // (special: literal `TODO` permitted)
    for fs in symbols.forbidden_states.values() {
        if let Some(FieldValue::Ident(kt_ref)) = field_value(fs.block, "kill_test") {
            let kt_name = kt_ref.name.as_str();
            if kt_name == "TODO" {
                continue;
            }
            match symbols.kill_tests.get(kt_name) {
                None => out.push(
                    Diagnostic::error(
                        codes::E_COH_UNRESOLVED_KILL_TEST_REF,
                        kt_ref.span,
                        format!("forbidden_state `{}` references unknown kill_test `{kt_name}`", fs.id),
                    )
                    .with_hint("declare it as `kill_test <id> { ... }` or use `kill_test: TODO`"),
                ),
                Some(kt) => {
                    // bipartite back-ref check
                    if let Some(FieldValue::Ident(back)) = field_value(kt.block, "forbidden") {
                        if back.name != fs.id {
                            out.push(Diagnostic::error(
                                codes::E_COH_BIPARTITE_MISMATCH,
                                back.span,
                                format!(
                                    "kill_test `{}` declares `forbidden: {}` but is referenced by forbidden_state `{}`",
                                    kt_name, back.name, fs.id
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }

    // reverse: kill_test.forbidden → must point at a real forbidden_state
    for kt in symbols.kill_tests.values() {
        if let Some(FieldValue::Ident(fs_ref)) = field_value(kt.block, "forbidden") {
            let fs_name = fs_ref.name.as_str();
            if !symbols.forbidden_states.contains_key(fs_name) {
                out.push(
                    Diagnostic::error(
                        codes::E_COH_UNRESOLVED_FORBIDDEN_REF,
                        fs_ref.span,
                        format!("kill_test `{}` references unknown forbidden_state `{fs_name}`", kt.id),
                    )
                    .with_hint("declare it as `forbidden_state <id> { ... }`"),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Derivation cycle detection
// ---------------------------------------------------------------------------

fn check_derivation_cycles(symbols: &SymbolTable<'_>, out: &mut Vec<Diagnostic>) {
    // Build adjacency: derived-id → [source-id ...]
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for d in symbols.derived.values() {
        let mut sources = Vec::new();
        if let Some(value) = field_value(d.block, "derives_from") {
            for (id, _) in collect_idents(value) {
                sources.push(id);
            }
        }
        adj.insert(d.id, sources);
    }

    // DFS for cycles. Color: 0 = white, 1 = gray (on stack), 2 = black.
    let mut color: HashMap<&str, u8> = HashMap::new();
    for &id in adj.keys() {
        if color.get(id).copied().unwrap_or(0) == 0 {
            let mut stack = vec![(id, 0usize)];
            color.insert(id, 1);
            while let Some(&mut (node, ref mut idx)) = stack.last_mut() {
                let neighbors = adj.get(node).map(Vec::as_slice).unwrap_or(&[]);
                if *idx < neighbors.len() {
                    let next = neighbors[*idx];
                    *idx += 1;
                    // Only recurse if next is itself a derived (forms part of the graph).
                    if !adj.contains_key(next) {
                        continue;
                    }
                    match color.get(next).copied().unwrap_or(0) {
                        0 => {
                            color.insert(next, 1);
                            stack.push((next, 0));
                        }
                        1 => {
                            // Back edge → cycle.
                            let span = symbols
                                .derived
                                .get(next)
                                .map(|d| d.span)
                                .unwrap_or(Span::DUMMY);
                            out.push(Diagnostic::error(
                                codes::E_COH_DERIVATION_CYCLE,
                                span,
                                format!(
                                    "cycle in `derives_from` graph involving `{next}` (reachable from `{node}`)"
                                ),
                            ));
                        }
                        _ => {}
                    }
                } else {
                    color.insert(node, 2);
                    stack.pop();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Warnings
// ---------------------------------------------------------------------------

fn check_warnings<'a>(symbols: &SymbolTable<'a>, out: &mut Vec<Diagnostic>) {
    // Build set of cell-ids actually referenced anywhere. The IDs borrow
    // from the AST through the symbol table; lifetime 'a flows naturally.
    let mut referenced: HashSet<&'a str> = HashSet::new();

    fn add_idents<'a>(value: &'a FieldValue, out: &mut HashSet<&'a str>) {
        match value {
            FieldValue::Ident(i) => {
                out.insert(i.name.as_str());
            }
            FieldValue::List { items, .. } => {
                for it in items {
                    add_idents(it, out);
                }
            }
            _ => {}
        }
    }

    fn add_map_keys<'a>(value: &'a FieldValue, out: &mut HashSet<&'a str>) {
        if let FieldValue::Map { entries, .. } = value {
            for entry in entries {
                out.insert(entry.key.name.as_str());
            }
        }
    }

    fn add_field<'a>(block: &'a BlockDecl, key: &str, out: &mut HashSet<&'a str>) {
        if let Some(value) = field_value(block, key) {
            add_idents(value, out);
            add_map_keys(value, out);
        }
    }

    fn add_anon_block_field_names<'a>(block: &'a BlockDecl, kind: &str, out: &mut HashSet<&'a str>) {
        if let Some(b) = nested_block(block, kind) {
            for item in &b.items {
                if let BlockItem::Field(f) = item {
                    out.insert(f.key.name.as_str());
                }
            }
        }
    }

    for d in symbols.derived.values() {
        add_field(d.block, "derives_from", &mut referenced);
    }
    for a in symbols.associations.values() {
        add_field(a.block, "over", &mut referenced);
    }
    for e in symbols.events.values() {
        add_field(e.decl.block, "mutates", &mut referenced);
        for step in e.steps.values() {
            add_field(step, "mutates", &mut referenced);
        }
    }
    for fs in symbols.forbidden_states.values() {
        add_field(fs.block, "cells", &mut referenced);
        add_anon_block_field_names(fs.block, "cells", &mut referenced);
    }
    for pf in symbols.post_failures.values() {
        for variant in ["cells_after", "cells_after_pre_rollback", "cells_after_rollback"] {
            add_field(pf.block, variant, &mut referenced);
            add_anon_block_field_names(pf.block, variant, &mut referenced);
        }
    }

    for cell in symbols.cells.values() {
        if !referenced.contains(cell.id) {
            out.push(
                Diagnostic::warning(
                    codes::W_COH_UNUSED_CELL,
                    cell.span,
                    format!("cell `{}` is declared but never referenced", cell.id),
                )
                .with_hint("either reference it from a derived/event/forbidden_state, or remove it"),
            );
        }
    }

    // Event with empty mutates list.
    for e in symbols.events.values() {
        if let Some(FieldValue::List { items, .. }) = field_value(e.decl.block, "mutates") {
            if items.is_empty() {
                out.push(Diagnostic::warning(
                    codes::W_COH_EVENT_EMPTY_MUTATES,
                    e.decl.span,
                    format!("event `{}` declares an empty `mutates: []` list", e.decl.id),
                ));
            }
        }
    }

    // Behavioural kill_test for structurally_unreachable forbidden state.
    for fs in symbols.forbidden_states.values() {
        let reachability = field_value(fs.block, "reachability")
            .and_then(|v| match v {
                FieldValue::Ident(i) => Some(i.name.as_str()),
                _ => None,
            });
        if reachability != Some("structurally_unreachable") {
            continue;
        }
        let Some(FieldValue::Ident(kt_ref)) = field_value(fs.block, "kill_test") else { continue };
        let Some(kt) = symbols.kill_tests.get(kt_ref.name.as_str()) else { continue };
        if let Some(FieldValue::Ident(kind)) = field_value(kt.block, "kind") {
            if kind.name == "behavioural" {
                out.push(
                    Diagnostic::warning(
                        codes::W_COH_REDUNDANT_KILL_TEST,
                        kind.span,
                        format!(
                            "kill_test `{}` is `kind: behavioural` for forbidden_state `{}` whose `reachability: structurally_unreachable` — consider `kind: type_shape` instead",
                            kt.id, fs.id
                        ),
                    )
                    .with_hint("type-shape kill-tests assert the structural property that makes the state unreachable"),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AST navigation helpers
// ---------------------------------------------------------------------------

fn block_name_ident(block: &BlockDecl) -> Option<&Ident> {
    use crate::ast::BlockName;
    match &block.name {
        Some(BlockName::Ident(i)) => Some(i),
        _ => None,
    }
}

fn field_value<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a FieldValue> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Field(f) if f.key.name == key => Some(&f.value),
        _ => None,
    })
}

fn nested_block<'a>(block: &'a BlockDecl, kind: &str) -> Option<&'a BlockDecl> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Block(b) if b.kind.name == kind => Some(b),
        _ => None,
    })
}

fn field_is_true(block: &BlockDecl, key: &str) -> bool {
    matches!(field_value(block, key), Some(FieldValue::Bool { value: true, .. }))
}

/// Collect identifiers from a field value. Lists are flattened; non-ident
/// items are skipped.
fn collect_idents(value: &FieldValue) -> Vec<(&str, Span)> {
    let mut out = Vec::new();
    match value {
        FieldValue::Ident(i) => out.push((i.name.as_str(), i.span)),
        FieldValue::List { items, .. } => {
            for it in items {
                out.extend(collect_idents(it));
            }
        }
        _ => {}
    }
    out
}

/// Collect map keys from a `{ k: v }` value.
fn collect_map_keys(value: &FieldValue) -> Vec<(&str, Span)> {
    match value {
        FieldValue::Map { entries, .. } => entries
            .iter()
            .map(|MapEntry { key, .. }| (key.name.as_str(), key.span))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;
    use crate::parser::parse;

    fn analyze_str(src: &str) -> Vec<Diagnostic> {
        let pr = parse(src);
        let mut diags = Vec::new();
        run(&pr.file, &mut diags);
        diags
    }

    #[test]
    fn duplicate_id_across_kinds_emits_diagnostic() {
        let d = analyze_str("slice s { cell foo { type: boolean mutable: true } event foo { mutates: [foo] step s { op: \"x\" fallible: true } } }");
        assert!(
            d.iter().any(|d| d.code == codes::E_COH_DUPLICATE_ID),
            "expected duplicate-id diagnostic, got: {d:#?}"
        );
    }

    #[test]
    fn unresolved_cell_ref_in_mutates() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [does_not_exist]
                    step s1 { op: "x" fallible: true mutates: [does_not_exist] }
                }
            }"#,
        );
        let unresolved: Vec<_> = d.iter().filter(|d| d.code == codes::E_COH_UNRESOLVED_CELL_REF).collect();
        assert!(unresolved.len() >= 2, "expected ≥2 unresolved-cell diagnostics (event + step)");
    }

    #[test]
    fn unresolved_cell_ref_in_derives_from() {
        let d = analyze_str(
            r#"slice s {
                cell a { type: boolean mutable: true }
                derived d {
                    derives_from: [a, ghost]
                    derivation: "a"
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_COH_UNRESOLVED_CELL_REF));
    }

    #[test]
    fn forbidden_state_kill_test_resolves() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                forbidden_state fs {
                    description: "x"
                    predicate: "c == true"
                    reachability: currently_reachable
                    kill_test: kt
                }
                kill_test kt {
                    forbidden: fs
                    kind: structural
                    assertion: "x"
                    status: declared
                }
            }"#,
        );
        let errors: Vec<_> = d.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "expected clean coherence pass, got: {errors:#?}");
    }

    #[test]
    fn bipartite_mismatch_caught() {
        let d = analyze_str(
            r#"slice s {
                forbidden_state fs1 {
                    description: "x"
                    predicate: "x"
                    reachability: currently_reachable
                    kill_test: kt
                }
                forbidden_state fs2 {
                    description: "x"
                    predicate: "x"
                    reachability: currently_reachable
                    kill_test: TODO
                }
                kill_test kt {
                    forbidden: fs2
                    kind: structural
                    assertion: "x"
                    status: declared
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_COH_BIPARTITE_MISMATCH));
    }

    #[test]
    fn kill_test_forbidden_unresolved() {
        let d = analyze_str(
            r#"slice s {
                kill_test kt {
                    forbidden: ghost
                    kind: structural
                    assertion: "x"
                    status: declared
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_COH_UNRESOLVED_FORBIDDEN_REF));
    }

    #[test]
    fn forbidden_state_kill_test_todo_no_error() {
        let d = analyze_str(
            r#"slice s {
                forbidden_state fs {
                    description: "x"
                    predicate: "x"
                    reachability: currently_reachable
                    kill_test: TODO
                }
            }"#,
        );
        assert!(!d.iter().any(|d| d.code == codes::E_COH_UNRESOLVED_KILL_TEST_REF));
    }

    #[test]
    fn unused_cell_warning() {
        let d = analyze_str(
            r#"slice s {
                cell unused { type: boolean mutable: true }
                cell used { type: boolean mutable: true }
                event e {
                    mutates: [used]
                    step s1 { op: "x" fallible: true mutates: [used] }
                }
            }"#,
        );
        let warnings: Vec<_> = d.iter().filter(|d| d.code == codes::W_COH_UNUSED_CELL).collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unused"));
    }

    #[test]
    fn derivation_cycle_caught() {
        let d = analyze_str(
            r#"slice s {
                cell base { type: boolean mutable: true }
                derived d1 {
                    derives_from: [d2]
                    derivation: "d2"
                }
                derived d2 {
                    derives_from: [d1]
                    derivation: "d1"
                }
            }"#,
        );
        assert!(
            d.iter().any(|d| d.code == codes::E_COH_DERIVATION_CYCLE),
            "expected derivation-cycle diagnostic, got: {d:#?}"
        );
    }

    #[test]
    fn post_failure_step_must_be_fallible() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    step s1 { op: "x" fallible: false mutates: [c] }
                }
                post_failure pf {
                    event: e
                    step: s1
                    outcome: "Err"
                    cells_after_pre_rollback { c: true }
                    cells_after_rollback { c: false }
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_COH_STEP_NOT_FALLIBLE));
    }

    #[test]
    fn post_failure_unknown_event() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                post_failure pf {
                    event: ghost
                    step: s1
                    outcome: "Err"
                    cells_after { c: true }
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_COH_UNRESOLVED_EVENT_REF));
    }

    #[test]
    fn redundant_behavioural_kill_test_warning() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                forbidden_state fs {
                    description: "x"
                    predicate: "c == true"
                    reachability: structurally_unreachable
                    kill_test: kt
                }
                kill_test kt {
                    forbidden: fs
                    kind: behavioural
                    assertion: "x"
                    status: declared
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::W_COH_REDUNDANT_KILL_TEST));
    }

    #[test]
    fn canonical_fixture_passes_coherence() {
        let src = include_str!("../../tests/fixtures/rule_lifecycle_minimal.memspec");
        let pr = parse(src);
        assert!(pr.diagnostics.is_empty(), "fixture should parse cleanly");
        let mut diags = Vec::new();
        run(&pr.file, &mut diags);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(
            errors.is_empty(),
            "canonical fixture should pass coherence pass; got errors: {errors:#?}"
        );
    }
}
