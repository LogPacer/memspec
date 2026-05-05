//! Experimental inline revision support.
//!
//! The `experimental-revisions` feature remains opt-in, but it is no
//! longer debug-only. This module owns the single canonical source hash
//! helper plus the parser/analyzer support for inline `revisions { ... }`
//! blocks and the source-to-source synthesis logic used by the CLI.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ast::{
    BlockDecl, BlockItem, BlockName, Field, FieldValue, File, Ident, Import, MapEntry, SliceDecl,
};
use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::parser;
use crate::span::Span;

pub const REVISION_MANIFEST_VERSION: &str = "memspec.revision_manifest/0.1-experimental";
pub const PATCH_FORMAT_VERSION: &str = "memspec.semantic_patch/0.1-experimental";
const LONG_CHAIN_THRESHOLD: usize = 200;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GenesisRevisionReport {
    pub schema_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub slice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memspec_version: Option<String>,
    pub materialized_view: MaterializedViewSummary,
    pub revision: RevisionSummary,
    pub projection: ProjectionSummary,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RevisionAppendReport {
    pub schema_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub slice: Option<String>,
    pub appended: bool,
    pub no_op: bool,
    pub message: String,
    pub revisions: Vec<RevisionSummary>,
    pub projection: ProjectionSummary,
    pub file_size: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MaterializedViewSummary {
    pub source_hash: String,
    pub byte_len: usize,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RevisionSummary {
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    pub result_hash: String,
    pub patch_format_version: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub ops: Vec<SemanticPatchOp>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProjectionSummary {
    pub imports: usize,
    pub walks: usize,
    pub cells: usize,
    pub derived: usize,
    pub associations: usize,
    pub events: usize,
    pub steps: usize,
    pub post_failures: usize,
    pub forbidden_states: usize,
    pub kill_tests: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct BlockPathSegment {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SemanticPatchOp {
    GenesisFromMaterializedView {
        source_hash: String,
        byte_len: usize,
        line_count: usize,
    },
    AddSlice {
        id: String,
        span: Span,
    },
    AddImport {
        alias: String,
        path: String,
        span: Span,
    },
    AddWalk {
        walk: i64,
        span: Span,
    },
    AddDeclaration {
        kind: String,
        id: String,
        span: Span,
    },
    AddStep {
        event: String,
        id: String,
        span: Span,
    },
    AddBlock {
        kind: String,
        name: String,
        items: Vec<String>,
    },
    RemoveBlock {
        kind: String,
        name: String,
    },
    ModifyField {
        block_path: Vec<BlockPathSegment>,
        field_name: String,
        value: String,
    },
    RemoveField {
        block_path: Vec<BlockPathSegment>,
        field_name: String,
    },
    ReorderItems {
        block_path: Vec<BlockPathSegment>,
        new_order: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct InlineRevision {
    pub span: Span,
    pub summary: RevisionSummary,
}

#[derive(Debug, Default, Clone)]
pub struct InlineRevisionChain {
    pub revisions_block_span: Option<Span>,
    pub revisions: Vec<InlineRevision>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct RevisionSynthesis {
    pub new_source: String,
    pub report: RevisionAppendReport,
}

#[derive(Debug)]
pub struct RevisionSynthesisError {
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn run(file: &File, out: &mut Vec<Diagnostic>) {
    let mut chain = collect_inline_revisions(file);
    out.append(&mut chain.diagnostics);
    validate_inline_revision_chain(
        file,
        &chain.revisions,
        TerminalCheck::RequireCurrentProjection,
        out,
    );
}

pub fn build_genesis_revision(
    file: &File,
    source: &str,
    source_path: Option<String>,
    reason: String,
    author: Option<String>,
) -> GenesisRevisionReport {
    let source_hash = source_sha256(source);
    let materialized_view = MaterializedViewSummary {
        source_hash: source_hash.clone(),
        byte_len: source.len(),
        line_count: line_count(source),
    };

    let mut ops = vec![SemanticPatchOp::GenesisFromMaterializedView {
        source_hash: source_hash.clone(),
        byte_len: materialized_view.byte_len,
        line_count: materialized_view.line_count,
    }];

    let mut projection = ProjectionSummary::default();
    let mut memspec_version = None;

    if let Some(slice) = &file.slice {
        projection.imports = slice.imports.len();
        ops.push(SemanticPatchOp::AddSlice {
            id: slice.name.name.clone(),
            span: slice.name.span,
        });
        for import in &slice.imports {
            ops.push(SemanticPatchOp::AddImport {
                alias: import.alias.name.clone(),
                path: import.path.clone(),
                span: import.span,
            });
        }

        memspec_version = meta_string_field(slice, "memspec_version");
        collect_projection(slice, &mut projection, &mut ops);
    }

    GenesisRevisionReport {
        schema_version: REVISION_MANIFEST_VERSION.to_owned(),
        source_path,
        slice: file.slice.as_ref().map(|s| s.name.name.clone()),
        memspec_version,
        materialized_view,
        revision: RevisionSummary {
            revision_number: 1,
            base_revision: None,
            base_hash: None,
            result_hash: source_hash,
            patch_format_version: PATCH_FORMAT_VERSION.to_owned(),
            reason,
            author,
            ops,
        },
        projection,
    }
}

/// Append inline revisions to `source` and return the rewritten file bytes.
///
/// The caller is responsible for durable file I/O. Keeping this function
/// pure lets the CLI perform tempfile+rename+fsync while tests can exercise
/// the synthesis logic without touching disk.
pub fn synthesize_revision_source(
    source: &str,
    source_path: Option<String>,
    reason: String,
    author: Option<String>,
) -> Result<RevisionSynthesis, RevisionSynthesisError> {
    let parse_result = parser::parse(source);
    let mut diagnostics = parse_result.diagnostics;
    if diagnostics.iter().any(is_error) {
        return Err(RevisionSynthesisError {
            message: "cannot synthesize revision for a syntactically invalid .memspec file"
                .to_owned(),
            diagnostics,
        });
    }

    let file = parse_result.file;
    let mut chain = collect_inline_revisions(&file);
    diagnostics.append(&mut chain.diagnostics);
    validate_inline_revision_chain(
        &file,
        &chain.revisions,
        TerminalCheck::AllowCurrentProjectionMismatch,
        &mut diagnostics,
    );
    diagnostics.retain(|diagnostic| diagnostic.code != codes::E_REV_TERMINAL_MISMATCH);
    if diagnostics.iter().any(is_error) {
        return Err(RevisionSynthesisError {
            message: "existing revision chain is invalid".to_owned(),
            diagnostics,
        });
    }

    let projection = canonical_source_projection(&file);
    let projection_hash = source_sha256(&projection);
    let mut projection_summary = ProjectionSummary::default();
    if let Some(slice) = &file.slice {
        projection_summary.imports = slice.imports.len();
        let mut sink = Vec::new();
        collect_projection(slice, &mut projection_summary, &mut sink);
    }

    if let Some(last) = chain.revisions.last() {
        if last.summary.result_hash == projection_hash {
            let report = RevisionAppendReport {
                schema_version: REVISION_MANIFEST_VERSION.to_owned(),
                source_path,
                slice: file.slice.as_ref().map(|s| s.name.name.clone()),
                appended: false,
                no_op: true,
                message: "no semantic change - no revision appended".to_owned(),
                revisions: chain.revisions.into_iter().map(|r| r.summary).collect(),
                projection: projection_summary,
                file_size: source.len(),
            };
            return Ok(RevisionSynthesis {
                new_source: source.to_owned(),
                report,
            });
        }
    }

    let reason = if reason.is_empty() {
        "automated edit via watcher".to_owned()
    } else {
        reason
    };
    let previous_projection = if chain.revisions.is_empty() {
        empty_projection_for_file(&file)
    } else {
        replay_revisions(&file, &chain.revisions).map_err(|diagnostic| RevisionSynthesisError {
            message: "existing revision chain failed replay validation".to_owned(),
            diagnostics: vec![diagnostic],
        })?
    };
    let ops = diff_projections(&previous_projection, &projection)?;

    if ops.is_empty() {
        let diagnostic = Diagnostic::error(
            codes::E_REV_DIFF_NULL_BUT_TEXT_DIFFERS,
            file.span,
            "structural diff produced no ops but the projection hash changed",
        )
        .with_hint("extend the v0 semantic patch vocabulary before appending this revision");
        return Err(RevisionSynthesisError {
            message: "semantic diff did not explain the changed source projection".to_owned(),
            diagnostics: vec![diagnostic],
        });
    }

    let mut appended = Vec::new();
    if chain.revisions.is_empty() {
        let genesis_source = empty_projection_for_file(&file);
        let genesis_hash = source_sha256(&genesis_source);
        appended.push(RevisionToRender {
            summary: RevisionSummary {
                revision_number: 1,
                base_revision: None,
                base_hash: None,
                result_hash: genesis_hash.clone(),
                patch_format_version: PATCH_FORMAT_VERSION.to_owned(),
                reason: "genesis baseline before first synthesized edit".to_owned(),
                author: author.clone(),
                ops: vec![SemanticPatchOp::GenesisFromMaterializedView {
                    source_hash: genesis_hash,
                    byte_len: genesis_source.len(),
                    line_count: line_count(&genesis_source),
                }],
            },
        });
        appended.push(RevisionToRender {
            summary: RevisionSummary {
                revision_number: 2,
                base_revision: Some(1),
                base_hash: Some(appended[0].summary.result_hash.clone()),
                result_hash: projection_hash,
                patch_format_version: PATCH_FORMAT_VERSION.to_owned(),
                reason,
                author,
                ops,
            },
        });
    } else {
        let last = chain
            .revisions
            .last()
            .expect("checked non-empty chain above");
        let revision_number = last.summary.revision_number + 1;
        appended.push(RevisionToRender {
            summary: RevisionSummary {
                revision_number,
                base_revision: Some(last.summary.revision_number),
                base_hash: Some(last.summary.result_hash.clone()),
                result_hash: projection_hash,
                patch_format_version: PATCH_FORMAT_VERSION.to_owned(),
                reason,
                author,
                ops,
            },
        });
    }

    let new_source =
        render_source_with_appended_revisions(source, &file, chain.revisions_block_span, &appended)
            .map_err(|diagnostic| RevisionSynthesisError {
                message: "failed to render the appended revisions block".to_owned(),
                diagnostics: vec![diagnostic],
            })?;

    let replay_parse = parser::parse(&new_source);
    let mut replay_diagnostics = replay_parse.diagnostics;
    if !replay_diagnostics.iter().any(is_error) {
        let mut inline = collect_inline_revisions(&replay_parse.file);
        replay_diagnostics.append(&mut inline.diagnostics);
        validate_inline_revision_chain(
            &replay_parse.file,
            &inline.revisions,
            TerminalCheck::RequireCurrentProjection,
            &mut replay_diagnostics,
        );
    }
    if replay_diagnostics.iter().any(is_error) {
        return Err(RevisionSynthesisError {
            message: "freshly rendered revision chain failed replay validation".to_owned(),
            diagnostics: replay_diagnostics,
        });
    }

    for revision in &appended {
        chain.revisions.push(InlineRevision {
            span: Span::DUMMY,
            summary: revision.summary.clone(),
        });
    }

    let report = RevisionAppendReport {
        schema_version: REVISION_MANIFEST_VERSION.to_owned(),
        source_path,
        slice: file.slice.as_ref().map(|s| s.name.name.clone()),
        appended: true,
        no_op: false,
        message: format!(
            "appended revision {}",
            appended
                .last()
                .map(|revision| revision.summary.revision_number)
                .unwrap_or(0)
        ),
        revisions: appended.into_iter().map(|r| r.summary).collect(),
        projection: projection_summary,
        file_size: new_source.len(),
    };

    Ok(RevisionSynthesis { new_source, report })
}

pub fn collect_inline_revisions(file: &File) -> InlineRevisionChain {
    let Some(slice) = &file.slice else {
        return InlineRevisionChain::default();
    };
    let revisions_blocks: Vec<&BlockDecl> = slice
        .items
        .iter()
        .filter_map(|item| match item {
            BlockItem::Block(block) if block.kind.name == "revisions" => Some(block),
            _ => None,
        })
        .collect();

    let Some(revisions_block) = revisions_blocks.first() else {
        return InlineRevisionChain::default();
    };

    let mut chain = InlineRevisionChain {
        revisions_block_span: Some(revisions_block.span),
        revisions: Vec::new(),
        diagnostics: Vec::new(),
    };

    if revisions_blocks.len() > 1 {
        chain.diagnostics.push(
            Diagnostic::error(
                codes::E_REV_REPLAY_FAILED,
                revisions_blocks[1].span,
                "slice declares more than one `revisions` block",
            )
            .with_hint("keep exactly one append-only revisions block per slice"),
        );
    }

    for item in &revisions_block.items {
        match item {
            BlockItem::Block(block) if block.kind.name == "revision" => {
                if let Some(revision) = parse_revision_entry(block, &mut chain.diagnostics) {
                    chain.revisions.push(revision);
                }
            }
            BlockItem::Block(block) => chain.diagnostics.push(
                Diagnostic::error(
                    codes::E_REV_REPLAY_FAILED,
                    block.span,
                    format!(
                        "`revisions` block may only contain `revision N` blocks, found `{}`",
                        block.kind.name
                    ),
                )
                .with_hint("write `revision 1 { ... }`, `revision 2 { ... }`, ..."),
            ),
            BlockItem::Field(field) => chain.diagnostics.push(
                Diagnostic::error(
                    codes::E_REV_REPLAY_FAILED,
                    field.span,
                    "`revisions` block may not contain fields",
                )
                .with_hint("put base_hash/result_hash/ops/reason inside each `revision N` block"),
            ),
        }
    }

    chain
}

pub fn canonical_source_projection(file: &File) -> String {
    let Some(slice) = &file.slice else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str("slice ");
    out.push_str(&slice.name.name);
    out.push_str(" {\n");
    for import in &slice.imports {
        out.push_str("  use ");
        out.push_str(&quote_string(&import.path));
        out.push_str(" as ");
        out.push_str(&import.alias.name);
        out.push('\n');
    }
    if !slice.imports.is_empty() && slice.items.iter().any(|item| !is_revisions_item(item)) {
        out.push('\n');
    }
    let mut first_item = true;
    for item in &slice.items {
        if is_revisions_item(item) {
            continue;
        }
        if !first_item {
            out.push('\n');
        }
        render_block_item(item, 1, &mut out);
        first_item = false;
    }
    out.push_str("}\n");
    out
}

pub fn source_sha256(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

impl Default for ProjectionSummary {
    fn default() -> Self {
        Self {
            imports: 0,
            walks: 0,
            cells: 0,
            derived: 0,
            associations: 0,
            events: 0,
            steps: 0,
            post_failures: 0,
            forbidden_states: 0,
            kill_tests: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TerminalCheck {
    RequireCurrentProjection,
    AllowCurrentProjectionMismatch,
}

#[derive(Debug, Clone)]
struct RevisionToRender {
    summary: RevisionSummary,
}

fn validate_inline_revision_chain(
    file: &File,
    revisions: &[InlineRevision],
    terminal_check: TerminalCheck,
    out: &mut Vec<Diagnostic>,
) {
    if revisions.is_empty() {
        return;
    }

    if revisions.len() > LONG_CHAIN_THRESHOLD {
        out.push(
            Diagnostic::info(
                codes::W_REV_LONG_CHAIN,
                revisions[LONG_CHAIN_THRESHOLD].span,
                format!(
                    "revisions block has {} entries; consider snapshot compaction",
                    revisions.len()
                ),
            )
            .with_hint("v0 replay stays correct, but long chains make parsing and diffs noisy"),
        );
    }

    for (idx, revision) in revisions.iter().enumerate() {
        let expected = u64::try_from(idx + 1).unwrap_or(u64::MAX);
        if revision.summary.revision_number != expected {
            out.push(
                Diagnostic::error(
                    codes::E_REV_NUMBER_GAP,
                    revision.span,
                    format!(
                        "revision block number {} is not contiguous; expected {expected}",
                        revision.summary.revision_number
                    ),
                )
                .with_hint("revision numbers must be 1, 2, 3, ... in source order"),
            );
        }

        for op in &revision.summary.ops {
            if reorder_targets_walks(op) {
                out.push(
                    Diagnostic::error(
                        codes::E_REV_REORDER_FORBIDDEN_ON_WALKS,
                        revision.span,
                        "ReorderItems may not target walk declarations",
                    )
                    .with_hint("walk declarations are chronological and append-only in v0"),
                );
            }
        }
    }

    if revisions
        .first()
        .and_then(|revision| revision.summary.base_hash.as_ref())
        .is_some()
    {
        let first = revisions.first().expect("checked non-empty revisions");
        out.push(
            Diagnostic::error(
                codes::E_REV_BASE_HASH_BREAK,
                first.span,
                "revision 1 must have `base_hash: null`",
            )
            .with_hint("the genesis revision is anchored without a prior base hash"),
        );
    }

    for pair in revisions.windows(2) {
        let previous = &pair[0];
        let next = &pair[1];
        if next.summary.base_hash.as_deref() != Some(previous.summary.result_hash.as_str()) {
            out.push(
                Diagnostic::error(
                    codes::E_REV_BASE_HASH_BREAK,
                    next.span,
                    format!(
                        "revision {} base_hash does not match revision {} result_hash",
                        next.summary.revision_number, previous.summary.revision_number
                    ),
                )
                .with_hint("each revision must name the exact result_hash it builds on"),
            );
        }
    }

    let replayed_projection = match replay_revisions(file, revisions) {
        Ok(projection) => Some(projection),
        Err(diagnostic) => {
            out.push(diagnostic);
            None
        }
    };

    if matches!(terminal_check, TerminalCheck::RequireCurrentProjection) {
        let projection = canonical_source_projection(file);
        let projection_hash = source_sha256(&projection);
        let last = revisions.last().expect("checked non-empty revisions");
        if last.summary.result_hash != projection_hash {
            out.push(
                Diagnostic::error(
                    codes::E_REV_TERMINAL_MISMATCH,
                    last.span,
                    format!(
                        "terminal revision hash {} does not match current source projection {}",
                        last.summary.result_hash, projection_hash
                    ),
                )
                .with_hint(
                    "run `memspec experimental synthesize-revision <file>` after semantic edits",
                ),
            );
        }
        if let Some(replayed_projection) = replayed_projection {
            let replayed_hash = source_sha256(&replayed_projection);
            if replayed_hash != projection_hash {
                out.push(
                    Diagnostic::error(
                        codes::E_REV_REPLAY_FAILED,
                        last.span,
                        format!(
                            "terminal replay hash {replayed_hash} does not match current source projection {projection_hash}"
                        ),
                    )
                    .with_hint(
                        "revision ops must replay to the same projection as the current source",
                    ),
                );
            }
        }
    }
}

fn replay_revisions(file: &File, revisions: &[InlineRevision]) -> Result<String, Diagnostic> {
    let empty_projection = empty_projection_for_file(file);
    let mut replay_file = parser::parse(&empty_projection).file;

    for revision in revisions {
        for op in &revision.summary.ops {
            apply_revision_op(&mut replay_file, op, revision.span)?;
        }
        let projection = canonical_source_projection(&replay_file);
        let replay_hash = source_sha256(&projection);
        if replay_hash != revision.summary.result_hash {
            return Err(replay_error(
                revision.span,
                format!(
                    "revision {} ops replay to {replay_hash}, not declared result_hash {}",
                    revision.summary.revision_number, revision.summary.result_hash
                ),
            )
            .with_hint("the semantic patch ops must explain the full revision hash transition"));
        }
    }

    Ok(canonical_source_projection(&replay_file))
}

fn apply_revision_op(file: &mut File, op: &SemanticPatchOp, span: Span) -> Result<(), Diagnostic> {
    match op {
        SemanticPatchOp::GenesisFromMaterializedView {
            source_hash,
            byte_len,
            line_count: expected_lines,
        } => {
            let projection = canonical_source_projection(file);
            let replay_hash = source_sha256(&projection);
            if &replay_hash != source_hash {
                return Err(replay_error(
                    span,
                    format!(
                        "genesis source_hash {source_hash} does not match replay cursor {replay_hash}"
                    ),
                ));
            }
            if projection.len() != *byte_len {
                return Err(replay_error(
                    span,
                    format!(
                        "genesis byte_len {byte_len} does not match replay cursor byte_len {}",
                        projection.len()
                    ),
                ));
            }
            let actual_lines = line_count(&projection);
            if actual_lines != *expected_lines {
                return Err(replay_error(
                    span,
                    format!(
                        "genesis line_count {expected_lines} does not match replay cursor line_count {actual_lines}"
                    ),
                ));
            }
            Ok(())
        }
        SemanticPatchOp::AddSlice { id, .. } => {
            let slice = replay_slice_mut(file, span)?;
            if slice.name.name != *id {
                return Err(replay_error(
                    span,
                    format!(
                        "add_slice id `{id}` does not match replay slice `{}`",
                        slice.name.name
                    ),
                ));
            }
            Ok(())
        }
        SemanticPatchOp::AddImport { alias, path, .. } => {
            let slice = replay_slice_mut(file, span)?;
            if let Some(existing) = slice
                .imports
                .iter()
                .find(|import| import.alias.name == *alias)
            {
                if existing.path == *path {
                    return Ok(());
                }
                return Err(replay_error(
                    span,
                    format!(
                        "add_import alias `{alias}` already points to `{}`",
                        existing.path
                    ),
                ));
            }
            slice.imports.push(Import {
                span: Span::DUMMY,
                path: path.clone(),
                path_span: Span::DUMMY,
                alias: Ident {
                    span: Span::DUMMY,
                    name: alias.clone(),
                },
            });
            Ok(())
        }
        SemanticPatchOp::AddWalk { walk, .. } => {
            let slice = replay_slice_mut(file, span)?;
            if top_level_block_index(slice, "walk", &walk.to_string()).is_some() {
                return Err(replay_error(
                    span,
                    format!("add_walk target `{walk}` already exists"),
                ));
            }
            slice.items.push(BlockItem::Block(empty_block(
                "walk",
                Some(BlockName::Int {
                    span: Span::DUMMY,
                    value: *walk,
                }),
            )));
            Ok(())
        }
        SemanticPatchOp::AddDeclaration { kind, id, .. } => {
            let slice = replay_slice_mut(file, span)?;
            if top_level_block_index(slice, kind, id).is_some() {
                return Err(replay_error(
                    span,
                    format!("add_declaration target `{kind} {id}` already exists"),
                ));
            }
            slice.items.push(BlockItem::Block(empty_block(
                kind,
                Some(BlockName::Ident(Ident {
                    span: Span::DUMMY,
                    name: id.clone(),
                })),
            )));
            Ok(())
        }
        SemanticPatchOp::AddStep { event, id, .. } => {
            let slice = replay_slice_mut(file, span)?;
            let Some(event_index) = top_level_block_index(slice, "event", event) else {
                return Err(replay_error(
                    span,
                    format!("add_step target event `{event}` does not exist"),
                ));
            };
            let BlockItem::Block(event_block) = &mut slice.items[event_index] else {
                unreachable!("top_level_block_index only returns blocks");
            };
            if child_block_index(event_block, "step", id).is_some() {
                return Err(replay_error(
                    span,
                    format!("add_step target `{event}.{id}` already exists"),
                ));
            }
            event_block.items.push(BlockItem::Block(empty_block(
                "step",
                Some(BlockName::Ident(Ident {
                    span: Span::DUMMY,
                    name: id.clone(),
                })),
            )));
            Ok(())
        }
        SemanticPatchOp::AddBlock { kind, name, items } => {
            let block = parse_replay_block(kind, name, items, span)?;
            let slice = replay_slice_mut(file, span)?;
            if top_level_block_index(slice, kind, name).is_some() {
                return Err(replay_error(
                    span,
                    format!("add_block target `{kind} {name}` already exists"),
                ));
            }
            slice.items.push(BlockItem::Block(block));
            Ok(())
        }
        SemanticPatchOp::RemoveBlock { kind, name } => {
            let slice = replay_slice_mut(file, span)?;
            let Some(index) = top_level_block_index(slice, kind, name) else {
                return Err(replay_error(
                    span,
                    format!("remove_block target `{kind} {name}` does not exist"),
                ));
            };
            slice.items.remove(index);
            Ok(())
        }
        SemanticPatchOp::ModifyField {
            block_path,
            field_name,
            value,
        } => {
            let value = parse_replay_field_value(field_name, value, span)?;
            let block = replay_block_by_path_mut(file, block_path, span)?;
            match field_index(block, field_name) {
                Some(index) => {
                    let BlockItem::Field(field) = &mut block.items[index] else {
                        unreachable!("field_index only returns fields");
                    };
                    field.value = value;
                }
                None => block.items.push(BlockItem::Field(Field {
                    span: Span::DUMMY,
                    key: Ident {
                        span: Span::DUMMY,
                        name: field_name.clone(),
                    },
                    value,
                })),
            }
            Ok(())
        }
        SemanticPatchOp::RemoveField {
            block_path,
            field_name,
        } => {
            let block = replay_block_by_path_mut(file, block_path, span)?;
            let Some(index) = field_index(block, field_name) else {
                return Err(replay_error(
                    span,
                    format!("remove_field target `{field_name}` does not exist"),
                ));
            };
            block.items.remove(index);
            Ok(())
        }
        SemanticPatchOp::ReorderItems {
            block_path,
            new_order,
        } => {
            if reorder_targets_walk_path(block_path) {
                return Err(Diagnostic::error(
                    codes::E_REV_REORDER_FORBIDDEN_ON_WALKS,
                    span,
                    "ReorderItems may not target walk declarations",
                ));
            }
            let block = replay_block_by_path_mut(file, block_path, span)?;
            reorder_child_blocks(block, new_order, span)
        }
    }
}

fn replay_slice_mut(file: &mut File, span: Span) -> Result<&mut SliceDecl, Diagnostic> {
    file.slice.as_mut().ok_or_else(|| {
        replay_error(
            span,
            "revision replay cursor has no slice declaration".to_owned(),
        )
    })
}

fn replay_block_by_path_mut<'a>(
    file: &'a mut File,
    path: &[BlockPathSegment],
    span: Span,
) -> Result<&'a mut BlockDecl, Diagnostic> {
    let slice = replay_slice_mut(file, span)?;
    replay_block_in_items_mut(&mut slice.items, path, span)
}

fn replay_block_in_items_mut<'a>(
    items: &'a mut [BlockItem],
    path: &[BlockPathSegment],
    span: Span,
) -> Result<&'a mut BlockDecl, Diagnostic> {
    let Some((segment, rest)) = path.split_first() else {
        return Err(replay_error(span, "revision op has an empty block_path"));
    };
    let Some(index) = items.iter().position(|item| match item {
        BlockItem::Block(block) => block_matches_segment(block, segment),
        BlockItem::Field(_) => false,
    }) else {
        return Err(replay_error(
            span,
            format!(
                "block_path segment `{} {}` does not exist",
                segment.kind, segment.name
            ),
        ));
    };
    let BlockItem::Block(block) = &mut items[index] else {
        unreachable!("position only matches blocks");
    };
    if rest.is_empty() {
        Ok(block)
    } else {
        replay_block_in_items_mut(&mut block.items, rest, span)
    }
}

fn parse_replay_block(
    kind: &str,
    name: &str,
    items: &[String],
    span: Span,
) -> Result<BlockDecl, Diagnostic> {
    let mut source = String::new();
    source.push_str("slice __replay {\n  ");
    source.push_str(kind);
    if !name.is_empty() {
        source.push(' ');
        source.push_str(name);
    }
    source.push_str(" {\n");
    for item in items {
        for line in item.lines() {
            source.push_str("    ");
            source.push_str(line);
            source.push('\n');
        }
    }
    source.push_str("  }\n}\n");

    let parsed = parser::parse(&source);
    if parsed.diagnostics.iter().any(is_error) {
        return Err(replay_error(
            span,
            format!("add_block target `{kind} {name}` could not be parsed for replay"),
        ));
    }
    parsed
        .file
        .slice
        .and_then(|slice| {
            slice.items.into_iter().find_map(|item| match item {
                BlockItem::Block(block) => Some(block),
                BlockItem::Field(_) => None,
            })
        })
        .ok_or_else(|| replay_error(span, format!("add_block target `{kind} {name}` is empty")))
}

fn parse_replay_field_value(
    field_name: &str,
    value: &str,
    span: Span,
) -> Result<FieldValue, Diagnostic> {
    let source =
        format!("slice __replay {{\n  cell __holder {{\n    {field_name}: {value}\n  }}\n}}\n");
    let parsed = parser::parse(&source);
    if parsed.diagnostics.iter().any(is_error) {
        return Err(replay_error(
            span,
            format!("field `{field_name}` value `{value}` could not be parsed for replay"),
        ));
    }
    let Some(slice) = parsed.file.slice else {
        return Err(replay_error(span, "field replay parser returned no slice"));
    };
    let Some(BlockItem::Block(block)) = slice.items.into_iter().next() else {
        return Err(replay_error(
            span,
            "field replay parser returned no holder block",
        ));
    };
    block
        .items
        .into_iter()
        .find_map(|item| match item {
            BlockItem::Field(field) if field.key.name == field_name => Some(field.value),
            _ => None,
        })
        .ok_or_else(|| replay_error(span, format!("field `{field_name}` value did not replay")))
}

fn reorder_child_blocks(
    block: &mut BlockDecl,
    new_order: &[String],
    span: Span,
) -> Result<(), Diagnostic> {
    let target_kind = if block.kind.name == "event" {
        Some("step".to_owned())
    } else {
        child_kind_for_order(block, new_order)
    };
    let Some(target_kind) = target_kind else {
        return Err(replay_error(
            span,
            "reorder_items did not match any child blocks",
        ));
    };

    let existing_order = child_order(block, &target_kind);
    let existing_set: BTreeSet<&String> = existing_order.iter().collect();
    let new_set: BTreeSet<&String> = new_order.iter().collect();
    if existing_set != new_set {
        return Err(replay_error(
            span,
            format!(
                "reorder_items new_order {:?} does not match existing `{target_kind}` children {:?}",
                new_order, existing_order
            ),
        ));
    }

    let mut by_name: BTreeMap<String, BlockDecl> = BTreeMap::new();
    for item in &block.items {
        if let BlockItem::Block(child) = item {
            if child.kind.name == target_kind {
                by_name.insert(block_name_display(child), child.clone());
            }
        }
    }
    let mut ordered = new_order
        .iter()
        .filter_map(|name| by_name.remove(name))
        .collect::<Vec<_>>()
        .into_iter();
    for item in &mut block.items {
        if matches!(item, BlockItem::Block(child) if child.kind.name == target_kind) {
            let Some(next) = ordered.next() else {
                return Err(replay_error(
                    span,
                    "reorder_items exhausted ordered children during replay",
                ));
            };
            *item = BlockItem::Block(next);
        }
    }
    Ok(())
}

fn child_kind_for_order(block: &BlockDecl, new_order: &[String]) -> Option<String> {
    let wanted: BTreeSet<&String> = new_order.iter().collect();
    let mut by_kind: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in &block.items {
        if let BlockItem::Block(child) = item {
            by_kind
                .entry(child.kind.name.clone())
                .or_default()
                .push(block_name_display(child));
        }
    }
    by_kind.into_iter().find_map(|(kind, names)| {
        let existing: BTreeSet<&String> = names.iter().collect();
        (existing == wanted).then_some(kind)
    })
}

fn empty_block(kind: &str, name: Option<BlockName>) -> BlockDecl {
    BlockDecl {
        span: Span::DUMMY,
        kind: Ident {
            span: Span::DUMMY,
            name: kind.to_owned(),
        },
        name,
        items: Vec::new(),
    }
}

fn top_level_block_index(slice: &SliceDecl, kind: &str, name: &str) -> Option<usize> {
    slice.items.iter().position(|item| match item {
        BlockItem::Block(block) => block.kind.name == kind && block_name_display(block) == name,
        BlockItem::Field(_) => false,
    })
}

fn child_block_index(block: &BlockDecl, kind: &str, name: &str) -> Option<usize> {
    block.items.iter().position(|item| match item {
        BlockItem::Block(child) => child.kind.name == kind && block_name_display(child) == name,
        BlockItem::Field(_) => false,
    })
}

fn field_index(block: &BlockDecl, field_name: &str) -> Option<usize> {
    block.items.iter().position(|item| match item {
        BlockItem::Field(field) => field.key.name == field_name,
        BlockItem::Block(_) => false,
    })
}

fn block_matches_segment(block: &BlockDecl, segment: &BlockPathSegment) -> bool {
    block.kind.name == segment.kind && block_name_display(block) == segment.name
}

fn reorder_targets_walk_path(path: &[BlockPathSegment]) -> bool {
    path.iter().any(|segment| {
        matches!(segment.kind.as_str(), "walk" | "walks")
            || matches!(segment.name.as_str(), "walk" | "walks")
    })
}

fn replay_error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::E_REV_REPLAY_FAILED, span, message)
}

fn parse_revision_entry(block: &BlockDecl, out: &mut Vec<Diagnostic>) -> Option<InlineRevision> {
    let revision_number = match &block.name {
        Some(BlockName::Int { value, .. }) if *value > 0 => u64::try_from(*value).ok()?,
        Some(BlockName::Int { value, .. }) => {
            out.push(Diagnostic::error(
                codes::E_REV_NUMBER_GAP,
                block.span,
                format!("revision number must be positive, got {value}"),
            ));
            return None;
        }
        _ => {
            out.push(
                Diagnostic::error(
                    codes::E_REV_REPLAY_FAILED,
                    block.span,
                    "`revision` block must use an integer name",
                )
                .with_hint("write `revision 1 { ... }`"),
            );
            return None;
        }
    };

    let base_hash = match required_field(block, "base_hash", out) {
        Some(value) => parse_optional_hash(value, out),
        None => None,
    };
    let result_hash =
        match required_field(block, "result_hash", out).and_then(|value| string_like(value, out)) {
            Some(hash) => hash,
            None => "sha256:<invalid>".to_owned(),
        };
    let reason = required_field(block, "reason", out)
        .and_then(|value| string_like(value, out))
        .unwrap_or_default();
    let author = field_value(block, "author").and_then(|value| string_like(value, out));
    let ops = match required_field(block, "ops", out) {
        Some(FieldValue::List { items, .. }) => parse_ops(items, out),
        Some(value) => {
            out.push(
                Diagnostic::error(
                    codes::E_REV_REPLAY_FAILED,
                    value.span(),
                    "`ops:` must be a list of op maps",
                )
                .with_hint("write `ops: [{ op: \"genesis_from_materialized_view\", ... }]`"),
            );
            Vec::new()
        }
        None => Vec::new(),
    };

    Some(InlineRevision {
        span: block.span,
        summary: RevisionSummary {
            revision_number,
            base_revision: revision_number.checked_sub(1).filter(|n| *n > 0),
            base_hash,
            result_hash,
            patch_format_version: PATCH_FORMAT_VERSION.to_owned(),
            reason,
            author,
            ops,
        },
    })
}

fn parse_optional_hash(value: &FieldValue, out: &mut Vec<Diagnostic>) -> Option<String> {
    match value {
        FieldValue::Ident(ident) if ident.name == "null" => None,
        FieldValue::String { value, .. } => Some(value.clone()),
        _ => {
            out.push(
                Diagnostic::error(
                    codes::E_REV_REPLAY_FAILED,
                    value.span(),
                    "`base_hash:` must be either null or a string",
                )
                .with_hint("write `base_hash: null` on revision 1 and a sha256 string afterwards"),
            );
            None
        }
    }
}

fn parse_ops(items: &[FieldValue], out: &mut Vec<Diagnostic>) -> Vec<SemanticPatchOp> {
    let mut ops = Vec::new();
    for item in items {
        match item {
            FieldValue::Map { entries, .. } => {
                if let Some(op) = parse_op(entries, item.span(), out) {
                    ops.push(op);
                }
            }
            _ => out.push(
                Diagnostic::error(
                    codes::E_REV_REPLAY_FAILED,
                    item.span(),
                    "revision op must be a map",
                )
                .with_hint("write `{ op: \"add_block\", ... }`"),
            ),
        }
    }
    ops
}

fn parse_op(
    entries: &[MapEntry],
    span: Span,
    out: &mut Vec<Diagnostic>,
) -> Option<SemanticPatchOp> {
    let tag = map_string(entries, "op");
    let Some(tag) = tag else {
        out.push(
            Diagnostic::error(
                codes::E_REV_UNKNOWN_OP,
                span,
                "revision op is missing `op:`",
            )
            .with_hint("every semantic patch op must be explicitly tagged"),
        );
        return None;
    };

    match tag.as_str() {
        "genesis_from_materialized_view" => Some(SemanticPatchOp::GenesisFromMaterializedView {
            source_hash: map_string(entries, "source_hash").unwrap_or_default(),
            byte_len: map_usize(entries, "byte_len").unwrap_or(0),
            line_count: map_usize(entries, "line_count").unwrap_or(0),
        }),
        "add_slice" => Some(SemanticPatchOp::AddSlice {
            id: map_string(entries, "id").unwrap_or_default(),
            span: Span::DUMMY,
        }),
        "add_import" => Some(SemanticPatchOp::AddImport {
            alias: map_string(entries, "alias").unwrap_or_default(),
            path: map_string(entries, "path").unwrap_or_default(),
            span: Span::DUMMY,
        }),
        "add_walk" => Some(SemanticPatchOp::AddWalk {
            walk: map_i64(entries, "walk").unwrap_or_default(),
            span: Span::DUMMY,
        }),
        "add_declaration" => Some(SemanticPatchOp::AddDeclaration {
            kind: map_string(entries, "kind").unwrap_or_default(),
            id: map_string(entries, "id").unwrap_or_default(),
            span: Span::DUMMY,
        }),
        "add_step" => Some(SemanticPatchOp::AddStep {
            event: map_string(entries, "event").unwrap_or_default(),
            id: map_string(entries, "id").unwrap_or_default(),
            span: Span::DUMMY,
        }),
        "add_block" => Some(SemanticPatchOp::AddBlock {
            kind: map_string(entries, "kind").unwrap_or_default(),
            name: map_string(entries, "name").unwrap_or_default(),
            items: map_string_list(entries, "items"),
        }),
        "remove_block" => Some(SemanticPatchOp::RemoveBlock {
            kind: map_string(entries, "kind").unwrap_or_default(),
            name: map_string(entries, "name").unwrap_or_default(),
        }),
        "modify_field" => Some(SemanticPatchOp::ModifyField {
            block_path: map_block_path(entries, "block_path"),
            field_name: map_string(entries, "field_name").unwrap_or_default(),
            value: map_string(entries, "value").unwrap_or_default(),
        }),
        "remove_field" => Some(SemanticPatchOp::RemoveField {
            block_path: map_block_path(entries, "block_path"),
            field_name: map_string(entries, "field_name").unwrap_or_default(),
        }),
        "reorder_items" => Some(SemanticPatchOp::ReorderItems {
            block_path: map_block_path(entries, "block_path"),
            new_order: map_string_list(entries, "new_order"),
        }),
        _ => {
            out.push(
                Diagnostic::error(
                    codes::E_REV_UNKNOWN_OP,
                    span,
                    format!("unknown revision op `{tag}`"),
                )
                .with_hint("bump the patch format before introducing new op tags"),
            );
            None
        }
    }
}

fn diff_projections(
    previous_projection: &str,
    current_projection: &str,
) -> Result<Vec<SemanticPatchOp>, RevisionSynthesisError> {
    if previous_projection == current_projection {
        return Ok(Vec::new());
    }

    let previous = parser::parse(previous_projection);
    let current = parser::parse(current_projection);
    let mut diagnostics = previous.diagnostics;
    diagnostics.extend(current.diagnostics);
    if diagnostics.iter().any(is_error) {
        return Err(RevisionSynthesisError {
            message: "cannot diff invalid source projection".to_owned(),
            diagnostics,
        });
    }

    let Some(previous_slice) = previous.file.slice.as_ref() else {
        return Err(diff_vocab_error(
            current.file.span,
            "previous projection has no slice",
        ));
    };
    let Some(current_slice) = current.file.slice.as_ref() else {
        return Err(diff_vocab_error(
            current.file.span,
            "current projection has no slice",
        ));
    };

    let mut ops = Vec::new();
    diff_imports(previous_slice, current_slice, &mut ops);
    diff_blocks(previous_slice, current_slice, &mut ops);

    if ops.is_empty() {
        return Err(diff_vocab_error(
            current.file.span,
            "source projection changed but no v0 semantic op represented the diff",
        ));
    }
    Ok(ops)
}

fn diff_imports(previous: &SliceDecl, current: &SliceDecl, ops: &mut Vec<SemanticPatchOp>) {
    let old_aliases: BTreeSet<&str> = previous
        .imports
        .iter()
        .map(|import| import.alias.name.as_str())
        .collect();
    for import in &current.imports {
        if !old_aliases.contains(import.alias.name.as_str()) {
            ops.push(SemanticPatchOp::AddImport {
                alias: import.alias.name.clone(),
                path: import.path.clone(),
                span: Span::DUMMY,
            });
        }
    }
}

fn diff_blocks(previous: &SliceDecl, current: &SliceDecl, ops: &mut Vec<SemanticPatchOp>) {
    let previous_blocks = top_level_blocks_by_key(previous);
    let current_blocks = top_level_blocks_by_key(current);

    for (key, block) in &current_blocks {
        if !previous_blocks.contains_key(key) {
            ops.push(SemanticPatchOp::AddBlock {
                kind: key.kind.clone(),
                name: key.name.clone(),
                items: block.items.iter().map(item_to_string).collect(),
            });
        }
    }

    for key in previous_blocks.keys() {
        if !current_blocks.contains_key(key) {
            ops.push(SemanticPatchOp::RemoveBlock {
                kind: key.kind.clone(),
                name: key.name.clone(),
            });
        }
    }

    for (key, before) in &previous_blocks {
        let Some(after) = current_blocks.get(key) else {
            continue;
        };
        diff_block_fields(
            &[BlockPathSegment {
                kind: key.kind.clone(),
                name: key.name.clone(),
            }],
            before,
            after,
            ops,
        );
        diff_nested_step_order(key, before, after, ops);
        diff_nested_step_fields(key, before, after, ops);
    }
}

fn diff_block_fields(
    path: &[BlockPathSegment],
    before: &BlockDecl,
    after: &BlockDecl,
    ops: &mut Vec<SemanticPatchOp>,
) {
    let before_fields = fields_by_name(before);
    let after_fields = fields_by_name(after);
    for (name, before_field) in &before_fields {
        match after_fields.get(name) {
            Some(after_field)
                if value_to_string(&before_field.value) != value_to_string(&after_field.value) =>
            {
                ops.push(SemanticPatchOp::ModifyField {
                    block_path: path.to_vec(),
                    field_name: (*name).to_owned(),
                    value: value_to_string(&after_field.value),
                });
            }
            Some(_) => {}
            None => ops.push(SemanticPatchOp::RemoveField {
                block_path: path.to_vec(),
                field_name: (*name).to_owned(),
            }),
        }
    }
    for (name, after_field) in &after_fields {
        if !before_fields.contains_key(name) {
            ops.push(SemanticPatchOp::ModifyField {
                block_path: path.to_vec(),
                field_name: (*name).to_owned(),
                value: value_to_string(&after_field.value),
            });
        }
    }
}

fn diff_nested_step_order(
    event_key: &BlockKey,
    before: &BlockDecl,
    after: &BlockDecl,
    ops: &mut Vec<SemanticPatchOp>,
) {
    if event_key.kind != "event" {
        return;
    }
    let before_order = child_order(before, "step");
    let after_order = child_order(after, "step");
    if before_order.len() <= 1 || before_order == after_order {
        return;
    }
    let before_set: BTreeSet<&String> = before_order.iter().collect();
    let after_set: BTreeSet<&String> = after_order.iter().collect();
    if before_set == after_set {
        ops.push(SemanticPatchOp::ReorderItems {
            block_path: vec![BlockPathSegment {
                kind: "event".to_owned(),
                name: event_key.name.clone(),
            }],
            new_order: after_order,
        });
    }
}

fn diff_nested_step_fields(
    event_key: &BlockKey,
    before: &BlockDecl,
    after: &BlockDecl,
    ops: &mut Vec<SemanticPatchOp>,
) {
    if event_key.kind != "event" {
        return;
    }
    let before_steps = child_blocks_by_key(before, "step");
    let after_steps = child_blocks_by_key(after, "step");
    for (step_name, before_step) in &before_steps {
        let Some(after_step) = after_steps.get(step_name) else {
            continue;
        };
        let path = vec![
            BlockPathSegment {
                kind: "event".to_owned(),
                name: event_key.name.clone(),
            },
            BlockPathSegment {
                kind: "step".to_owned(),
                name: (*step_name).to_owned(),
            },
        ];
        diff_block_fields(&path, before_step, after_step, ops);
    }
}

fn render_source_with_appended_revisions(
    source: &str,
    file: &File,
    revisions_span: Option<Span>,
    revisions: &[RevisionToRender],
) -> Result<String, Diagnostic> {
    let entries: String = revisions
        .iter()
        .map(|revision| render_revision_entry(revision, 2))
        .collect();

    if let Some(span) = revisions_span {
        let Some(insert_at) = close_brace_insert_at(source, span) else {
            return Err(Diagnostic::error(
                codes::E_REV_REPLAY_FAILED,
                span,
                "could not locate closing brace for existing revisions block",
            ));
        };
        let mut out = String::with_capacity(source.len() + entries.len() + 2);
        out.push_str(&source[..insert_at]);
        if !source[..insert_at].ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&entries);
        out.push_str(&source[insert_at..]);
        return Ok(out);
    }

    let Some(slice) = &file.slice else {
        return Err(Diagnostic::error(
            codes::E_REV_REPLAY_FAILED,
            file.span,
            "cannot append revisions without a slice declaration",
        ));
    };
    let Some(insert_at) = close_brace_insert_at(source, slice.span) else {
        return Err(Diagnostic::error(
            codes::E_REV_REPLAY_FAILED,
            slice.span,
            "could not locate closing brace for slice",
        ));
    };

    let mut block = String::new();
    block.push_str("\n  revisions {\n");
    block.push_str(&entries);
    block.push_str("  }\n");

    let mut out = String::with_capacity(source.len() + block.len());
    out.push_str(&source[..insert_at]);
    if !source[..insert_at].ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&block);
    out.push_str(&source[insert_at..]);
    Ok(out)
}

fn render_revision_entry(revision: &RevisionToRender, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let inner = "  ".repeat(depth + 1);
    let op_indent = "  ".repeat(depth + 2);
    let mut out = String::new();
    out.push_str(&indent);
    out.push_str("revision ");
    out.push_str(&revision.summary.revision_number.to_string());
    out.push_str(" {\n");
    out.push_str(&inner);
    out.push_str("base_hash: ");
    match &revision.summary.base_hash {
        Some(hash) => out.push_str(&quote_string(hash)),
        None => out.push_str("null"),
    }
    out.push('\n');
    out.push_str(&inner);
    out.push_str("result_hash: ");
    out.push_str(&quote_string(&revision.summary.result_hash));
    out.push('\n');
    out.push_str(&inner);
    out.push_str("ops: [\n");
    for op in &revision.summary.ops {
        out.push_str(&op_indent);
        out.push_str(&render_op_map(op));
        out.push_str(",\n");
    }
    out.push_str(&inner);
    out.push_str("]\n");
    out.push_str(&inner);
    out.push_str("reason: ");
    out.push_str(&quote_string(&revision.summary.reason));
    out.push('\n');
    if let Some(author) = &revision.summary.author {
        out.push_str(&inner);
        out.push_str("author: ");
        out.push_str(&quote_string(author));
        out.push('\n');
    }
    out.push_str(&indent);
    out.push_str("}\n");
    out
}

fn render_op_map(op: &SemanticPatchOp) -> String {
    match op {
        SemanticPatchOp::GenesisFromMaterializedView {
            source_hash,
            byte_len,
            line_count,
        } => format!(
            r#"{{ op: "genesis_from_materialized_view", source_hash: {}, byte_len: {byte_len}, line_count: {line_count} }}"#,
            quote_string(source_hash)
        ),
        SemanticPatchOp::AddSlice { id, .. } => {
            format!(r#"{{ op: "add_slice", id: {} }}"#, quote_string(id))
        }
        SemanticPatchOp::AddImport { alias, path, .. } => format!(
            r#"{{ op: "add_import", alias: {}, path: {} }}"#,
            quote_string(alias),
            quote_string(path)
        ),
        SemanticPatchOp::AddWalk { walk, .. } => {
            format!(r#"{{ op: "add_walk", walk: {walk} }}"#)
        }
        SemanticPatchOp::AddDeclaration { kind, id, .. } => format!(
            r#"{{ op: "add_declaration", kind: {}, id: {} }}"#,
            quote_string(kind),
            quote_string(id)
        ),
        SemanticPatchOp::AddStep { event, id, .. } => format!(
            r#"{{ op: "add_step", event: {}, id: {} }}"#,
            quote_string(event),
            quote_string(id)
        ),
        SemanticPatchOp::AddBlock { kind, name, items } => format!(
            r#"{{ op: "add_block", kind: {}, name: {}, items: [{}] }}"#,
            quote_string(kind),
            quote_string(name),
            items
                .iter()
                .map(|item| quote_string(item))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        SemanticPatchOp::RemoveBlock { kind, name } => format!(
            r#"{{ op: "remove_block", kind: {}, name: {} }}"#,
            quote_string(kind),
            quote_string(name)
        ),
        SemanticPatchOp::ModifyField {
            block_path,
            field_name,
            value,
        } => format!(
            r#"{{ op: "modify_field", block_path: [{}], field_name: {}, value: {} }}"#,
            render_block_path(block_path),
            quote_string(field_name),
            quote_string(value)
        ),
        SemanticPatchOp::RemoveField {
            block_path,
            field_name,
        } => format!(
            r#"{{ op: "remove_field", block_path: [{}], field_name: {} }}"#,
            render_block_path(block_path),
            quote_string(field_name)
        ),
        SemanticPatchOp::ReorderItems {
            block_path,
            new_order,
        } => format!(
            r#"{{ op: "reorder_items", block_path: [{}], new_order: [{}] }}"#,
            render_block_path(block_path),
            new_order
                .iter()
                .map(|item| quote_string(item))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn render_block_path(path: &[BlockPathSegment]) -> String {
    path.iter()
        .map(|segment| {
            format!(
                r#"{{ kind: {}, name: {} }}"#,
                quote_string(&segment.kind),
                quote_string(&segment.name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_projection(
    slice: &SliceDecl,
    projection: &mut ProjectionSummary,
    ops: &mut Vec<SemanticPatchOp>,
) {
    for item in &slice.items {
        let BlockItem::Block(block) = item else {
            continue;
        };
        match block.kind.name.as_str() {
            "walk" => {
                projection.walks += 1;
                if let Some(BlockName::Int { value, .. }) = &block.name {
                    ops.push(SemanticPatchOp::AddWalk {
                        walk: *value,
                        span: block.span,
                    });
                }
            }
            "cell" => push_decl(block, &mut projection.cells, ops),
            "derived" => push_decl(block, &mut projection.derived, ops),
            "association" => push_decl(block, &mut projection.associations, ops),
            "event" => {
                push_decl(block, &mut projection.events, ops);
                let event_id = block_name_str(block).unwrap_or("<anon>").to_owned();
                for inner in &block.items {
                    let BlockItem::Block(step) = inner else {
                        continue;
                    };
                    if step.kind.name != "step" {
                        continue;
                    }
                    projection.steps += 1;
                    if let Some(id) = block_name_str(step) {
                        ops.push(SemanticPatchOp::AddStep {
                            event: event_id.clone(),
                            id: id.to_owned(),
                            span: step.span,
                        });
                    }
                }
            }
            "post_failure" => push_decl(block, &mut projection.post_failures, ops),
            "forbidden_state" => push_decl(block, &mut projection.forbidden_states, ops),
            "kill_test" => push_decl(block, &mut projection.kill_tests, ops),
            _ => {}
        }
    }
}

fn push_decl(block: &BlockDecl, count: &mut usize, ops: &mut Vec<SemanticPatchOp>) {
    *count += 1;
    if let Some(id) = block_name_str(block) {
        ops.push(SemanticPatchOp::AddDeclaration {
            kind: block.kind.name.clone(),
            id: id.to_owned(),
            span: block.span,
        });
    }
}

fn meta_string_field(slice: &SliceDecl, key: &str) -> Option<String> {
    for item in &slice.items {
        let BlockItem::Block(block) = item else {
            continue;
        };
        if block.kind.name != "meta" {
            continue;
        }
        return string_field(block, key);
    }
    None
}

fn required_field<'a>(
    block: &'a BlockDecl,
    key: &str,
    out: &mut Vec<Diagnostic>,
) -> Option<&'a FieldValue> {
    let value = field_value(block, key);
    if value.is_none() {
        out.push(Diagnostic::error(
            codes::E_REV_REPLAY_FAILED,
            block.span,
            format!(
                "revision {} is missing required field `{key}:`",
                block_name_display(block)
            ),
        ));
    }
    value
}

fn field_value<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a FieldValue> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Field(field) if field.key.name == key => Some(&field.value),
        _ => None,
    })
}

fn string_field(block: &BlockDecl, key: &str) -> Option<String> {
    field_value(block, key).and_then(|value| match value {
        FieldValue::String { value, .. } => Some(value.clone()),
        _ => None,
    })
}

fn string_like(value: &FieldValue, out: &mut Vec<Diagnostic>) -> Option<String> {
    match value {
        FieldValue::String { value, .. } => Some(value.clone()),
        FieldValue::Ident(ident) => Some(ident.name.clone()),
        FieldValue::Int { value, .. } => Some(value.to_string()),
        _ => {
            out.push(Diagnostic::error(
                codes::E_REV_REPLAY_FAILED,
                value.span(),
                "expected string-like revision field value",
            ));
            None
        }
    }
}

fn map_string(entries: &[MapEntry], key: &str) -> Option<String> {
    entries
        .iter()
        .find(|entry| entry.key.name == key)
        .map(|entry| value_to_scalar_string(&entry.value))
}

fn map_i64(entries: &[MapEntry], key: &str) -> Option<i64> {
    entries
        .iter()
        .find(|entry| entry.key.name == key)
        .and_then(|entry| match &entry.value {
            FieldValue::Int { value, .. } => Some(*value),
            FieldValue::String { value, .. } => value.parse().ok(),
            _ => None,
        })
}

fn map_usize(entries: &[MapEntry], key: &str) -> Option<usize> {
    map_i64(entries, key).and_then(|value| usize::try_from(value).ok())
}

fn map_string_list(entries: &[MapEntry], key: &str) -> Vec<String> {
    entries
        .iter()
        .find(|entry| entry.key.name == key)
        .and_then(|entry| match &entry.value {
            FieldValue::List { items, .. } => {
                Some(items.iter().map(value_to_scalar_string).collect::<Vec<_>>())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn value_to_scalar_string(value: &FieldValue) -> String {
    match value {
        FieldValue::String { value, .. } => value.clone(),
        FieldValue::Ident(ident) => ident.name.clone(),
        FieldValue::Int { value, .. } => value.to_string(),
        FieldValue::Bool { value, .. } => value.to_string(),
        _ => value_to_string(value),
    }
}

fn map_block_path(entries: &[MapEntry], key: &str) -> Vec<BlockPathSegment> {
    let Some(entry) = entries.iter().find(|entry| entry.key.name == key) else {
        return Vec::new();
    };
    let FieldValue::List { items, .. } = &entry.value else {
        return Vec::new();
    };
    items.iter().filter_map(value_to_path_segment).collect()
}

fn value_to_path_segment(value: &FieldValue) -> Option<BlockPathSegment> {
    match value {
        FieldValue::Map { entries, .. } => Some(BlockPathSegment {
            kind: map_string(entries, "kind").unwrap_or_default(),
            name: map_string(entries, "name").unwrap_or_default(),
        }),
        FieldValue::String { value, .. } => {
            let (kind, name) = value.split_once(':').unwrap_or((value, ""));
            Some(BlockPathSegment {
                kind: kind.to_owned(),
                name: name.to_owned(),
            })
        }
        FieldValue::Ident(ident) => Some(BlockPathSegment {
            kind: ident.name.clone(),
            name: String::new(),
        }),
        _ => None,
    }
}

fn reorder_targets_walks(op: &SemanticPatchOp) -> bool {
    let SemanticPatchOp::ReorderItems { block_path, .. } = op else {
        return false;
    };
    block_path.iter().any(|segment| {
        matches!(segment.kind.as_str(), "walk" | "walks")
            || matches!(segment.name.as_str(), "walk" | "walks")
    })
}

fn close_brace_insert_at(source: &str, span: Span) -> Option<usize> {
    let end = span.end.min(source.len());
    source[..end].rfind('}')
}

fn empty_projection_for_file(file: &File) -> String {
    let name = file
        .slice
        .as_ref()
        .map(|slice| slice.name.name.as_str())
        .unwrap_or("unnamed");
    format!("slice {name} {{\n}}\n")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BlockKey {
    kind: String,
    name: String,
}

fn top_level_blocks_by_key(slice: &SliceDecl) -> BTreeMap<BlockKey, &BlockDecl> {
    slice
        .items
        .iter()
        .filter_map(|item| match item {
            BlockItem::Block(block) if block.kind.name != "revisions" => {
                Some((block_key(block), block))
            }
            _ => None,
        })
        .collect()
}

fn child_blocks_by_key<'a>(block: &'a BlockDecl, kind: &str) -> BTreeMap<String, &'a BlockDecl> {
    block
        .items
        .iter()
        .filter_map(|item| match item {
            BlockItem::Block(child) if child.kind.name == kind => {
                Some((block_name_display(child), child))
            }
            _ => None,
        })
        .collect()
}

fn fields_by_name(block: &BlockDecl) -> BTreeMap<&str, &Field> {
    block
        .items
        .iter()
        .filter_map(|item| match item {
            BlockItem::Field(field) => Some((field.key.name.as_str(), field)),
            _ => None,
        })
        .collect()
}

fn child_order(block: &BlockDecl, kind: &str) -> Vec<String> {
    block
        .items
        .iter()
        .filter_map(|item| match item {
            BlockItem::Block(child) if child.kind.name == kind => Some(block_name_display(child)),
            _ => None,
        })
        .collect()
}

fn block_key(block: &BlockDecl) -> BlockKey {
    BlockKey {
        kind: block.kind.name.clone(),
        name: block_name_display(block),
    }
}

fn block_name_display(block: &BlockDecl) -> String {
    match &block.name {
        Some(BlockName::Ident(ident)) => ident.name.clone(),
        Some(BlockName::Int { value, .. }) => value.to_string(),
        None => String::new(),
    }
}

fn block_name_str(block: &BlockDecl) -> Option<&str> {
    match &block.name {
        Some(BlockName::Ident(i)) => Some(i.name.as_str()),
        _ => None,
    }
}

fn is_revisions_item(item: &BlockItem) -> bool {
    matches!(item, BlockItem::Block(block) if block.kind.name == "revisions")
}

fn item_to_string(item: &BlockItem) -> String {
    let mut out = String::new();
    render_block_item(item, 0, &mut out);
    out.trim().to_owned()
}

fn render_block_item(item: &BlockItem, depth: usize, out: &mut String) {
    match item {
        BlockItem::Field(field) => render_field(field, depth, out),
        BlockItem::Block(block) => render_block(block, depth, out),
    }
}

fn render_block(block: &BlockDecl, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push_str(&block.kind.name);
    match &block.name {
        Some(BlockName::Ident(ident)) => {
            out.push(' ');
            out.push_str(&ident.name);
        }
        Some(BlockName::Int { value, .. }) => {
            out.push(' ');
            out.push_str(&value.to_string());
        }
        None => {}
    }
    out.push_str(" {\n");
    for item in &block.items {
        render_block_item(item, depth + 1, out);
    }
    out.push_str(&indent);
    out.push_str("}\n");
}

fn render_field(field: &Field, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push_str(&field.key.name);
    out.push_str(": ");
    out.push_str(&value_to_string(&field.value));
    out.push('\n');
}

fn value_to_string(value: &FieldValue) -> String {
    match value {
        FieldValue::Ident(ident) => ident.name.clone(),
        FieldValue::String { value, .. } => quote_string(value),
        FieldValue::Int { value, .. } => value.to_string(),
        FieldValue::Bool { value, .. } => value.to_string(),
        FieldValue::List { items, .. } => {
            format!(
                "[{}]",
                items
                    .iter()
                    .map(value_to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        FieldValue::Map { entries, .. } => {
            format!(
                "{{ {} }}",
                entries
                    .iter()
                    .map(|entry| format!("{}: {}", entry.key.name, value_to_string(&entry.value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        FieldValue::TypeApp {
            head,
            params,
            alternation,
            ..
        } => {
            let sep = if *alternation { " | " } else { ", " };
            format!(
                "{}<{}>",
                head.name,
                params
                    .iter()
                    .map(value_to_string)
                    .collect::<Vec<_>>()
                    .join(sep)
            )
        }
        FieldValue::Call { head, args, .. } => {
            format!(
                "{}({})",
                head.name,
                args.iter()
                    .map(value_to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        FieldValue::QualifiedIdent { alias, name, .. } => {
            format!("{}.{}", alias.name, name.name)
        }
    }
}

fn quote_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn is_error(diagnostic: &Diagnostic) -> bool {
    diagnostic.severity == Severity::Error
}

fn diff_vocab_error(span: Span, message: impl Into<String>) -> RevisionSynthesisError {
    RevisionSynthesisError {
        message: "semantic diff vocabulary hole".to_owned(),
        diagnostics: vec![
            Diagnostic::error(codes::E_REV_DIFF_VOCABULARY_HOLE, span, message)
                .with_hint("add a semantic patch op variant before appending this revision"),
        ],
    }
}

fn line_count(source: &str) -> usize {
    if source.is_empty() {
        return 0;
    }
    source.bytes().filter(|b| *b == b'\n').count() + usize::from(!source.ends_with('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn genesis_revision_builds_initial_import_without_rewriting_source() {
        let source = include_str!("../../tests/fixtures/rule_lifecycle_minimal.memspec");
        let file = parser::parse(source).file;
        let report = build_genesis_revision(
            &file,
            source,
            Some("rule_lifecycle_minimal.memspec".to_owned()),
            "initial import".to_owned(),
            Some("codex".to_owned()),
        );

        assert_eq!(report.schema_version, REVISION_MANIFEST_VERSION);
        assert_eq!(report.slice.as_deref(), Some("rule_lifecycle_minimal"));
        assert_eq!(report.memspec_version.as_deref(), Some("0.1"));
        assert_eq!(report.revision.revision_number, 1);
        assert_eq!(report.revision.base_revision, None);
        assert_eq!(report.revision.base_hash, None);
        assert_eq!(
            report.revision.result_hash,
            report.materialized_view.source_hash
        );
        assert!(report.revision.result_hash.starts_with("sha256:"));

        assert_eq!(report.projection.walks, 1);
        assert_eq!(report.projection.cells, 3);
        assert_eq!(report.projection.derived, 1);
        assert_eq!(report.projection.associations, 1);
        assert_eq!(report.projection.events, 2);
        assert_eq!(report.projection.steps, 4);
        assert_eq!(report.projection.post_failures, 2);
        assert_eq!(report.projection.forbidden_states, 1);
        assert_eq!(report.projection.kill_tests, 1);
        assert_eq!(report.revision.ops.len(), 18);
    }

    #[test]
    fn genesis_hash_changes_when_materialized_source_changes() {
        let source = include_str!("../../tests/fixtures/rule_lifecycle_minimal.memspec");
        let file = parser::parse(source).file;
        let first = build_genesis_revision(&file, source, None, "initial import".to_owned(), None);

        let changed_source = format!("{source}\n// local edit\n");
        let changed_file = parser::parse(&changed_source).file;
        let second = build_genesis_revision(
            &changed_file,
            &changed_source,
            None,
            "initial import".to_owned(),
            None,
        );

        assert_ne!(
            first.materialized_view.source_hash,
            second.materialized_view.source_hash
        );
    }

    #[test]
    fn canonical_projection_ignores_revisions_block_and_comments() {
        let source = r#"slice s {
  // comment
  cell c { type: boolean mutable: true }
  revisions {
    revision 1 {
      base_hash: null
      result_hash: "sha256:aaa"
      ops: []
      reason: "x"
    }
  }
}
"#;
        let file = parser::parse(source).file;
        let projection = canonical_source_projection(&file);
        assert!(!projection.contains("revisions"));
        assert!(!projection.contains("comment"));
        assert!(projection.contains("cell c"));
    }
}
