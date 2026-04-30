//! Pass 1 — structural completeness.
//!
//! Walks each slot block and checks that the fields the v0 grammar
//! marks REQUIRED are actually present. Does not validate field
//! contents (those are coherence-pass concerns: type-domain validity,
//! ref resolution, etc.). Does not check post_failure coverage —
//! that's the symmetric-failure pass.
//!
//! Required-field table (per `docs/grammar-v0.md`):
//!
//! | slot              | required fields                                    |
//! |-------------------|----------------------------------------------------|
//! | `cell`            | `type`, `mutable`                                  |
//! | `derived`         | `derives_from`, `derivation`                       |
//! | `association`     | `invariant`, `over`, `enforced_by`                 |
//! | `event`           | `mutates`, ≥1 `step` block                         |
//! | `step` (in event) | `op`, `fallible`                                   |
//! | `post_failure`    | `event`, `step`, `outcome`, cells_after\*           |
//! | `forbidden_state` | `description`, `predicate` OR `cells`, `reachability`, `kill_test` |
//! | `kill_test`       | `forbidden`, `kind`, `assertion`                   |
//!
//! \*cells_after means either a `cells_after:` field, OR an anonymous
//!  block named `cells_after`, OR — when atomicity is transactional —
//!  the pair `cells_after_pre_rollback` + `cells_after_rollback`.
//!  Structural pass accepts ANY of these; the symmetric-failure pass
//!  enforces the rollback-pair requirement.

use crate::ast::{BlockDecl, BlockItem, File};
use crate::diagnostic::{Diagnostic, codes};
use crate::span::Span;

/// Append structural diagnostics to `out`.
pub fn run(file: &File, out: &mut Vec<Diagnostic>) {
    let Some(slice) = &file.slice else {
        return; // Parser already emitted E0105 for empty file.
    };

    if slice.items.is_empty() {
        out.push(
            Diagnostic::error(
                codes::E_STRUCT_EMPTY_SLICE,
                slice.span,
                format!("slice `{}` declares no slot blocks", slice.name.name),
            )
            .with_hint("a slice must declare at least one cell, event, or other slot"),
        );
        return;
    }

    for item in &slice.items {
        let BlockItem::Block(block) = item else { continue };
        match block.kind.name.as_str() {
            "cell" => check_cell(block, out),
            "derived" => check_derived(block, out),
            "association" => check_association(block, out),
            "event" => check_event(block, out),
            "post_failure" => check_post_failure(block, out),
            "forbidden_state" => check_forbidden_state(block, out),
            "kill_test" => check_kill_test(block, out),
            // `meta` and `walk` are slice metadata blocks — no required-field
            // enforcement at this layer beyond what the parser already gave.
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Per-slot checks
// ---------------------------------------------------------------------------

fn check_cell(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "cell");
    require_field(block, "type", out);
    require_field(block, "mutable", out);
}

fn check_derived(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "derived");
    require_field(block, "derives_from", out);
    require_field(block, "derivation", out);
}

fn check_association(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "association");
    require_field(block, "invariant", out);
    require_field(block, "over", out);
    require_field(block, "enforced_by", out);
}

fn check_event(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "event");
    require_field(block, "mutates", out);
    let step_count = count_blocks_named(block, "step");
    if step_count == 0 {
        out.push(
            Diagnostic::error(
                codes::E_STRUCT_EVENT_NO_STEPS,
                block.span,
                format!(
                    "event `{}` declares no `step` blocks",
                    block_name_or(block, "<unnamed>"),
                ),
            )
            .with_hint("an event must contain at least one `step { ... }` block"),
        );
    }
    // Each step needs op + fallible
    for item in &block.items {
        if let BlockItem::Block(b) = item {
            if b.kind.name == "step" {
                require_named(b, out, "step");
                require_field(b, "op", out);
                require_field(b, "fallible", out);
            }
        }
    }
}

fn check_post_failure(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "post_failure");
    require_field(block, "event", out);
    require_field(block, "step", out);
    require_field(block, "outcome", out);
    // cells_after may appear as a field OR as an anonymous block under any of
    // these names. Structural pass accepts any; symmetric-failure pass refines.
    let has_cells_after = has_field(block, "cells_after")
        || has_block(block, "cells_after")
        || has_block(block, "cells_after_pre_rollback")
        || has_block(block, "cells_after_rollback");
    if !has_cells_after {
        out.push(
            Diagnostic::error(
                codes::E_STRUCT_MISSING_FIELD,
                block.span,
                format!(
                    "post_failure `{}` is missing a `cells_after` declaration",
                    block_name_or(block, "<unnamed>"),
                ),
            )
            .with_hint(
                "add `cells_after { ... }` (or `cells_after_pre_rollback { ... }` + `cells_after_rollback { ... }` for transactional events)",
            ),
        );
    }
}

fn check_forbidden_state(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "forbidden_state");
    require_field(block, "description", out);
    // Body: either `predicate:` OR a `cells:` field/block.
    let has_predicate = has_field(block, "predicate");
    let has_cells = has_field(block, "cells") || has_block(block, "cells");
    if !has_predicate && !has_cells {
        out.push(
            Diagnostic::error(
                codes::E_STRUCT_FORBIDDEN_STATE_BODY,
                block.span,
                format!(
                    "forbidden_state `{}` is missing a `predicate:` or `cells:` body",
                    block_name_or(block, "<unnamed>"),
                ),
            )
            .with_hint("declare exactly one of `predicate: \"...\"` or `cells: { id: value, ... }`"),
        );
    }
    require_field(block, "reachability", out);
    require_field(block, "kill_test", out);
}

fn check_kill_test(block: &BlockDecl, out: &mut Vec<Diagnostic>) {
    require_named(block, out, "kill_test");
    require_field(block, "forbidden", out);
    require_field(block, "kind", out);
    require_field(block, "assertion", out);
    // `status: declared` is the default. If a `status:` field is missing,
    // emit an info-level note so the agent knows the default is in play.
    if !has_field(block, "status") {
        out.push(Diagnostic::info(
            codes::I_STRUCT_KILL_TEST_TODO,
            block.span,
            format!(
                "kill_test `{}` has no explicit `status:`; defaulting to `declared`",
                block_name_or(block, "<unnamed>"),
            ),
        ));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_named(block: &BlockDecl, out: &mut Vec<Diagnostic>, slot_kind: &str) {
    if block.name.is_none() {
        out.push(
            Diagnostic::error(
                codes::E_STRUCT_MISSING_FIELD,
                block.span,
                format!("`{slot_kind}` block requires an identifier name"),
            )
            .with_hint(format!("write `{slot_kind} <id> {{ ... }}`")),
        );
    }
}

fn require_field(block: &BlockDecl, key: &str, out: &mut Vec<Diagnostic>) {
    if !has_field(block, key) {
        out.push(Diagnostic::error(
            codes::E_STRUCT_MISSING_FIELD,
            block.span,
            format!(
                "`{}` block missing required field `{key}:`",
                block.kind.name
            ),
        ));
    }
}

fn has_field(block: &BlockDecl, key: &str) -> bool {
    block.items.iter().any(|item| match item {
        BlockItem::Field(f) => f.key.name == key,
        _ => false,
    })
}

fn has_block(block: &BlockDecl, kind: &str) -> bool {
    block.items.iter().any(|item| match item {
        BlockItem::Block(b) => b.kind.name == kind,
        _ => false,
    })
}

fn count_blocks_named(block: &BlockDecl, kind: &str) -> usize {
    block
        .items
        .iter()
        .filter(|item| matches!(item, BlockItem::Block(b) if b.kind.name == kind))
        .count()
}

fn block_name_or<'b>(block: &'b BlockDecl, fallback: &'b str) -> &'b str {
    use crate::ast::BlockName;
    match &block.name {
        Some(BlockName::Ident(i)) => i.name.as_str(),
        Some(BlockName::Int { .. }) => "<numeric>",
        None => fallback,
    }
}

// Suppress unused-import warning when only Span helpers are needed elsewhere.
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
        let mut diags = pr.diagnostics;
        run(&pr.file, &mut diags);
        diags
    }

    #[test]
    fn empty_slice_emits_diagnostic() {
        let d = analyze_str("slice s { }");
        assert!(d.iter().any(|d| d.code == codes::E_STRUCT_EMPTY_SLICE));
    }

    #[test]
    fn cell_missing_type_and_mutable() {
        let d = analyze_str("slice s { cell c { } }");
        let missing: Vec<_> = d.iter().filter(|d| d.code == codes::E_STRUCT_MISSING_FIELD).collect();
        // type + mutable both missing
        assert_eq!(missing.len(), 2, "expected 2 missing-field diagnostics, got: {missing:#?}");
    }

    #[test]
    fn event_with_no_steps_emits_diagnostic() {
        let d = analyze_str(
            r#"slice s {
                event e {
                    mutates: [c]
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_STRUCT_EVENT_NO_STEPS));
    }

    #[test]
    fn forbidden_state_without_predicate_or_cells() {
        let d = analyze_str(
            r#"slice s {
                forbidden_state f {
                    description: "bad"
                    reachability: currently_reachable
                    kill_test: kt
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::E_STRUCT_FORBIDDEN_STATE_BODY));
    }

    #[test]
    fn forbidden_state_with_predicate_passes() {
        let d = analyze_str(
            r#"slice s {
                forbidden_state f {
                    description: "bad"
                    predicate: "a == b"
                    reachability: currently_reachable
                    kill_test: kt
                }
            }"#,
        );
        assert!(!d.iter().any(|d| d.code == codes::E_STRUCT_FORBIDDEN_STATE_BODY));
    }

    #[test]
    fn kill_test_with_no_status_emits_info() {
        let d = analyze_str(
            r#"slice s {
                kill_test kt {
                    forbidden: f
                    kind: structural
                    assertion: "x"
                }
            }"#,
        );
        assert!(d.iter().any(|d| d.code == codes::I_STRUCT_KILL_TEST_TODO));
    }

    #[test]
    fn canonical_fixture_passes_structural_pass() {
        let src = include_str!("../../tests/fixtures/rule_lifecycle_minimal.memspec");
        let pr = parse(src);
        assert!(pr.diagnostics.is_empty(), "fixture should parse cleanly");
        let mut diags = Vec::new();
        run(&pr.file, &mut diags);
        // Filter out info-level notes (kill_test TODO defaults).
        let errors_or_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity != crate::diagnostic::Severity::Info)
            .collect();
        assert!(
            errors_or_warnings.is_empty(),
            "canonical fixture should pass structural pass; got: {errors_or_warnings:#?}"
        );
    }

    #[test]
    fn cell_without_name_emits_diagnostic() {
        let d = analyze_str("slice s { cell { type: boolean mutable: true } }");
        assert!(d.iter().any(|d| d.code == codes::E_STRUCT_MISSING_FIELD));
    }

    #[test]
    fn anonymous_cells_after_pre_rollback_satisfies_post_failure() {
        let d = analyze_str(
            r#"slice s {
                post_failure pf {
                    event: e
                    step: s1
                    outcome: "Err(...)"
                    cells_after_pre_rollback {
                        a: published
                    }
                    cells_after_rollback {
                        a: unchanged
                    }
                }
            }"#,
        );
        assert!(
            !d.iter().any(|d| d.code == codes::E_STRUCT_MISSING_FIELD
                && d.message.contains("cells_after"))
        );
    }
}
