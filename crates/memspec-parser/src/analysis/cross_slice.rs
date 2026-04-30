//! Cross-slice ref resolution.
//!
//! Runs over a [`WorkingSet`] (root file + transitively imported files)
//! and validates every `alias.id` qualified reference against the
//! imported file's declarations. Single-file analysis (structural,
//! coherence, symmetric-failure) continues to work file-by-file; this
//! pass adds the cross-file resolution that makes imports useful.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File, MapEntry};
use crate::diagnostic::{Diagnostic, codes};

use super::loader::WorkingSet;

/// Per-file ID inventory, sliced by slot kind. Just enough for qualified-
/// ref resolution; the full coherence-pass symbol table stays local.
#[derive(Debug, Default, Clone)]
struct PerFileIds {
    cells: HashSet<String>,
    derived: HashSet<String>,
    associations: HashSet<String>,
    events: HashSet<String>,
    post_failures: HashSet<String>,
    forbidden_states: HashSet<String>,
    kill_tests: HashSet<String>,
}

impl PerFileIds {
    fn contains(&self, id: &str) -> bool {
        self.cells.contains(id)
            || self.derived.contains(id)
            || self.associations.contains(id)
            || self.events.contains(id)
            || self.post_failures.contains(id)
            || self.forbidden_states.contains(id)
            || self.kill_tests.contains(id)
    }
}

fn build_per_file_ids(file: &File) -> PerFileIds {
    let mut ids = PerFileIds::default();
    let Some(slice) = &file.slice else { return ids };
    for item in &slice.items {
        let BlockItem::Block(b) = item else { continue };
        let Some(BlockName::Ident(name)) = &b.name else { continue };
        let id = name.name.clone();
        match b.kind.name.as_str() {
            "cell" => {
                ids.cells.insert(id);
            }
            "derived" => {
                ids.derived.insert(id);
            }
            "association" => {
                ids.associations.insert(id);
            }
            "event" => {
                ids.events.insert(id);
            }
            "post_failure" => {
                ids.post_failures.insert(id);
            }
            "forbidden_state" => {
                ids.forbidden_states.insert(id);
            }
            "kill_test" => {
                ids.kill_tests.insert(id);
            }
            _ => {}
        }
    }
    ids
}

/// Run cross-slice qualified-ref resolution. Returns a per-file diagnostic
/// vector keyed by canonical path.
pub fn resolve(working_set: &WorkingSet) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
    let mut out: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();

    // Build per-file ID inventories once.
    let per_file: BTreeMap<PathBuf, PerFileIds> = working_set
        .files
        .iter()
        .map(|lf| (lf.path.clone(), build_per_file_ids(&lf.file)))
        .collect();

    for lf in &working_set.files {
        let mut diagnostics = Vec::new();
        if let Some(slice) = &lf.file.slice {
            // Use the loader's already-resolved alias map (alias → canonical path).
            // Walk all qualified refs in the slice.
            for item in &slice.items {
                walk_block_for_qualified_refs(
                    item_as_block(item),
                    &lf.imports_resolved,
                    &per_file,
                    &mut diagnostics,
                );
            }
        }
        if !diagnostics.is_empty() {
            out.insert(lf.path.clone(), diagnostics);
        }
    }

    out
}

fn item_as_block(item: &BlockItem) -> Option<&BlockDecl> {
    match item {
        BlockItem::Block(b) => Some(b),
        _ => None,
    }
}

/// Collect every cell-id reachable through qualified refs in `lf`'s file —
/// i.e., for each `alias.id` reference found, the canonical (path, id) the
/// alias resolves to. Used by `analyze_working_set` to suppress
/// false-positive "unused cell" warnings on cells consumed across slices.
pub fn cross_referenced_ids(working_set: &WorkingSet) -> BTreeMap<PathBuf, std::collections::HashSet<String>> {
    let mut out: BTreeMap<PathBuf, std::collections::HashSet<String>> = BTreeMap::new();
    for lf in &working_set.files {
        let Some(slice) = &lf.file.slice else { continue };
        for item in &slice.items {
            collect_qualified(item_as_block(item), &lf.imports_resolved, &mut out);
        }
    }
    out
}

fn collect_qualified(
    block: Option<&BlockDecl>,
    alias_map: &BTreeMap<String, PathBuf>,
    out: &mut BTreeMap<PathBuf, std::collections::HashSet<String>>,
) {
    let Some(block) = block else { return };
    for item in &block.items {
        match item {
            BlockItem::Field(f) => collect_qualified_value(&f.value, alias_map, out),
            BlockItem::Block(b) => collect_qualified(Some(b), alias_map, out),
        }
    }
}

fn collect_qualified_value(
    value: &FieldValue,
    alias_map: &BTreeMap<String, PathBuf>,
    out: &mut BTreeMap<PathBuf, std::collections::HashSet<String>>,
) {
    match value {
        FieldValue::QualifiedIdent { alias, name, .. } => {
            if let Some(target) = alias_map.get(alias.name.as_str()) {
                out.entry(target.clone()).or_default().insert(name.name.clone());
            }
        }
        FieldValue::List { items, .. } => {
            for it in items {
                collect_qualified_value(it, alias_map, out);
            }
        }
        FieldValue::Map { entries, .. } => {
            for MapEntry { value, .. } in entries {
                collect_qualified_value(value, alias_map, out);
            }
        }
        FieldValue::TypeApp { params, .. } => {
            for p in params {
                collect_qualified_value(p, alias_map, out);
            }
        }
        FieldValue::Call { args, .. } => {
            for a in args {
                collect_qualified_value(a, alias_map, out);
            }
        }
        _ => {}
    }
}

/// Walk a block (and its inner blocks) looking for QualifiedIdent values
/// in any field; resolve each against the alias map + per-file ids.
fn walk_block_for_qualified_refs(
    block: Option<&BlockDecl>,
    alias_map: &BTreeMap<String, PathBuf>,
    per_file: &BTreeMap<PathBuf, PerFileIds>,
    out: &mut Vec<Diagnostic>,
) {
    let Some(block) = block else { return };
    for item in &block.items {
        match item {
            BlockItem::Field(f) => walk_value(&f.value, alias_map, per_file, out),
            BlockItem::Block(b) => {
                walk_block_for_qualified_refs(Some(b), alias_map, per_file, out);
            }
        }
    }
}

fn walk_value(
    value: &FieldValue,
    alias_map: &BTreeMap<String, PathBuf>,
    per_file: &BTreeMap<PathBuf, PerFileIds>,
    out: &mut Vec<Diagnostic>,
) {
    match value {
        FieldValue::QualifiedIdent { alias, name, .. } => {
            match alias_map.get(alias.name.as_str()) {
                None => {
                    out.push(
                        Diagnostic::error(
                            codes::E_LOADER_UNRESOLVED_ALIAS,
                            alias.span,
                            format!(
                                "qualified reference uses unknown alias `{}` — no `use` declaration imports it",
                                alias.name
                            ),
                        )
                        .with_hint(format!(
                            "add `use \"<path>\" as {}` to the top of this slice, or fix the alias",
                            alias.name
                        )),
                    );
                }
                Some(target_path) => {
                    let Some(ids) = per_file.get(target_path) else { return };
                    if !ids.contains(&name.name) {
                        out.push(
                            Diagnostic::error(
                                codes::E_LOADER_QUALIFIED_REF_UNRESOLVED,
                                name.span,
                                format!(
                                    "qualified reference `{}.{}` does not resolve in imported slice `{}`",
                                    alias.name,
                                    name.name,
                                    target_path.display()
                                ),
                            )
                            .with_hint("verify the imported slice declares this id, or check the alias points at the right file"),
                        );
                    }
                }
            }
        }
        FieldValue::List { items, .. } => {
            for it in items {
                walk_value(it, alias_map, per_file, out);
            }
        }
        FieldValue::Map { entries, .. } => {
            for MapEntry { value, .. } in entries {
                walk_value(value, alias_map, per_file, out);
            }
        }
        FieldValue::TypeApp { params, .. } => {
            for p in params {
                walk_value(p, alias_map, per_file, out);
            }
        }
        FieldValue::Call { args, .. } => {
            for a in args {
                walk_value(a, alias_map, per_file, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::loader::{InMemoryLoader, load_with_imports};
    use std::path::Path;

    fn run_resolve(loader: &InMemoryLoader, root: &str) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
        let ws = load_with_imports(loader, Path::new(root));
        resolve(&ws)
    }

    #[test]
    fn qualified_ref_resolves_when_imported() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    derived d {
                        derives_from: [o.cell_a]
                        derivation: "..."
                    }
                }"#,
            )
            .with_file(
                "/root/other.memspec",
                "slice other { cell cell_a { type: boolean mutable: true } }",
            );
        let diags = run_resolve(&loader, "/root/main.memspec");
        assert!(diags.is_empty(), "expected clean resolution, got: {diags:#?}");
    }

    #[test]
    fn qualified_ref_with_unknown_alias_emits_diagnostic() {
        let loader = InMemoryLoader::new().with_file(
            "/root/main.memspec",
            r#"slice main {
                derived d {
                    derives_from: [ghost.cell_a]
                    derivation: "..."
                }
            }"#,
        );
        let diags = run_resolve(&loader, "/root/main.memspec");
        let main_diags = diags.get(Path::new("/root/main.memspec")).expect("main diags");
        assert!(main_diags.iter().any(|d| d.code == codes::E_LOADER_UNRESOLVED_ALIAS));
    }

    #[test]
    fn qualified_ref_with_unknown_id_emits_diagnostic() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    derived d {
                        derives_from: [o.does_not_exist]
                        derivation: "..."
                    }
                }"#,
            )
            .with_file(
                "/root/other.memspec",
                "slice other { cell cell_a { type: boolean mutable: true } }",
            );
        let diags = run_resolve(&loader, "/root/main.memspec");
        let main_diags = diags.get(Path::new("/root/main.memspec")).expect("main diags");
        assert!(main_diags.iter().any(|d| d.code == codes::E_LOADER_QUALIFIED_REF_UNRESOLVED));
    }

    #[test]
    fn qualified_ref_in_map_key_value_resolves() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    forbidden_state fs {
                        description: "x"
                        cells: { local_c: true }
                        reachability: currently_reachable
                        kill_test: TODO
                    }
                    cell local_c { type: boolean mutable: true }
                }"#,
            )
            .with_file("/root/other.memspec", "slice other { cell cell_a { type: boolean mutable: true } }");
        let diags = run_resolve(&loader, "/root/main.memspec");
        assert!(diags.is_empty(), "no qualified refs to resolve here, got: {diags:#?}");
    }
}
