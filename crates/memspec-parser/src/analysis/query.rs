//! Agent-facing inspection surface — `memspec query`.
//!
//! Four subqueries:
//! - `list_ids` — every declared ID, grouped by slot kind.
//! - `by_id` — full declaration JSON for a given ID.
//! - `refs_to` — every site that points at a given ID, with span + role.
//! - `gaps` — structured gap analysis (unkilled forbidden states,
//!   `kill_test: TODO`, missing post_failure rows, unused cells).
//!
//! All operate on AST alone — no source-code access, no test running.
//! Outputs are `serde::Serialize`able report types ready for JSON.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File, Ident, MapEntry, SliceDecl};
use crate::span::Span;

// ---------------------------------------------------------------------------
// list-ids
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct ListIdsReport {
    pub slice: Option<String>,
    pub cells: Vec<String>,
    pub derived: Vec<String>,
    pub associations: Vec<String>,
    pub events: Vec<EventListing>,
    pub post_failures: Vec<String>,
    pub forbidden_states: Vec<String>,
    pub kill_tests: Vec<String>,
    pub walks: Vec<i64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct EventListing {
    pub id: String,
    pub steps: Vec<String>,
}

pub fn list_ids(file: &File) -> ListIdsReport {
    let mut report = ListIdsReport::default();
    let Some(slice) = &file.slice else { return report };
    report.slice = Some(slice.name.name.clone());

    for item in &slice.items {
        let BlockItem::Block(b) = item else { continue };
        match b.kind.name.as_str() {
            "cell" => push_named(&mut report.cells, b),
            "derived" => push_named(&mut report.derived, b),
            "association" => push_named(&mut report.associations, b),
            "event" => {
                if let Some(name) = block_name_str(b) {
                    let steps: Vec<String> = b
                        .items
                        .iter()
                        .filter_map(|i| match i {
                            BlockItem::Block(s) if s.kind.name == "step" => {
                                block_name_str(s).map(str::to_owned)
                            }
                            _ => None,
                        })
                        .collect();
                    report.events.push(EventListing { id: name.to_owned(), steps });
                }
            }
            "post_failure" => push_named(&mut report.post_failures, b),
            "forbidden_state" => push_named(&mut report.forbidden_states, b),
            "kill_test" => push_named(&mut report.kill_tests, b),
            "walk" => {
                if let Some(BlockName::Int { value, .. }) = &b.name {
                    report.walks.push(*value);
                }
            }
            _ => {}
        }
    }
    report
}

fn push_named(out: &mut Vec<String>, block: &BlockDecl) {
    if let Some(n) = block_name_str(block) {
        out.push(n.to_owned());
    }
}

fn block_name_str(block: &BlockDecl) -> Option<&str> {
    match &block.name {
        Some(BlockName::Ident(i)) => Some(i.name.as_str()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// by-id
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ByIdReport<'a> {
    pub id: String,
    pub kind: String,
    pub span: Span,
    pub block: &'a BlockDecl,
}

pub fn by_id<'a>(file: &'a File, id: &str) -> Option<ByIdReport<'a>> {
    let slice = file.slice.as_ref()?;
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            if let Some(name) = block_name_str(b) {
                if name == id {
                    return Some(ByIdReport {
                        id: id.to_owned(),
                        kind: b.kind.name.clone(),
                        span: b.span,
                        block: b,
                    });
                }
                // Also walk into events to find scoped step ids.
                if b.kind.name == "event" {
                    for inner in &b.items {
                        if let BlockItem::Block(step) = inner {
                            if step.kind.name == "step" {
                                if let Some(step_name) = block_name_str(step) {
                                    if step_name == id {
                                        return Some(ByIdReport {
                                            id: format!("{name}.{step_name}"),
                                            kind: "step".to_owned(),
                                            span: step.span,
                                            block: step,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// refs-to
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RefsReport {
    pub id: String,
    pub references: Vec<RefSite>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RefSite {
    /// Path-like locator: `event/promote/mutates`, `forbidden_state/fs/cells`,
    /// `kill_test/kt/forbidden`, etc.
    pub from: String,
    pub field: String,
    pub span: Span,
}

pub fn refs_to(file: &File, id: &str) -> RefsReport {
    let mut report = RefsReport { id: id.to_owned(), references: Vec::new() };
    let Some(slice) = &file.slice else { return report };

    for item in &slice.items {
        let BlockItem::Block(b) = item else { continue };
        let owner_id = block_name_str(b).unwrap_or("<anon>");
        let owner_path = format!("{}/{}", b.kind.name, owner_id);
        scan_block_refs(b, &owner_path, id, &mut report.references);
    }
    report
}

fn scan_block_refs(block: &BlockDecl, owner_path: &str, target: &str, out: &mut Vec<RefSite>) {
    for item in &block.items {
        match item {
            BlockItem::Field(f) => scan_value_refs(&f.value, owner_path, &f.key.name, target, out),
            BlockItem::Block(inner) => {
                let inner_path = match &inner.name {
                    Some(BlockName::Ident(i)) => format!("{owner_path}/{}/{}", inner.kind.name, i.name),
                    Some(BlockName::Int { value, .. }) => {
                        format!("{owner_path}/{}/{}", inner.kind.name, value)
                    }
                    None => format!("{owner_path}/{}", inner.kind.name),
                };
                scan_block_refs(inner, &inner_path, target, out);
            }
        }
    }
}

fn scan_value_refs(
    value: &FieldValue,
    owner_path: &str,
    field: &str,
    target: &str,
    out: &mut Vec<RefSite>,
) {
    match value {
        FieldValue::Ident(i) => {
            if i.name == target {
                out.push(RefSite {
                    from: owner_path.to_owned(),
                    field: field.to_owned(),
                    span: i.span,
                });
            }
        }
        FieldValue::List { items, .. } => {
            for it in items {
                scan_value_refs(it, owner_path, field, target, out);
            }
        }
        FieldValue::Map { entries, .. } => {
            for MapEntry { key, value, .. } in entries {
                if key.name == target {
                    out.push(RefSite {
                        from: owner_path.to_owned(),
                        field: format!("{field}.<map-key>"),
                        span: key.span,
                    });
                }
                scan_value_refs(value, owner_path, field, target, out);
            }
        }
        FieldValue::TypeApp { params, .. } => {
            for p in params {
                scan_value_refs(p, owner_path, field, target, out);
            }
        }
        FieldValue::Call { args, .. } => {
            for a in args {
                scan_value_refs(a, owner_path, field, target, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// gaps
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct GapsReport {
    pub unkilled_forbidden_states: Vec<UnkilledFS>,
    pub kill_tests_unresolved: Vec<UnresolvedKT>,
    pub missing_post_failure: Vec<MissingPF>,
    pub unused_cells: Vec<UnusedCell>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UnkilledFS {
    pub id: String,
    pub kill_test_ref: Option<String>,
    pub reason: &'static str,
    pub span: Span,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UnresolvedKT {
    pub id: String,
    pub status: String,
    pub span: Span,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct MissingPF {
    pub event: String,
    pub step: String,
    pub span: Span,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UnusedCell {
    pub id: String,
    pub span: Span,
}

pub fn gaps(file: &File) -> GapsReport {
    let mut report = GapsReport::default();
    let Some(slice) = &file.slice else { return report };

    let mut declared_kill_tests: BTreeMap<&str, &BlockDecl> = BTreeMap::new();
    let mut declared_cells: BTreeMap<&str, (Span, &BlockDecl)> = BTreeMap::new();
    let mut declared_pf: BTreeMap<(&str, &str), &BlockDecl> = BTreeMap::new();

    for item in &slice.items {
        let BlockItem::Block(b) = item else { continue };
        match b.kind.name.as_str() {
            "cell" => {
                if let Some(n) = block_name_str(b) {
                    declared_cells.insert(n, (b.span, b));
                }
            }
            "kill_test" => {
                if let Some(n) = block_name_str(b) {
                    declared_kill_tests.insert(n, b);
                }
            }
            "post_failure" => {
                let event = field_ident(b, "event").map(|i| i.name.as_str());
                let step = field_ident(b, "step").map(|i| i.name.as_str());
                if let (Some(e), Some(s)) = (event, step) {
                    declared_pf.insert((e, s), b);
                }
            }
            _ => {}
        }
    }

    // Unkilled forbidden states + kill_tests_unresolved.
    for item in &slice.items {
        let BlockItem::Block(b) = item else { continue };
        if b.kind.name != "forbidden_state" {
            continue;
        }
        let Some(fs_id) = block_name_str(b) else { continue };
        let kt_field = field_value(b, "kill_test");
        match kt_field {
            None => report.unkilled_forbidden_states.push(UnkilledFS {
                id: fs_id.to_owned(),
                kill_test_ref: None,
                reason: "no kill_test field declared",
                span: b.span,
            }),
            Some(FieldValue::Ident(i)) => {
                let name = i.name.as_str();
                if name == "TODO" {
                    report.unkilled_forbidden_states.push(UnkilledFS {
                        id: fs_id.to_owned(),
                        kill_test_ref: Some("TODO".to_owned()),
                        reason: "kill_test obligation marked TODO",
                        span: i.span,
                    });
                } else if let Some(kt) = declared_kill_tests.get(name) {
                    let status = field_ident(kt, "status").map(|i| i.name.as_str()).unwrap_or("declared");
                    if status != "executed_passing" {
                        report.unkilled_forbidden_states.push(UnkilledFS {
                            id: fs_id.to_owned(),
                            kill_test_ref: Some(name.to_owned()),
                            reason: match status {
                                "executed_failing" => "kill_test status: executed_failing",
                                "resolved" => "kill_test resolved but not executed",
                                _ => "kill_test status: declared (not yet resolved or executed)",
                            },
                            span: i.span,
                        });
                    }
                } else {
                    // Unresolved ref — coherence pass already errors; still surface in gaps.
                    report.unkilled_forbidden_states.push(UnkilledFS {
                        id: fs_id.to_owned(),
                        kill_test_ref: Some(name.to_owned()),
                        reason: "kill_test reference does not resolve",
                        span: i.span,
                    });
                }
            }
            _ => {}
        }
    }

    // kill_tests with non-passing status.
    for (id, kt) in &declared_kill_tests {
        let status = field_ident(kt, "status")
            .map(|i| i.name.as_str())
            .unwrap_or("declared");
        if status != "executed_passing" {
            report.kill_tests_unresolved.push(UnresolvedKT {
                id: (*id).to_owned(),
                status: status.to_owned(),
                span: kt.span,
            });
        }
    }

    // Missing post_failure rows (mirror the symmetric-failure pass).
    for item in &slice.items {
        let BlockItem::Block(event) = item else { continue };
        if event.kind.name != "event" {
            continue;
        }
        let Some(event_name) = block_name_str(event) else { continue };
        let steps: Vec<(&Ident, &BlockDecl)> = event
            .items
            .iter()
            .filter_map(|it| match it {
                BlockItem::Block(b) if b.kind.name == "step" => match &b.name {
                    Some(BlockName::Ident(n)) => Some((n, b)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let fallible_count = steps
            .iter()
            .filter(|(_, b)| field_is_true(b, "fallible"))
            .count();
        if fallible_count < 2 {
            continue;
        }
        for (i, (step_name, step_block)) in steps.iter().enumerate() {
            if i == 0 || !field_is_true(step_block, "fallible") {
                continue;
            }
            let prior_observable = steps[..i].iter().any(|(_, prior)| {
                field_is_true(prior, "fallible")
                    || matches!(field_value(prior, "mutates"),
                                Some(FieldValue::List { items, .. }) if !items.is_empty())
            });
            if !prior_observable {
                continue;
            }
            if !declared_pf.contains_key(&(event_name, step_name.name.as_str())) {
                report.missing_post_failure.push(MissingPF {
                    event: event_name.to_owned(),
                    step: step_name.name.clone(),
                    span: step_name.span,
                });
            }
        }
    }

    // Unused cells.
    let referenced = collect_referenced_cells(slice);
    for (cell_id, (span, _)) in &declared_cells {
        if !referenced.contains(cell_id) {
            report.unused_cells.push(UnusedCell {
                id: (*cell_id).to_owned(),
                span: *span,
            });
        }
    }

    report
}

fn collect_referenced_cells<'a>(slice: &'a SliceDecl) -> std::collections::HashSet<&'a str> {
    let mut out = std::collections::HashSet::new();

    fn visit_value<'a>(value: &'a FieldValue, out: &mut std::collections::HashSet<&'a str>) {
        match value {
            FieldValue::Ident(i) => {
                out.insert(i.name.as_str());
            }
            FieldValue::List { items, .. } => {
                for it in items {
                    visit_value(it, out);
                }
            }
            FieldValue::Map { entries, .. } => {
                for entry in entries {
                    out.insert(entry.key.name.as_str());
                    visit_value(&entry.value, out);
                }
            }
            FieldValue::TypeApp { params, .. } => {
                for p in params {
                    visit_value(p, out);
                }
            }
            FieldValue::Call { args, .. } => {
                for a in args {
                    visit_value(a, out);
                }
            }
            _ => {}
        }
    }

    fn visit_block<'a>(block: &'a BlockDecl, out: &mut std::collections::HashSet<&'a str>) {
        // Capture both field-form and anonymous-block-form references.
        for item in &block.items {
            match item {
                BlockItem::Field(f) => visit_value(&f.value, out),
                BlockItem::Block(b) => {
                    // Anonymous blocks like cells / cells_after carry cell-ids
                    // as their inner field keys.
                    if matches!(b.kind.name.as_str(),
                                "cells" | "cells_after"
                                | "cells_after_pre_rollback" | "cells_after_rollback") {
                        for inner in &b.items {
                            if let BlockItem::Field(f) = inner {
                                out.insert(f.key.name.as_str());
                            }
                        }
                    }
                    visit_block(b, out);
                }
            }
        }
    }

    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            visit_block(b, &mut out);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// AST helpers (local to keep query self-contained)
// ---------------------------------------------------------------------------

fn field_value<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a FieldValue> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Field(f) if f.key.name == key => Some(&f.value),
        _ => None,
    })
}

fn field_ident<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a Ident> {
    match field_value(block, key) {
        Some(FieldValue::Ident(i)) => Some(i),
        _ => None,
    }
}

fn field_is_true(block: &BlockDecl, key: &str) -> bool {
    matches!(field_value(block, key), Some(FieldValue::Bool { value: true, .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn parse_fixture() -> File {
        let src = include_str!("../../tests/fixtures/rule_lifecycle_minimal.memspec");
        parse(src).file
    }

    #[test]
    fn list_ids_finds_all_canonical_ids() {
        let f = parse_fixture();
        let r = list_ids(&f);
        assert_eq!(r.slice.as_deref(), Some("rule_lifecycle_minimal"));
        assert_eq!(r.cells.len(), 3);
        assert_eq!(r.derived.len(), 1);
        assert_eq!(r.associations.len(), 1);
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.post_failures.len(), 2);
        assert_eq!(r.forbidden_states.len(), 1);
        assert_eq!(r.kill_tests.len(), 1);
        assert_eq!(r.walks, vec![1]);
        // Steps for the promote event.
        let promote = r.events.iter().find(|e| e.id == "promote").expect("promote present");
        assert_eq!(promote.steps, vec!["s1_update", "s2_changelog"]);
    }

    #[test]
    fn by_id_finds_top_level_block() {
        let f = parse_fixture();
        let r = by_id(&f, "rule_state").expect("rule_state present");
        assert_eq!(r.id, "rule_state");
        assert_eq!(r.kind, "cell");
    }

    #[test]
    fn by_id_finds_event_step() {
        let f = parse_fixture();
        let r = by_id(&f, "s1_update").expect("step present");
        assert_eq!(r.id, "promote.s1_update"); // first match wins
        assert_eq!(r.kind, "step");
    }

    #[test]
    fn by_id_returns_none_for_missing() {
        let f = parse_fixture();
        assert!(by_id(&f, "ghost").is_none());
    }

    #[test]
    fn refs_to_finds_cell_uses() {
        let f = parse_fixture();
        let r = refs_to(&f, "rule_state");
        // Expect at least: derives_from, over, mutates (event + step),
        // post_failure cells_after_pre_rollback, post_failure cells_after_rollback.
        assert!(
            r.references.len() >= 5,
            "expected ≥5 refs to rule_state, got {}: {:#?}",
            r.references.len(),
            r.references
        );
        // Check a representative path
        assert!(r.references.iter().any(|s| s.from.starts_with("event/promote") && s.field == "mutates"));
    }

    #[test]
    fn gaps_canonical_fixture_only_kill_test_status() {
        let f = parse_fixture();
        let g = gaps(&f);
        // Canonical fixture: 1 kill_test with status: declared (not executed_passing).
        assert_eq!(g.kill_tests_unresolved.len(), 1);
        assert_eq!(g.kill_tests_unresolved[0].status, "declared");
        // The single forbidden state HAS a kill_test pointing at it,
        // but kill_test status is not executed_passing → unkilled.
        assert_eq!(g.unkilled_forbidden_states.len(), 1);
        // Symmetric-failure rows are complete (we added pf_deprecate_changelog_fail).
        assert!(g.missing_post_failure.is_empty(), "expected no missing post_failure rows");
        // No unused cells.
        assert!(g.unused_cells.is_empty(), "expected no unused cells");
    }

    #[test]
    fn gaps_unused_cell_surfaces() {
        let pr = parse(
            r#"slice s {
                cell used { type: boolean mutable: true }
                cell unused { type: boolean mutable: true }
                event e {
                    mutates: [used]
                    step s1 { op: "x" fallible: true mutates: [used] }
                }
            }"#,
        );
        let g = gaps(&pr.file);
        assert_eq!(g.unused_cells.len(), 1);
        assert_eq!(g.unused_cells[0].id, "unused");
    }

    #[test]
    fn gaps_missing_post_failure_surfaces() {
        let pr = parse(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
            }"#,
        );
        let g = gaps(&pr.file);
        assert_eq!(g.missing_post_failure.len(), 1);
        assert_eq!(g.missing_post_failure[0].event, "e");
        assert_eq!(g.missing_post_failure[0].step, "s2");
    }
}
