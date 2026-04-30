//! `memspec suggest` — propose the next missing slot/clause as a fill-in template.
//!
//! Replaces LSP completion. Deterministic: given a file, returns the
//! single highest-priority gap and a template that fills it. The writer
//! agent's iteration loop calls this in place of LLM-guessing what to
//! author next.
//!
//! Priority order (most-blocking first):
//! 1. Empty slice → suggest a cell template.
//! 2. Missing required field on existing slot → suggest the field.
//! 3. Forbidden state without predicate/cells body → suggest predicate.
//! 4. Forbidden state's `kill_test:` ref doesn't resolve → suggest a
//!    matching kill_test block.
//! 5. Kill_test's `forbidden:` ref doesn't resolve → suggest a matching
//!    forbidden_state block.
//! 6. Event with ≥2 fallible steps missing a post_failure row → suggest
//!    the post_failure block.
//! 7. Walk-clean.

use serde::Serialize;

use crate::analysis::{coherence, structural, symmetric_failure};
use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File};
use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::span::Span;

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SuggestReport {
    pub status: SuggestStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<Gap>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestStatus {
    /// Walk-complete (no analyzer errors of any kind).
    WalkComplete,
    /// At least one gap exists. `gap` is populated.
    Gap,
    /// File has parse errors; nothing to suggest until those are fixed.
    ParseErrors,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Gap {
    pub kind: GapKind,
    pub message: String,
    pub diagnostic_code: &'static str,
    pub span: Span,
    /// `.memspec` text the writer can paste in. Indented to match a
    /// typical slice body.
    pub template: String,
    /// Hint about where to insert the template (after which existing block,
    /// or "end of slice").
    pub insert_after: InsertHint,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// Slice is empty — needs at least one slot declaration.
    EmptySlice,
    /// An existing block is missing a required field.
    MissingField,
    /// A forbidden_state has no `predicate:` or `cells:` body.
    ForbiddenStateBody,
    /// A kill_test ID is referenced but no matching block exists.
    MissingKillTest,
    /// A forbidden_state ID is referenced by a kill_test but no matching block exists.
    MissingForbiddenState,
    /// An event has fallible steps that need a post_failure row.
    MissingPostFailure,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InsertHint {
    AfterBlock { id: String },
    EndOfSlice,
    InsideBlock { id: String },
}

pub fn suggest(file: &File) -> SuggestReport {
    // Run all three analyzer passes to get the full gap picture.
    let mut diagnostics = Vec::new();
    structural::run(file, &mut diagnostics);
    coherence::run(file, &mut diagnostics);
    symmetric_failure::run(file, &mut diagnostics);

    // Triage by priority.
    if let Some(gap) = next_gap(file, &diagnostics) {
        SuggestReport { status: SuggestStatus::Gap, gap: Some(gap) }
    } else if has_errors(&diagnostics) {
        // Errors of a kind we don't have a template for yet — fall back to
        // surfacing the first error without a template.
        let d = diagnostics.iter().find(|d| d.severity == Severity::Error).unwrap();
        SuggestReport {
            status: SuggestStatus::Gap,
            gap: Some(Gap {
                kind: GapKind::MissingField,
                message: format!("{} (no template available — fix manually)", d.message),
                diagnostic_code: d.code,
                span: d.span,
                template: String::new(),
                insert_after: InsertHint::EndOfSlice,
            }),
        }
    } else {
        SuggestReport { status: SuggestStatus::WalkComplete, gap: None }
    }
}

/// Wrapper that runs parsing too — convenience for callers without an AST.
pub fn suggest_from_source(source: &str) -> SuggestReport {
    let pr = crate::parser::parse(source);
    if pr.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return SuggestReport { status: SuggestStatus::ParseErrors, gap: None };
    }
    suggest(&pr.file)
}

fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

fn next_gap(file: &File, diagnostics: &[Diagnostic]) -> Option<Gap> {
    let slice = file.slice.as_ref()?;

    // Priority 1: empty slice.
    if let Some(d) = diag_with_code(diagnostics, codes::E_STRUCT_EMPTY_SLICE) {
        return Some(Gap {
            kind: GapKind::EmptySlice,
            message: d.message.clone(),
            diagnostic_code: d.code,
            span: d.span,
            template: cell_template("rule_state", "boolean"),
            insert_after: InsertHint::EndOfSlice,
        });
    }

    // Priority 2: forbidden_state missing body.
    if let Some(d) = diag_with_code(diagnostics, codes::E_STRUCT_FORBIDDEN_STATE_BODY) {
        let id = id_from_message(&d.message).unwrap_or_else(|| "<id>".to_owned());
        return Some(Gap {
            kind: GapKind::ForbiddenStateBody,
            message: d.message.clone(),
            diagnostic_code: d.code,
            span: d.span,
            template: format!(
                "  // add to forbidden_state `{id}`:\n  predicate: \"<expression over your declared cells>\"\n"
            ),
            insert_after: InsertHint::InsideBlock { id },
        });
    }

    // Priority 3: any structural missing-field.
    if let Some(d) = diag_with_code(diagnostics, codes::E_STRUCT_MISSING_FIELD) {
        return Some(Gap {
            kind: GapKind::MissingField,
            message: d.message.clone(),
            diagnostic_code: d.code,
            span: d.span,
            template: missing_field_template(&d.message),
            insert_after: InsertHint::EndOfSlice,
        });
    }

    // Priority 4: kill_test reference doesn't resolve → suggest the kill_test block.
    if let Some(d) = diag_with_code(diagnostics, codes::E_COH_UNRESOLVED_KILL_TEST_REF) {
        let kt_id = extract_quoted_after(&d.message, "unknown kill_test").unwrap_or_else(|| "<id>".to_owned());
        let fs_id = extract_quoted_after(&d.message, "forbidden_state").unwrap_or_else(|| "<fs>".to_owned());
        return Some(Gap {
            kind: GapKind::MissingKillTest,
            message: d.message.clone(),
            diagnostic_code: d.code,
            span: d.span,
            template: kill_test_template(&kt_id, &fs_id),
            insert_after: last_block_id(slice, "kill_test")
                .map_or(InsertHint::EndOfSlice, |id| InsertHint::AfterBlock { id }),
        });
    }

    // Priority 5: forbidden_state reference doesn't resolve → suggest the FS block.
    if let Some(d) = diag_with_code(diagnostics, codes::E_COH_UNRESOLVED_FORBIDDEN_REF) {
        let fs_id = extract_quoted_after(&d.message, "unknown forbidden_state").unwrap_or_else(|| "<id>".to_owned());
        return Some(Gap {
            kind: GapKind::MissingForbiddenState,
            message: d.message.clone(),
            diagnostic_code: d.code,
            span: d.span,
            template: forbidden_state_template(&fs_id),
            insert_after: last_block_id(slice, "forbidden_state")
                .map_or(InsertHint::EndOfSlice, |id| InsertHint::AfterBlock { id }),
        });
    }

    // Priority 6: missing post_failure for symmetric-failure rule.
    if let Some(d) = diag_with_code(diagnostics, codes::E_SYMFAIL_MISSING_POST_FAILURE) {
        let (event, step) = parse_event_step(&d.message)
            .unwrap_or_else(|| ("<event>".to_owned(), "<step>".to_owned()));
        let event_atomicity = event_atomicity(slice, &event);
        return Some(Gap {
            kind: GapKind::MissingPostFailure,
            message: d.message.clone(),
            diagnostic_code: d.code,
            span: d.span,
            template: post_failure_template(&event, &step, &event_atomicity),
            insert_after: last_block_id(slice, "post_failure")
                .map_or(InsertHint::EndOfSlice, |id| InsertHint::AfterBlock { id }),
        });
    }

    None
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

fn cell_template(id: &str, ty: &str) -> String {
    format!(
        "  cell {id} {{\n    type: {ty}\n    mutable: true\n    // ref: \"path/to/file.rs:NN\"  // optional advisory\n  }}\n"
    )
}

fn missing_field_template(message: &str) -> String {
    // Best-effort: try to extract the missing field name from the message.
    // Diagnostic format is: "`<kind>` block missing required field `<key>:`"
    let key = extract_field_name(message);
    match key.as_deref() {
        Some("type") => "  type: <abstract-type>  // boolean | enum<...> | set<X> | append_only_relation<X> | ...\n".to_owned(),
        Some("mutable") => "  mutable: true\n".to_owned(),
        Some("derives_from") => "  derives_from: [<source-cell-id>, <source-cell-id>]\n".to_owned(),
        Some("derivation") => "  derivation: \"<expression over derives_from>\"\n".to_owned(),
        Some("invariant") => "  invariant: \"<expression that must always hold>\"\n".to_owned(),
        Some("over") => "  over: [<cell-id>, <cell-id>]\n".to_owned(),
        Some("enforced_by") => "  enforced_by: <schema | callback | method | event_handler(<event-id>) | construction_only | derivation | convention | none>\n".to_owned(),
        Some("mutates") => "  mutates: [<cell-id>]\n".to_owned(),
        Some("op") => "  op: \"<operation source-language identifier>\"\n".to_owned(),
        Some("fallible") => "  fallible: true  // or false\n".to_owned(),
        Some("event") => "  event: <event-id>\n".to_owned(),
        Some("step") => "  step: <step-id>  // must exist inside the named event\n".to_owned(),
        Some("outcome") => "  outcome: \"Err(<failure-mode>)\"\n".to_owned(),
        Some("description") => "  description: \"<plain-language statement of why this state is forbidden>\"\n".to_owned(),
        Some("reachability") => "  reachability: currently_reachable  // or runtime_unreachable | structurally_unreachable\n".to_owned(),
        Some("kill_test") => "  kill_test: <kill_test-id>  // or `TODO` to acknowledge the obligation without resolving it\n".to_owned(),
        Some("forbidden") => "  forbidden: <forbidden_state-id>\n".to_owned(),
        Some("kind") => "  kind: structural  // or behavioural | type_shape | property | model_check\n".to_owned(),
        Some("assertion") => "  assertion: \"<plain-language statement of what would prove this state unreachable>\"\n".to_owned(),
        Some(other) => format!("  {other}: <value>\n"),
        None => "  <field>: <value>\n".to_owned(),
    }
}

fn kill_test_template(kt_id: &str, fs_id: &str) -> String {
    format!(
        "  kill_test {kt_id} {{\n    forbidden: {fs_id}\n    kind: structural  // or behavioural | type_shape | property | model_check\n    assertion: \"<what would prove {fs_id} is unreachable>\"\n    status: declared\n  }}\n"
    )
}

fn forbidden_state_template(fs_id: &str) -> String {
    format!(
        "  forbidden_state {fs_id} {{\n    description: \"<plain-language statement of why this is forbidden>\"\n    predicate: \"<expression over your declared cells>\"\n    reachability: currently_reachable\n    kill_test: <kill_test-id-or-TODO>\n  }}\n"
    )
}

fn post_failure_template(event: &str, step: &str, atomicity: &str) -> String {
    let cells_block = if matches!(atomicity, "transactional" | "db_transaction") {
        "    cells_after_pre_rollback {\n      // <cell-id>: <value-after-step-committed-but-before-rollback>\n    }\n    cells_after_rollback {\n      // <cell-id>: <value-after-rollback-restores-pre-event-state>\n    }\n"
    } else {
        "    cells_after {\n      // <cell-id>: <value-after-failure>\n    }\n"
    };
    format!(
        "  post_failure pf_{event}_{step}_fail {{\n    event: {event}\n    step: {step}\n    outcome: \"Err(<failure-mode>)\"\n{cells_block}    result: rejected\n  }}\n"
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn diag_with_code<'a>(diags: &'a [Diagnostic], code: &str) -> Option<&'a Diagnostic> {
    diags.iter().find(|d| d.code == code)
}

/// Pull a backtick-quoted identifier out of a diagnostic message. Returns
/// the FIRST one found.
fn id_from_message(message: &str) -> Option<String> {
    let start = message.find('`')?;
    let rest = &message[start + 1..];
    let end = rest.find('`')?;
    Some(rest[..end].to_owned())
}

/// Pull the backtick-quoted token that follows a marker phrase.
fn extract_quoted_after(message: &str, marker: &str) -> Option<String> {
    let pos = message.find(marker)?;
    let rest = &message[pos + marker.len()..];
    let start = rest.find('`')?;
    let body = &rest[start + 1..];
    let end = body.find('`')?;
    Some(body[..end].to_owned())
}

/// Pull a backticked field name from a structural-missing-field message.
/// Format: ``...missing required field `<key>:` ``
fn extract_field_name(message: &str) -> Option<String> {
    let key_marker = "required field `";
    let pos = message.find(key_marker)?;
    let rest = &message[pos + key_marker.len()..];
    let end = rest.find('`')?;
    Some(rest[..end].trim_end_matches(':').to_owned())
}

/// Parse `event: X, step: Y` out of a symmetric-failure diagnostic.
fn parse_event_step(message: &str) -> Option<(String, String)> {
    let event_marker = "event: ";
    let step_marker = ", step: ";
    let ep = message.find(event_marker)?;
    let after_ep = &message[ep + event_marker.len()..];
    let sp = after_ep.find(step_marker)?;
    let event = after_ep[..sp].to_owned();
    let after_sp = &after_ep[sp + step_marker.len()..];
    let end = after_sp.find('`').or_else(|| after_sp.find(' ')).unwrap_or(after_sp.len());
    let step = after_sp[..end].to_owned();
    Some((event, step))
}

fn last_block_id(slice: &crate::ast::SliceDecl, kind: &str) -> Option<String> {
    slice
        .items
        .iter()
        .rev()
        .find_map(|item| match item {
            BlockItem::Block(b) if b.kind.name == kind => match &b.name {
                Some(BlockName::Ident(i)) => Some(i.name.clone()),
                _ => None,
            },
            _ => None,
        })
}

fn event_atomicity(slice: &crate::ast::SliceDecl, event_id: &str) -> String {
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            if b.kind.name == "event" {
                if let Some(BlockName::Ident(i)) = &b.name {
                    if i.name == event_id {
                        if let Some(FieldValue::Ident(a)) = field_value(b, "atomicity") {
                            return a.name.clone();
                        }
                    }
                }
            }
        }
    }
    "none".to_owned()
}

fn field_value<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a FieldValue> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Field(f) if f.key.name == key => Some(&f.value),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggest_str(src: &str) -> SuggestReport {
        suggest_from_source(src)
    }

    #[test]
    fn parse_errors_are_surfaced() {
        let r = suggest_str("slice broken { cell foo { type: ");
        assert!(matches!(r.status, SuggestStatus::ParseErrors));
        assert!(r.gap.is_none());
    }

    #[test]
    fn empty_slice_suggests_a_cell() {
        let r = suggest_str("slice s { }");
        assert!(matches!(r.status, SuggestStatus::Gap));
        let gap = r.gap.unwrap();
        assert!(matches!(gap.kind, GapKind::EmptySlice));
        assert!(gap.template.contains("cell"));
        assert!(gap.template.contains("type:"));
    }

    #[test]
    fn missing_field_suggests_field_template() {
        let r = suggest_str("slice s { cell foo { type: boolean } }");
        let gap = r.gap.expect("gap");
        assert!(matches!(gap.kind, GapKind::MissingField));
        assert!(gap.template.contains("mutable:"));
    }

    #[test]
    fn forbidden_state_without_body_suggests_predicate() {
        let r = suggest_str(
            r#"slice s {
                forbidden_state fs {
                    description: "x"
                    reachability: currently_reachable
                    kill_test: kt
                }
            }"#,
        );
        let gap = r.gap.expect("gap");
        assert!(matches!(gap.kind, GapKind::ForbiddenStateBody));
        assert!(gap.template.contains("predicate:"));
        assert!(matches!(gap.insert_after, InsertHint::InsideBlock { .. }));
    }

    #[test]
    fn unresolved_kill_test_suggests_kill_test_block() {
        let r = suggest_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                forbidden_state fs {
                    description: "x"
                    predicate: "c == true"
                    reachability: currently_reachable
                    kill_test: kt_missing
                }
            }"#,
        );
        let gap = r.gap.expect("gap");
        assert!(matches!(gap.kind, GapKind::MissingKillTest));
        assert!(gap.template.contains("kill_test kt_missing"));
        assert!(gap.template.contains("forbidden: fs"));
    }

    #[test]
    fn unresolved_forbidden_suggests_fs_block() {
        let r = suggest_str(
            r#"slice s {
                kill_test kt {
                    forbidden: fs_missing
                    kind: structural
                    assertion: "x"
                    status: declared
                }
            }"#,
        );
        let gap = r.gap.expect("gap");
        assert!(matches!(gap.kind, GapKind::MissingForbiddenState));
        assert!(gap.template.contains("forbidden_state fs_missing"));
    }

    #[test]
    fn missing_post_failure_suggests_pre_post_rollback_for_db_transaction() {
        let r = suggest_str(
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
        let gap = r.gap.expect("gap");
        assert!(matches!(gap.kind, GapKind::MissingPostFailure));
        assert!(gap.template.contains("event: e"));
        assert!(gap.template.contains("step: s2"));
        assert!(gap.template.contains("cells_after_pre_rollback"));
        assert!(gap.template.contains("cells_after_rollback"));
    }

    #[test]
    fn missing_post_failure_suggests_single_cells_after_for_non_transactional() {
        let r = suggest_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                event e {
                    mutates: [c]
                    step s1 { op: "x" fallible: true mutates: [c] }
                    step s2 { op: "y" fallible: true mutates: [c] }
                }
            }"#,
        );
        let gap = r.gap.expect("gap");
        assert!(matches!(gap.kind, GapKind::MissingPostFailure));
        assert!(gap.template.contains("cells_after"));
        assert!(!gap.template.contains("cells_after_pre_rollback"));
    }

    #[test]
    fn walk_complete_returns_no_gap() {
        let src = include_str!("../../tests/fixtures/rule_lifecycle_minimal.memspec");
        let r = suggest_from_source(src);
        assert!(matches!(r.status, SuggestStatus::WalkComplete));
        assert!(r.gap.is_none());
    }

    #[test]
    fn id_from_message_works() {
        let s = "forbidden_state `fs_archived_active` is missing a `predicate:` body";
        assert_eq!(id_from_message(s), Some("fs_archived_active".to_owned()));
    }

    #[test]
    fn extract_quoted_after_works() {
        let s = "forbidden_state `fs` references unknown kill_test `kt_missing`";
        assert_eq!(
            extract_quoted_after(s, "unknown kill_test"),
            Some("kt_missing".to_owned())
        );
    }

    #[test]
    fn parse_event_step_works() {
        let s = "missing post_failure for `event: e, step: s2`";
        assert_eq!(
            parse_event_step(s),
            Some(("e".to_owned(), "s2".to_owned()))
        );
    }
}

