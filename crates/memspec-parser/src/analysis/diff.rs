//! Per-walk diff — `memspec diff --from N --to M`.
//!
//! Reads inline provenance fields on each clause (`walk_added`,
//! `walk_changed`, `walk_killed`, `walk_superseded`) and reports what
//! happened in the half-open walk range `(from, to]`. Per-clause
//! provenance is the source of truth; if a clause omits `walk_added`,
//! the slice's top-level `walk:` value (or 1 if absent) is used as the
//! default.
//!
//! No file-history reconstruction. The .memspec file IS the history —
//! every walk's contributions live inline.

use serde::Serialize;

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File};
use crate::span::Span;

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DiffReport {
    pub slice: Option<String>,
    pub from: i64,
    pub to: i64,
    pub walks_in_range: Vec<WalkSummary>,
    pub added: Vec<DiffEntry>,
    pub changed: Vec<DiffEntry>,
    pub killed: Vec<DiffEntry>,
    pub superseded: Vec<DiffEntry>,
    pub steps_added: Vec<DiffEntry>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DiffEntry {
    pub kind: String,
    pub id: String,
    pub walk: i64,
    pub span: Span,
    /// For step entries — the parent event id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct WalkSummary {
    pub walk: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Inclusive-of-`to`, exclusive-of-`from` range check.
fn in_range(walk: i64, from: i64, to: i64) -> bool {
    walk > from && walk <= to
}

pub fn diff(file: &File, from: i64, to: i64) -> Result<DiffReport, String> {
    if from > to {
        return Err(format!("invalid range: from={from} > to={to}"));
    }
    let Some(slice) = &file.slice else {
        return Ok(DiffReport {
            slice: None,
            from,
            to,
            walks_in_range: Vec::new(),
            added: Vec::new(),
            changed: Vec::new(),
            killed: Vec::new(),
            superseded: Vec::new(),
            steps_added: Vec::new(),
        });
    };

    // Slice-level default walk — used when a clause omits walk_added.
    let default_walk = slice_default_walk(slice).unwrap_or(1);

    let mut report = DiffReport {
        slice: Some(slice.name.name.clone()),
        from,
        to,
        walks_in_range: Vec::new(),
        added: Vec::new(),
        changed: Vec::new(),
        killed: Vec::new(),
        superseded: Vec::new(),
        steps_added: Vec::new(),
    };

    for item in &slice.items {
        let BlockItem::Block(b) = item else { continue };
        match b.kind.name.as_str() {
            "walk" => {
                if let Some(BlockName::Int { value, .. }) = &b.name {
                    if in_range(*value, from, to) {
                        report.walks_in_range.push(WalkSummary {
                            walk: *value,
                            summary: string_field(b, "summary"),
                        });
                    }
                }
            }
            kind @ ("cell" | "derived" | "association" | "event"
                | "post_failure" | "forbidden_state" | "kill_test") => {
                let id = block_name_str(b).unwrap_or("<anon>").to_owned();
                let span = b.span;

                let added = clause_walk(b, "walk_added", default_walk);
                if in_range(added, from, to) {
                    report.added.push(DiffEntry {
                        kind: kind.to_owned(),
                        id: id.clone(),
                        walk: added,
                        span,
                        parent: None,
                    });
                }
                if let Some(changed) = explicit_walk(b, "walk_changed") {
                    if in_range(changed, from, to) {
                        report.changed.push(DiffEntry {
                            kind: kind.to_owned(),
                            id: id.clone(),
                            walk: changed,
                            span,
                            parent: None,
                        });
                    }
                }
                if let Some(killed) = explicit_walk(b, "walk_killed") {
                    if in_range(killed, from, to) {
                        report.killed.push(DiffEntry {
                            kind: kind.to_owned(),
                            id: id.clone(),
                            walk: killed,
                            span,
                            parent: None,
                        });
                    }
                }
                if let Some(superseded) = explicit_walk(b, "walk_superseded") {
                    if in_range(superseded, from, to) {
                        report.superseded.push(DiffEntry {
                            kind: kind.to_owned(),
                            id: id.clone(),
                            walk: superseded,
                            span,
                            parent: None,
                        });
                    }
                }

                // Steps inside events get their own provenance.
                if kind == "event" {
                    let event_id = id.clone();
                    for inner in &b.items {
                        if let BlockItem::Block(step) = inner {
                            if step.kind.name != "step" {
                                continue;
                            }
                            let sid = block_name_str(step).unwrap_or("<anon>").to_owned();
                            let s_added = clause_walk(step, "walk_added", default_walk);
                            if in_range(s_added, from, to) {
                                report.steps_added.push(DiffEntry {
                                    kind: "step".to_owned(),
                                    id: sid,
                                    walk: s_added,
                                    span: step.span,
                                    parent: Some(event_id.clone()),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn slice_default_walk(slice: &crate::ast::SliceDecl) -> Option<i64> {
    // Honor an explicit top-level `walk N { ... }` block — the *highest*
    // walk number declared. (If multiple walks are present, default new
    // clauses to the most recent.)
    let mut max = None;
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            if b.kind.name == "walk" {
                if let Some(BlockName::Int { value, .. }) = &b.name {
                    max = Some(max.map_or(*value, |m: i64| m.max(*value)));
                }
            }
        }
    }
    max
}

fn explicit_walk(block: &BlockDecl, key: &str) -> Option<i64> {
    match field_value(block, key) {
        Some(FieldValue::Int { value, .. }) => Some(*value),
        _ => None,
    }
}

fn clause_walk(block: &BlockDecl, key: &str, default: i64) -> i64 {
    explicit_walk(block, key).unwrap_or(default)
}

fn field_value<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a FieldValue> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Field(f) if f.key.name == key => Some(&f.value),
        _ => None,
    })
}

fn string_field(block: &BlockDecl, key: &str) -> Option<String> {
    match field_value(block, key) {
        Some(FieldValue::String { value, .. }) => Some(value.clone()),
        _ => None,
    }
}

fn block_name_str(block: &BlockDecl) -> Option<&str> {
    match &block.name {
        Some(BlockName::Ident(i)) => Some(i.name.as_str()),
        _ => None,
    }
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
    fn diff_from_0_to_1_lists_everything() {
        let f = parse_fixture();
        let r = diff(&f, 0, 1).expect("ok");
        // Canonical fixture: 3 cells + 1 derived + 1 association + 2 events
        // + 2 post_failures + 1 forbidden_state + 1 kill_test = 11 added at slot level.
        assert_eq!(r.added.len(), 11);
        // Walk 1 record is in range.
        assert_eq!(r.walks_in_range.len(), 1);
        assert_eq!(r.walks_in_range[0].walk, 1);
        // 4 steps (2 per event).
        assert_eq!(r.steps_added.len(), 4);
    }

    #[test]
    fn diff_from_1_to_1_is_empty() {
        let f = parse_fixture();
        let r = diff(&f, 1, 1).expect("ok");
        assert!(r.added.is_empty());
        assert!(r.changed.is_empty());
        assert!(r.killed.is_empty());
        assert!(r.steps_added.is_empty());
    }

    #[test]
    fn diff_from_1_to_2_only_returns_walk_2_clauses() {
        let pr = parse(
            r#"slice s {
                walk 1 { summary: "first" }
                walk 2 { summary: "second" added: [b] killed: [a] }
                cell a {
                    type: boolean
                    mutable: true
                    walk_added: 1
                    walk_killed: 2
                }
                cell b {
                    type: boolean
                    mutable: true
                    walk_added: 2
                }
            }"#,
        );
        let r = diff(&pr.file, 1, 2).expect("ok");
        assert_eq!(r.added.len(), 1);
        assert_eq!(r.added[0].id, "b");
        assert_eq!(r.killed.len(), 1);
        assert_eq!(r.killed[0].id, "a");
        // Walk 2 record is in range.
        assert_eq!(r.walks_in_range.len(), 1);
        assert_eq!(r.walks_in_range[0].walk, 2);
        assert_eq!(r.walks_in_range[0].summary.as_deref(), Some("second"));
    }

    #[test]
    fn diff_supersedence_tracked() {
        let pr = parse(
            r#"slice s {
                walk 1 { summary: "first" }
                walk 2 { summary: "second" }
                cell a {
                    type: boolean
                    mutable: true
                    walk_added: 1
                    walk_superseded: 2
                }
                cell b {
                    type: boolean
                    mutable: true
                    walk_added: 2
                }
            }"#,
        );
        let r = diff(&pr.file, 1, 2).expect("ok");
        assert_eq!(r.superseded.len(), 1);
        assert_eq!(r.superseded[0].id, "a");
    }

    #[test]
    fn diff_with_invalid_range_errors() {
        let f = parse_fixture();
        assert!(diff(&f, 5, 1).is_err());
    }

    #[test]
    fn diff_clause_inherits_slice_default_walk() {
        // No explicit walk_added on the cells — they should inherit walk 2.
        let pr = parse(
            r#"slice s {
                walk 1 { summary: "first" }
                walk 2 { summary: "second" }
                cell a { type: boolean mutable: true }
            }"#,
        );
        let r = diff(&pr.file, 1, 2).expect("ok");
        assert_eq!(r.added.len(), 1);
        assert_eq!(r.added[0].walk, 2);
    }

    #[test]
    fn diff_changed_field_tracked() {
        let pr = parse(
            r#"slice s {
                walk 1 { summary: "first" }
                walk 2 { summary: "second" }
                cell a {
                    type: boolean
                    mutable: true
                    walk_added: 1
                    walk_changed: 2
                }
            }"#,
        );
        let r = diff(&pr.file, 1, 2).expect("ok");
        assert_eq!(r.changed.len(), 1);
        assert_eq!(r.changed[0].id, "a");
        // Cell is added in walk 1, changed in walk 2; only "changed" is in range.
        assert_eq!(r.added.len(), 0);
    }
}
