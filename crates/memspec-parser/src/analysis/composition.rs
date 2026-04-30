//! Phase B — composition analysis.
//!
//! Cross-slice warnings that operate over the [`WorkingSet`]:
//!
//! - **W0273 unused-import** — a `use "..." as <alias>` declaration with no
//!   `<alias>.<id>` qualified reference anywhere in the importing slice.
//!   Suggests removing the import.
//! - **W0274 duplicate-import-target** — two `use` declarations in the same
//!   slice that resolve to the same canonical path under different aliases.
//!   Aliases are still distinct identifiers; this is a smell, not a hard error.
//! - **W0275 imported-id-shadowed-by-local-id** — the importing slice
//!   declares a local cell/event/etc. with the same id as a declaration
//!   in an imported slice that the importing slice references via
//!   `<alias>.<that-id>`. Confusing; suggests renaming one side.
//!
//! All checks operate on the AST + the loader's `imports_resolved` map.
//! Pass uses Warning severity; never blocks walk-completion.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, MapEntry, SliceDecl};
use crate::diagnostic::{Diagnostic, codes};

use super::loader::WorkingSet;

pub fn run(ws: &WorkingSet) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
    let mut out: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();

    // Per-file id inventories — needed for the shadowed-id check.
    let per_file_ids: BTreeMap<PathBuf, FileIds> = ws
        .files
        .iter()
        .map(|lf| {
            let ids = lf
                .file
                .slice
                .as_ref()
                .map(collect_top_level_ids)
                .unwrap_or_default();
            (lf.path.clone(), ids)
        })
        .collect();

    for lf in &ws.files {
        let mut diagnostics = Vec::new();
        let Some(slice) = &lf.file.slice else { continue };

        // Collect every alias actually referenced in this slice.
        let used_aliases = collect_used_aliases(slice);

        // W0273: unused imports.
        for import in &slice.imports {
            if !used_aliases.contains(import.alias.name.as_str()) {
                diagnostics.push(
                    Diagnostic::warning(
                        codes::W_COMP_UNUSED_IMPORT,
                        import.span,
                        format!(
                            "import `{}` is declared but no `{}.<id>` qualified reference uses it",
                            import.alias.name, import.alias.name
                        ),
                    )
                    .with_hint("either reference an id from this import via `<alias>.<id>`, or remove the `use` declaration"),
                );
            }
        }

        // W0274: duplicate canonical-path targets across this slice's imports.
        let mut seen_targets: BTreeMap<&PathBuf, &str> = BTreeMap::new();
        for import in &slice.imports {
            let alias = import.alias.name.as_str();
            if let Some(target) = lf.imports_resolved.get(alias) {
                if let Some(prior_alias) = seen_targets.get(target) {
                    diagnostics.push(
                        Diagnostic::warning(
                            codes::W_COMP_DUPLICATE_IMPORT_TARGET,
                            import.span,
                            format!(
                                "imports `{}` and `{}` both resolve to `{}` — consider collapsing to one alias",
                                prior_alias, alias, target.display()
                            ),
                        )
                        .with_hint("multiple aliases for the same canonical file is rarely intentional; rename one or remove the duplicate `use`"),
                    );
                } else {
                    seen_targets.insert(target, alias);
                }
            }
        }

        // W0275: imported id shadowed by local id of the same name.
        // For every (alias, name) pair the slice references via alias.name,
        // check whether the slice ALSO declares a local id named `name`.
        let local_ids: HashSet<&str> = per_file_ids
            .get(&lf.path)
            .map(|ids| ids.all_ids.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let qualified_refs = collect_qualified_refs(slice);
        let mut already_warned: HashSet<(&str, &str)> = HashSet::new();
        for (alias, name, span) in &qualified_refs {
            if !local_ids.contains(name.as_str()) {
                continue;
            }
            // Only warn once per (alias, name) pair, even if used multiple times.
            if already_warned.contains(&(alias.as_str(), name.as_str())) {
                continue;
            }
            // Verify the name actually exists in the imported slice — otherwise
            // E0403 already covered it; no shadow warning needed.
            let Some(target) = lf.imports_resolved.get(alias.as_str()) else { continue };
            let Some(target_ids) = per_file_ids.get(target) else { continue };
            if !target_ids.all_ids.contains(name.as_str()) {
                continue;
            }
            diagnostics.push(
                Diagnostic::warning(
                    codes::W_COMP_IMPORTED_ID_SHADOWED,
                    *span,
                    format!(
                        "qualified reference `{alias}.{name}` shadows a local declaration also named `{name}` in this slice — readers may confuse the two"
                    ),
                )
                .with_hint("rename the local declaration to disambiguate, or use the qualified form everywhere if the local one is intentional"),
            );
            already_warned.insert((alias.as_str(), name.as_str()));
        }

        if !diagnostics.is_empty() {
            out.insert(lf.path.clone(), diagnostics);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FileIds {
    all_ids: HashSet<String>,
}

fn collect_top_level_ids(slice: &SliceDecl) -> FileIds {
    let mut ids = FileIds::default();
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            if let Some(BlockName::Ident(i)) = &b.name {
                ids.all_ids.insert(i.name.clone());
            }
        }
    }
    ids
}

fn collect_used_aliases(slice: &SliceDecl) -> HashSet<String> {
    let mut out = HashSet::new();
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            visit_block_for_aliases(b, &mut out);
        }
    }
    out
}

fn visit_block_for_aliases(block: &BlockDecl, out: &mut HashSet<String>) {
    for item in &block.items {
        match item {
            BlockItem::Field(f) => visit_value_for_aliases(&f.value, out),
            BlockItem::Block(b) => visit_block_for_aliases(b, out),
        }
    }
}

fn visit_value_for_aliases(value: &FieldValue, out: &mut HashSet<String>) {
    match value {
        FieldValue::QualifiedIdent { alias, .. } => {
            out.insert(alias.name.clone());
        }
        FieldValue::List { items, .. } => {
            for it in items {
                visit_value_for_aliases(it, out);
            }
        }
        FieldValue::Map { entries, .. } => {
            for MapEntry { value, .. } in entries {
                visit_value_for_aliases(value, out);
            }
        }
        FieldValue::TypeApp { params, .. } => {
            for p in params {
                visit_value_for_aliases(p, out);
            }
        }
        FieldValue::Call { args, .. } => {
            for a in args {
                visit_value_for_aliases(a, out);
            }
        }
        _ => {}
    }
}

fn collect_qualified_refs(slice: &SliceDecl) -> Vec<(String, String, crate::span::Span)> {
    let mut out = Vec::new();
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            visit_block_for_qrefs(b, &mut out);
        }
    }
    out
}

fn visit_block_for_qrefs(block: &BlockDecl, out: &mut Vec<(String, String, crate::span::Span)>) {
    for item in &block.items {
        match item {
            BlockItem::Field(f) => visit_value_for_qrefs(&f.value, out),
            BlockItem::Block(b) => visit_block_for_qrefs(b, out),
        }
    }
}

fn visit_value_for_qrefs(value: &FieldValue, out: &mut Vec<(String, String, crate::span::Span)>) {
    match value {
        FieldValue::QualifiedIdent { alias, name, span } => {
            out.push((alias.name.clone(), name.name.clone(), *span));
        }
        FieldValue::List { items, .. } => {
            for it in items {
                visit_value_for_qrefs(it, out);
            }
        }
        FieldValue::Map { entries, .. } => {
            for MapEntry { value, .. } in entries {
                visit_value_for_qrefs(value, out);
            }
        }
        FieldValue::TypeApp { params, .. } => {
            for p in params {
                visit_value_for_qrefs(p, out);
            }
        }
        FieldValue::Call { args, .. } => {
            for a in args {
                visit_value_for_qrefs(a, out);
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

    fn run_str(loader: &InMemoryLoader, root: &str) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
        let ws = load_with_imports(loader, Path::new(root));
        run(&ws)
    }

    #[test]
    fn unused_import_warns() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    cell c { type: boolean mutable: true }
                }"#,
            )
            .with_file(
                "/root/other.memspec",
                "slice other { cell d { type: boolean mutable: true } }",
            );
        let diags = run_str(&loader, "/root/main.memspec");
        let main = diags.get(Path::new("/root/main.memspec")).expect("main diags");
        assert!(main.iter().any(|d| d.code == codes::W_COMP_UNUSED_IMPORT));
    }

    #[test]
    fn used_import_no_warning() {
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
        let diags = run_str(&loader, "/root/main.memspec");
        // No W0273 expected; might still get other compositions warnings on other files.
        let main_diags = diags.get(Path::new("/root/main.memspec")).cloned().unwrap_or_default();
        assert!(!main_diags.iter().any(|d| d.code == codes::W_COMP_UNUSED_IMPORT));
    }

    #[test]
    fn duplicate_target_warns() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    use "./other.memspec" as o2
                    derived d {
                        derives_from: [o.cell_a, o2.cell_a]
                        derivation: "..."
                    }
                }"#,
            )
            .with_file(
                "/root/other.memspec",
                "slice other { cell cell_a { type: boolean mutable: true } }",
            );
        let diags = run_str(&loader, "/root/main.memspec");
        let main = diags.get(Path::new("/root/main.memspec")).expect("main diags");
        assert!(main.iter().any(|d| d.code == codes::W_COMP_DUPLICATE_IMPORT_TARGET));
    }

    #[test]
    fn imported_id_shadowed_by_local_warns() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    cell shared { type: boolean mutable: true }
                    derived d {
                        derives_from: [o.shared, shared]
                        derivation: "..."
                    }
                }"#,
            )
            .with_file(
                "/root/other.memspec",
                "slice other { cell shared { type: enum<a | b> mutable: true } }",
            );
        let diags = run_str(&loader, "/root/main.memspec");
        let main = diags.get(Path::new("/root/main.memspec")).expect("main diags");
        assert!(main.iter().any(|d| d.code == codes::W_COMP_IMPORTED_ID_SHADOWED));
    }

    #[test]
    fn no_shadow_when_local_id_differs() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/main.memspec",
                r#"slice main {
                    use "./other.memspec" as o
                    cell only_local { type: boolean mutable: true }
                    derived d {
                        derives_from: [o.shared, only_local]
                        derivation: "..."
                    }
                }"#,
            )
            .with_file(
                "/root/other.memspec",
                "slice other { cell shared { type: boolean mutable: true } }",
            );
        let diags = run_str(&loader, "/root/main.memspec");
        let main_diags = diags.get(Path::new("/root/main.memspec")).cloned().unwrap_or_default();
        assert!(!main_diags.iter().any(|d| d.code == codes::W_COMP_IMPORTED_ID_SHADOWED));
    }
}
