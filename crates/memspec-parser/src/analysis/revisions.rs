//! Experimental revision manifests for external-tracker import trials.
//!
//! This module is intentionally gated behind `experimental-revisions`.
//! It proves that an existing `.memspec` can become revision 1 without
//! changing file syntax. It is not a released storage contract.

use schemars::JsonSchema;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File, SliceDecl};
use crate::span::Span;

pub const REVISION_MANIFEST_VERSION: &str = "memspec.revision_manifest/0.1-experimental";
pub const PATCH_FORMAT_VERSION: &str = "memspec.semantic_patch/0.1-experimental";

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

fn string_field(block: &BlockDecl, key: &str) -> Option<String> {
    block.items.iter().find_map(|item| match item {
        BlockItem::Field(field) if field.key.name == key => match &field.value {
            FieldValue::String { value, .. } => Some(value.clone()),
            _ => None,
        },
        _ => None,
    })
}

fn block_name_str(block: &BlockDecl) -> Option<&str> {
    match &block.name {
        Some(BlockName::Ident(i)) => Some(i.name.as_str()),
        _ => None,
    }
}

fn source_sha256(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
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
}
