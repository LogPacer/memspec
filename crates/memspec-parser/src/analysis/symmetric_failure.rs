//! Pass 3 — symmetric-failure (the founding-incident discipline).
//!
//! For every event E with steps `[s1, ..., sN]` where ≥2 of the `si` are
//! `fallible: true`, AND for every fallible step `si` (i > 1) where ≥1
//! prior step is `fallible: true` OR has a non-empty `mutates:` list:
//! a `post_failure` row with `event: E, step: si` MUST exist. This is the
//! discipline that catches the partial-success window class — the failure
//! mode the original Allium spec missed for the mTLS reload.
//!
//! Additionally, when `event.atomicity ∈ {transactional, db_transaction}`,
//! the post_failure row must distinguish `cells_after_pre_rollback` from
//! `cells_after_rollback`.

use std::collections::HashMap;

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File, Ident};
use crate::diagnostic::{Diagnostic, codes};
use crate::span::Span;

pub fn run(file: &File, out: &mut Vec<Diagnostic>) {
    let Some(slice) = &file.slice else { return };

    // Index post_failure rows by (event_id, step_id).
    let mut pf_index: HashMap<(&str, &str), &BlockDecl> = HashMap::new();
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            if b.kind.name != "post_failure" {
                continue;
            }
            let event = field_ident(b, "event").map(|i| i.name.as_str());
            let step = field_ident(b, "step").map(|i| i.name.as_str());
            if let (Some(e), Some(s)) = (event, step) {
                pf_index.insert((e, s), b);
            }
        }
    }

    for item in &slice.items {
        let BlockItem::Block(event) = item else { continue };
        if event.kind.name != "event" {
            continue;
        }
        let Some(event_name) = block_name(event) else { continue };

        // Collect ordered steps with their relevant flags.
        let steps: Vec<(&Ident, &BlockDecl)> = event
            .items
            .iter()
            .filter_map(|it| match it {
                BlockItem::Block(b) if b.kind.name == "step" => block_name_ident(b).map(|n| (n, b)),
                _ => None,
            })
            .collect();

        let fallible_count = steps
            .iter()
            .filter(|(_, b)| field_is_true(b, "fallible"))
            .count();
        if fallible_count < 2 {
            continue; // Rule applies only when ≥2 steps are fallible.
        }

        let atomicity = field_ident(event, "atomicity")
            .map(|i| i.name.as_str())
            .unwrap_or("none");
        let needs_rollback_pair = matches!(atomicity, "db_transaction" | "transactional");

        for (i, (step_name, step_block)) in steps.iter().enumerate() {
            if i == 0 {
                continue;
            }
            if !field_is_true(step_block, "fallible") {
                continue;
            }
            // Any prior step fallible or mutating?
            let prior_makes_state_observable = steps[..i].iter().any(|(_, prior)| {
                field_is_true(prior, "fallible")
                    || matches!(field_value(prior, "mutates"),
                                Some(FieldValue::List { items, .. }) if !items.is_empty())
            });
            if !prior_makes_state_observable {
                continue;
            }

            let key = (event_name, step_name.name.as_str());
            match pf_index.get(&key) {
                None => {
                    out.push(
                        Diagnostic::error(
                            codes::E_SYMFAIL_MISSING_POST_FAILURE,
                            step_name.span,
                            format!(
                                "missing `post_failure` row for `event: {event_name}, step: {step}` — what is the state of all cells if `{step}` fails AFTER prior steps succeeded?",
                                step = step_name.name
                            ),
                        )
                        .with_hint(format!(
                            "add `post_failure pf_<id> {{ event: {event_name} step: {step} outcome: \"...\" cells_after_pre_rollback {{ ... }} cells_after_rollback {{ ... }} }}`",
                            step = step_name.name
                        )),
                    );
                }
                Some(pf) if needs_rollback_pair => {
                    let has_pre = has_field(pf, "cells_after_pre_rollback")
                        || has_block(pf, "cells_after_pre_rollback");
                    let has_post = has_field(pf, "cells_after_rollback")
                        || has_block(pf, "cells_after_rollback");
                    if !(has_pre && has_post) {
                        out.push(
                            Diagnostic::error(
                                codes::E_SYMFAIL_MISSING_ROLLBACK_PAIR,
                                pf.span,
                                format!(
                                    "post_failure for `{event_name}.{step}` (atomicity: {atomicity}) must declare BOTH `cells_after_pre_rollback` and `cells_after_rollback`",
                                    step = step_name.name
                                ),
                            )
                            .with_hint(
                                "transactional events expose a transient pre-rollback state that may itself be a forbidden state; both the transient state AND the rolled-back steady state must be modeled",
                            ),
                        );
                    }
                }
                Some(_) => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers (same shape as coherence.rs; kept local to keep the pass standalone)
// ---------------------------------------------------------------------------

fn block_name(block: &BlockDecl) -> Option<&str> {
    block_name_ident(block).map(|i| i.name.as_str())
}

fn block_name_ident(block: &BlockDecl) -> Option<&Ident> {
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

fn field_ident<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a Ident> {
    match field_value(block, key) {
        Some(FieldValue::Ident(i)) => Some(i),
        _ => None,
    }
}

fn field_is_true(block: &BlockDecl, key: &str) -> bool {
    matches!(field_value(block, key), Some(FieldValue::Bool { value: true, .. }))
}

fn has_field(block: &BlockDecl, key: &str) -> bool {
    block.items.iter().any(|item| matches!(item, BlockItem::Field(f) if f.key.name == key))
}

fn has_block(block: &BlockDecl, kind: &str) -> bool {
    block.items.iter().any(|item| matches!(item, BlockItem::Block(b) if b.kind.name == kind))
}

#[allow(dead_code)]
fn _span_anchor() -> Span {
    Span::DUMMY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn analyze_str(src: &str) -> Vec<Diagnostic> {
        let pr = parse(src);
        let mut diags = Vec::new();
        run(&pr.file, &mut diags);
        diags
    }

    #[test]
    fn no_check_for_event_with_only_one_fallible_step() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    atomicity: db_transaction
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: false mutates: [c] }
                }
            }"#,
        );
        assert!(d.iter().all(|d| d.code != codes::E_SYMFAIL_MISSING_POST_FAILURE));
    }

    #[test]
    fn missing_post_failure_for_second_fallible_step() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    atomicity: db_transaction
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
            }"#,
        );
        assert!(
            d.iter().any(|d| d.code == codes::E_SYMFAIL_MISSING_POST_FAILURE),
            "expected missing-post_failure diagnostic, got: {d:#?}"
        );
    }

    #[test]
    fn satisfied_with_post_failure_row_and_rollback_pair() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    atomicity: db_transaction
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
                post_failure pf {
                    event: e
                    step: s2
                    outcome: "Err"
                    cells_after_pre_rollback { c: true }
                    cells_after_rollback { c: false }
                }
            }"#,
        );
        assert!(d.is_empty(), "expected clean run, got: {d:#?}");
    }

    #[test]
    fn transactional_event_requires_rollback_pair() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    atomicity: db_transaction
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
                post_failure pf {
                    event: e
                    step: s2
                    outcome: "Err"
                    cells_after { c: true }
                }
            }"#,
        );
        assert!(
            d.iter().any(|d| d.code == codes::E_SYMFAIL_MISSING_ROLLBACK_PAIR),
            "expected missing-rollback-pair diagnostic, got: {d:#?}"
        );
    }

    #[test]
    fn non_transactional_event_accepts_single_cells_after() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    atomicity: none
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
                post_failure pf {
                    event: e
                    step: s2
                    outcome: "Err"
                    cells_after { c: true }
                }
            }"#,
        );
        assert!(d.iter().all(|d| d.code != codes::E_SYMFAIL_MISSING_ROLLBACK_PAIR));
    }

    #[test]
    fn first_fallible_step_does_not_require_post_failure() {
        let d = analyze_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
                post_failure pf {
                    event: e
                    step: s2
                    outcome: "Err"
                    cells_after { c: true }
                }
            }"#,
        );
        // s1 is the first fallible step — no symmetric-failure row needed.
        assert!(d.iter().all(|d| d.code != codes::E_SYMFAIL_MISSING_POST_FAILURE));
    }
}
