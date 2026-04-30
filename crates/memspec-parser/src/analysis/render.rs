//! Human-readable renderings of a parsed slice.
//!
//! Two formats today:
//! - [`render_markdown`] — narrative-shaped doc you can paste into a PR
//!   description, a wiki, or stare at in a terminal. The format that
//!   answers "what does this slice say?" without reading the DSL.
//! - [`render_mermaid`] — Mermaid graph showing cells/derived/events
//!   nodes and `derives_from` / `mutates` / `kill_test` / `constrained_by`
//!   edges. Pastes into any markdown that supports Mermaid (GitHub, Obsidian).
//!
//! Both walk the AST only — they never ask the analyzer or the disk.
//! The renderer produces a string; the CLI writes it to stdout.

use std::fmt::Write;

use crate::ast::{BlockDecl, BlockItem, BlockName, FieldValue, File, MapEntry, SliceDecl};

use super::loader::WorkingSet;

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

pub fn render_markdown(file: &File) -> String {
    let Some(slice) = &file.slice else {
        return "*(no slice in file)*\n".to_owned();
    };
    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", slice.name.name);

    render_meta_section(slice, &mut out);
    render_walks_section(slice, &mut out);
    render_cells_section(slice, &mut out);
    render_derived_section(slice, &mut out);
    render_associations_section(slice, &mut out);
    render_events_section(slice, &mut out);
    render_post_failure_section(slice, &mut out);
    render_forbidden_states_section(slice, &mut out);
    render_kill_tests_section(slice, &mut out);

    out
}

fn render_meta_section(slice: &SliceDecl, out: &mut String) {
    let Some(meta) = anonymous_block(slice, "meta") else { return };
    let _ = writeln!(out, "## Meta\n");
    for item in &meta.items {
        if let BlockItem::Field(f) = item {
            let _ = writeln!(out, "- **{}**: {}", f.key.name, value_inline(&f.value));
        }
    }
    let _ = writeln!(out);
}

fn render_walks_section(slice: &SliceDecl, out: &mut String) {
    let walks: Vec<&BlockDecl> = slice
        .items
        .iter()
        .filter_map(|i| match i {
            BlockItem::Block(b) if b.kind.name == "walk" => Some(b),
            _ => None,
        })
        .collect();
    if walks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Walks\n");
    for w in walks {
        let n = match &w.name {
            Some(BlockName::Int { value, .. }) => value.to_string(),
            _ => "?".to_owned(),
        };
        let summary = string_field(w, "summary").unwrap_or_else(|| "(no summary)".to_owned());
        let _ = writeln!(out, "- **walk {n}** — {summary}");
        if let Some(FieldValue::List { items, .. }) = field_value(w, "added") {
            let ids = list_idents_inline(items);
            if !ids.is_empty() {
                let _ = writeln!(out, "  - added: {ids}");
            }
        }
        if let Some(FieldValue::List { items, .. }) = field_value(w, "killed") {
            let ids = list_idents_inline(items);
            if !ids.is_empty() {
                let _ = writeln!(out, "  - killed: {ids}");
            }
        }
    }
    let _ = writeln!(out);
}

fn render_cells_section(slice: &SliceDecl, out: &mut String) {
    let cells = slot_blocks(slice, "cell");
    if cells.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Cells\n");
    for c in cells {
        let id = block_name_or(c, "?");
        let ty = field_value(c, "type")
            .map(value_inline)
            .unwrap_or_else(|| "(no type)".to_owned());
        let mutability = if field_is_true(c, "mutable") { "mutable" } else { "immutable" };
        let _ = writeln!(out, "- **`{id}`** — `{ty}` ({mutability})");
        if let Some(default) = field_value(c, "default") {
            let _ = writeln!(out, "  - default: `{}`", value_inline(default));
        }
        if let Some(reff) = string_field(c, "ref") {
            let _ = writeln!(out, "  - ref: `{reff}`");
        }
        if let Some(cfg) = field_value(c, "cfg") {
            let _ = writeln!(out, "  - cfg: `{}`", value_inline(cfg));
        }
    }
    let _ = writeln!(out);
}

fn render_derived_section(slice: &SliceDecl, out: &mut String) {
    let blocks = slot_blocks(slice, "derived");
    if blocks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Derived\n");
    for d in blocks {
        let id = block_name_or(d, "?");
        let sources = list_field_inline(d, "derives_from");
        let _ = writeln!(out, "- **`{id}`** ← {sources}");
        if let Some(rule) = string_field(d, "derivation") {
            let _ = writeln!(out, "  - *derivation:* {}", quote(&rule));
        }
        if let Some(FieldValue::Bool { value: false, .. }) = field_value(d, "materialised") {
            let _ = writeln!(out, "  - *materialised:* false (computed-on-call)");
        }
    }
    let _ = writeln!(out);
}

fn render_associations_section(slice: &SliceDecl, out: &mut String) {
    let blocks = slot_blocks(slice, "association");
    if blocks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Associations\n");
    for a in blocks {
        let id = block_name_or(a, "?");
        let over = list_field_inline(a, "over");
        let _ = writeln!(out, "- **`{id}`** over {over}");
        if let Some(inv) = string_field(a, "invariant") {
            let _ = writeln!(out, "  - *invariant:* {}", quote(&inv));
        }
        if let Some(enforced) = field_value(a, "enforced_by") {
            let _ = writeln!(out, "  - *enforced by:* `{}`", value_inline(enforced));
        }
    }
    let _ = writeln!(out);
}

fn render_events_section(slice: &SliceDecl, out: &mut String) {
    let blocks = slot_blocks(slice, "event");
    if blocks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Events\n");
    for e in blocks {
        let id = block_name_or(e, "?");
        let _ = writeln!(out, "### `{id}`\n");
        if let Some(t) = string_field(e, "trigger") {
            let _ = writeln!(out, "- **trigger:** {}", quote(&t));
        }
        let mutates = list_field_inline(e, "mutates");
        if !mutates.is_empty() {
            let _ = writeln!(out, "- **mutates:** {mutates}");
        }
        if let Some(atom) = field_value(e, "atomicity") {
            let _ = writeln!(out, "- **atomicity:** `{}`", value_inline(atom));
        }
        if let Some(ser) = field_value(e, "serialization") {
            let _ = writeln!(out, "- **serialization:** `{}`", value_inline(ser));
        }
        let steps: Vec<&BlockDecl> = e
            .items
            .iter()
            .filter_map(|i| match i {
                BlockItem::Block(b) if b.kind.name == "step" => Some(b),
                _ => None,
            })
            .collect();
        if !steps.is_empty() {
            let _ = writeln!(out, "\n| step | op | fallible | mutates | precondition |");
            let _ = writeln!(out, "|---|---|---|---|---|");
            for s in steps {
                let sid = block_name_or(s, "?");
                let op = string_field(s, "op").unwrap_or_else(|| "—".to_owned());
                let fallible = if field_is_true(s, "fallible") { "yes" } else { "no" };
                let mutates = list_field_inline(s, "mutates");
                let precond = string_field(s, "precondition").unwrap_or_else(|| "—".to_owned());
                let _ = writeln!(
                    out,
                    "| `{sid}` | `{}` | {fallible} | {} | {} |",
                    md_escape(&op),
                    if mutates.is_empty() { "—".to_owned() } else { mutates },
                    md_escape(&precond),
                );
            }
        }
        let _ = writeln!(out);
    }
}

fn render_post_failure_section(slice: &SliceDecl, out: &mut String) {
    let blocks = slot_blocks(slice, "post_failure");
    if blocks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Post-failure rows\n");
    for pf in blocks {
        let id = block_name_or(pf, "?");
        let event = field_ident_str(pf, "event").unwrap_or("?");
        let step = field_ident_str(pf, "step").unwrap_or("?");
        let outcome = string_field(pf, "outcome").unwrap_or_else(|| "?".to_owned());
        let _ = writeln!(out, "### `{id}` — `{event}.{step}` → {}\n", quote(&outcome));
        for variant in ["cells_after_pre_rollback", "cells_after_rollback", "cells_after"] {
            if let Some(b) = nested_block(pf, variant) {
                let _ = writeln!(out, "- **{}:**", pretty_label(variant));
                for item in &b.items {
                    if let BlockItem::Field(f) = item {
                        let _ = writeln!(
                            out,
                            "  - `{}`: `{}`",
                            f.key.name,
                            value_inline(&f.value)
                        );
                    }
                }
            }
        }
        if let Some(result) = field_value(pf, "result") {
            let _ = writeln!(out, "- **result:** `{}`", value_inline(result));
        }
        if let Some(FieldValue::List { items, .. }) = field_value(pf, "invariants_held_after_rollback") {
            let inv = list_idents_inline(items);
            if !inv.is_empty() {
                let _ = writeln!(out, "- **invariants held after rollback:** {inv}");
            }
        }
        let _ = writeln!(out);
    }
}

fn render_forbidden_states_section(slice: &SliceDecl, out: &mut String) {
    let blocks = slot_blocks(slice, "forbidden_state");
    if blocks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Forbidden states\n");
    for fs in blocks {
        let id = block_name_or(fs, "?");
        let reach = field_ident_str(fs, "reachability").unwrap_or("?");
        let _ = writeln!(out, "### `{id}` — *{reach}*\n");
        if let Some(d) = string_field(fs, "description") {
            let _ = writeln!(out, "{}\n", d);
        }
        if let Some(p) = string_field(fs, "predicate") {
            let _ = writeln!(out, "- **predicate:** {}", quote(&p));
        }
        if let Some(FieldValue::Map { entries, .. }) = field_value(fs, "cells") {
            let _ = writeln!(out, "- **cells:**");
            for MapEntry { key, value, .. } in entries {
                let _ = writeln!(out, "  - `{}`: `{}`", key.name, value_inline(value));
            }
        }
        if let Some(kt) = field_value(fs, "kill_test") {
            let _ = writeln!(out, "- **kill_test:** `{}`", value_inline(kt));
        }
        let _ = writeln!(out);
    }
}

fn render_kill_tests_section(slice: &SliceDecl, out: &mut String) {
    let blocks = slot_blocks(slice, "kill_test");
    if blocks.is_empty() {
        return;
    }
    let _ = writeln!(out, "## Kill-tests\n");
    for kt in blocks {
        let id = block_name_or(kt, "?");
        let kind = field_ident_str(kt, "kind").unwrap_or("?");
        let status = field_ident_str(kt, "status").unwrap_or("declared");
        let forbidden = field_ident_str(kt, "forbidden").unwrap_or("?");
        let _ = writeln!(
            out,
            "### `{id}` — kind: *{kind}*, status: *{status}*\n"
        );
        let _ = writeln!(out, "- **forbidden:** `{forbidden}`");
        if let Some(a) = string_field(kt, "assertion") {
            let _ = writeln!(out, "- **assertion:** {}", quote(&a));
        }
        if let Some(r) = string_field(kt, "ref") {
            let _ = writeln!(out, "- **ref:** `{r}`");
        }
        let _ = writeln!(out);
    }
}

// ---------------------------------------------------------------------------
// Markdown — multi-slice aggregate
// ---------------------------------------------------------------------------

/// Render the entire working set as one markdown document. Each file gets
/// its own `# Slice: <name>` section; cross-slice qualified refs render
/// with bold formatting so they're visually distinct from local refs.
pub fn render_markdown_aggregate(ws: &WorkingSet) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Working set\n");
    let _ = writeln!(out, "**Root:** `{}`\n", ws.root.display());
    let _ = writeln!(out, "## Slices in this working set\n");
    for lf in &ws.files {
        let slug = lf.file.slice.as_ref().map(|s| s.name.name.clone()).unwrap_or_else(|| "<unnamed>".to_owned());
        let marker = if lf.path == ws.root { " *(root)*" } else { "" };
        let _ = writeln!(out, "- `{slug}`{marker} — `{}`", lf.path.display());
    }
    let _ = writeln!(out);

    // Dependency graph (alias → imported slug).
    let _ = writeln!(out, "## Dependency graph\n");
    for lf in &ws.files {
        let Some(slice) = &lf.file.slice else { continue };
        if slice.imports.is_empty() {
            continue;
        }
        let _ = writeln!(out, "- `{}` imports:", slice.name.name);
        for import in &slice.imports {
            let target = lf
                .imports_resolved
                .get(import.alias.name.as_str())
                .map(|p| {
                    ws.files
                        .iter()
                        .find(|other| &other.path == p)
                        .and_then(|other| other.file.slice.as_ref().map(|s| s.name.name.clone()))
                        .unwrap_or_else(|| import.path.clone())
                })
                .unwrap_or_else(|| import.path.clone());
            let _ = writeln!(out, "  - `{}` → `{}`", import.alias.name, target);
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "---\n");

    // Per-slice rendering.
    for lf in &ws.files {
        let Some(slice) = &lf.file.slice else { continue };
        let _ = writeln!(out, "# Slice: `{}`\n", slice.name.name);
        let _ = writeln!(out, "*File: `{}`*\n", lf.path.display());

        if !slice.imports.is_empty() {
            let _ = writeln!(out, "## Imports\n");
            for import in &slice.imports {
                let _ = writeln!(out, "- `use \"{}\" as {}`", import.path, import.alias.name);
            }
            let _ = writeln!(out);
        }

        // Reuse the single-file renderer for the slot sections, but skip the
        // top-level `# <slug>` title since we already emitted it above.
        let single = render_markdown(&lf.file);
        // Strip the leading "# <name>\n" line.
        let body = single.lines().skip(2).collect::<Vec<_>>().join("\n");
        out.push_str(&body);
        let _ = writeln!(out, "\n---\n");
    }

    out
}

// ---------------------------------------------------------------------------
// Mermaid
// ---------------------------------------------------------------------------

pub fn render_mermaid(file: &File) -> String {
    let Some(slice) = &file.slice else {
        return "graph TD\n  empty[\"(no slice)\"]\n".to_owned();
    };
    let mut out = String::new();
    let _ = writeln!(out, "graph TD");
    let _ = writeln!(out, "  %% slice: {}", slice.name.name);
    let _ = writeln!(out);

    // Nodes — distinct shapes per slot kind.
    for c in slot_blocks(slice, "cell") {
        let id = block_name_or(c, "?");
        let ty = field_value(c, "type").map(value_inline).unwrap_or_default();
        let _ = writeln!(out, "  {}[(\"{id}<br/><i>{}</i>\")]", mid(id), mermaid_escape(&ty));
    }
    for d in slot_blocks(slice, "derived") {
        let id = block_name_or(d, "?");
        let _ = writeln!(out, "  {}{{{{\"{id}<br/>(derived)\"}}}}", mid(id));
    }
    for a in slot_blocks(slice, "association") {
        let id = block_name_or(a, "?");
        let _ = writeln!(out, "  {}[/\"{id}<br/>(invariant)\"\\]", mid(id));
    }
    for e in slot_blocks(slice, "event") {
        let id = block_name_or(e, "?");
        let _ = writeln!(out, "  {}([\"{id}<br/>(event)\"])", mid(id));
    }
    for fs in slot_blocks(slice, "forbidden_state") {
        let id = block_name_or(fs, "?");
        let reach = field_ident_str(fs, "reachability").unwrap_or("?");
        let _ = writeln!(out, "  {}{{{{\"⛔ {id}<br/><i>{reach}</i>\"}}}}", mid(id));
    }
    for kt in slot_blocks(slice, "kill_test") {
        let id = block_name_or(kt, "?");
        let kind = field_ident_str(kt, "kind").unwrap_or("?");
        let status = field_ident_str(kt, "status").unwrap_or("declared");
        let _ = writeln!(out, "  {}[/\"🎯 {id}<br/>{kind} · {status}\"/]", mid(id));
    }

    let _ = writeln!(out);

    // Edges.
    for d in slot_blocks(slice, "derived") {
        let id = block_name_or(d, "?");
        if let Some(FieldValue::List { items, .. }) = field_value(d, "derives_from") {
            for item in items {
                if let FieldValue::Ident(i) = item {
                    let _ = writeln!(out, "  {} -.derives.-> {}", mid(&i.name), mid(id));
                }
            }
        }
    }
    for a in slot_blocks(slice, "association") {
        let id = block_name_or(a, "?");
        if let Some(FieldValue::List { items, .. }) = field_value(a, "over") {
            for item in items {
                if let FieldValue::Ident(i) = item {
                    let _ = writeln!(out, "  {} -.constrains.-> {}", mid(id), mid(&i.name));
                }
            }
        }
    }
    for e in slot_blocks(slice, "event") {
        let id = block_name_or(e, "?");
        if let Some(FieldValue::List { items, .. }) = field_value(e, "mutates") {
            for item in items {
                if let FieldValue::Ident(i) = item {
                    let _ = writeln!(out, "  {} ==mutates==> {}", mid(id), mid(&i.name));
                }
            }
        }
    }
    for fs in slot_blocks(slice, "forbidden_state") {
        let id = block_name_or(fs, "?");
        if let Some(FieldValue::Map { entries, .. }) = field_value(fs, "cells") {
            for MapEntry { key, .. } in entries {
                let _ = writeln!(out, "  {} -.touches.-> {}", mid(id), mid(&key.name));
            }
        }
        if let Some(FieldValue::Ident(kt_ref)) = field_value(fs, "kill_test") {
            if kt_ref.name != "TODO" {
                let _ = writeln!(out, "  {} ==kill==> {}", mid(&kt_ref.name), mid(id));
            }
        }
    }
    for pf in slot_blocks(slice, "post_failure") {
        let id = block_name_or(pf, "?");
        let _ = writeln!(out, "  {}[\"📜 {id}<br/>(post_failure)\"]", mid(id));
        if let Some(ev) = field_ident_str(pf, "event") {
            let _ = writeln!(out, "  {} -.documents.-> {}", mid(id), mid(ev));
        }
    }

    out
}

/// Mermaid node IDs must be safe identifiers. Map our IDs to a safe form.
fn mid(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            s.push(c);
        } else {
            s.push('_');
        }
    }
    s
}

/// Slug-prefixed Mermaid node ID for cross-slice rendering. Avoids
/// collisions when two slices declare ids with the same name.
fn pmid(slice_slug: &str, name: &str) -> String {
    format!("{}__{}", mid(slice_slug), mid(name))
}

// ---------------------------------------------------------------------------
// Mermaid — multi-slice aggregate
// ---------------------------------------------------------------------------

/// Render the entire working set as ONE Mermaid graph. Each slice gets its
/// own `subgraph` block; node IDs are slug-prefixed so they don't collide.
/// Cross-slice qualified refs render as edges between subgraphs.
pub fn render_mermaid_aggregate(ws: &WorkingSet) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "graph TD");
    let _ = writeln!(out, "  %% working set rooted at {}", ws.root.display());
    let _ = writeln!(out);

    // Build slug → canonical-path map so we can resolve `<alias>.<id>` to
    // the target slice's slug for prefixing.
    let mut path_to_slug: std::collections::BTreeMap<&std::path::Path, &str> = std::collections::BTreeMap::new();
    for lf in &ws.files {
        if let Some(s) = lf.file.slice.as_ref().map(|s| s.name.name.as_str()) {
            path_to_slug.insert(lf.path.as_path(), s);
        }
    }

    // Per-slice subgraph: nodes only.
    for lf in &ws.files {
        let Some(slice) = &lf.file.slice else { continue };
        let slug = slice.name.name.as_str();
        let _ = writeln!(out, "  subgraph {}_box [\"slice {}\"]", mid(slug), slug);
        for c in slot_blocks(slice, "cell") {
            let id = block_name_or(c, "?");
            let ty = field_value(c, "type").map(value_inline).unwrap_or_default();
            let _ = writeln!(out, "    {}[(\"{id}<br/><i>{}</i>\")]", pmid(slug, id), mermaid_escape(&ty));
        }
        for d in slot_blocks(slice, "derived") {
            let id = block_name_or(d, "?");
            let _ = writeln!(out, "    {}{{{{\"{id}<br/>(derived)\"}}}}", pmid(slug, id));
        }
        for a in slot_blocks(slice, "association") {
            let id = block_name_or(a, "?");
            let _ = writeln!(out, "    {}[/\"{id}<br/>(invariant)\"\\]", pmid(slug, id));
        }
        for e in slot_blocks(slice, "event") {
            let id = block_name_or(e, "?");
            let _ = writeln!(out, "    {}([\"{id}<br/>(event)\"])", pmid(slug, id));
        }
        for fs in slot_blocks(slice, "forbidden_state") {
            let id = block_name_or(fs, "?");
            let reach = field_ident_str(fs, "reachability").unwrap_or("?");
            let _ = writeln!(out, "    {}{{{{\"⛔ {id}<br/><i>{reach}</i>\"}}}}", pmid(slug, id));
        }
        for kt in slot_blocks(slice, "kill_test") {
            let id = block_name_or(kt, "?");
            let kind = field_ident_str(kt, "kind").unwrap_or("?");
            let status = field_ident_str(kt, "status").unwrap_or("declared");
            let _ = writeln!(out, "    {}[/\"🎯 {id}<br/>{kind} · {status}\"/]", pmid(slug, id));
        }
        for pf in slot_blocks(slice, "post_failure") {
            let id = block_name_or(pf, "?");
            let _ = writeln!(out, "    {}[\"📜 {id}<br/>(post_failure)\"]", pmid(slug, id));
        }
        let _ = writeln!(out, "  end");
    }

    let _ = writeln!(out);

    // Edges, both intra- and cross-slice.
    for lf in &ws.files {
        let Some(slice) = &lf.file.slice else { continue };
        let slug = slice.name.name.as_str();

        for d in slot_blocks(slice, "derived") {
            let id = block_name_or(d, "?");
            if let Some(value) = field_value(d, "derives_from") {
                let mut cb = |src: String| {
                    let _ = writeln!(out, "  {} -.derives.-> {}", src, pmid(slug, id));
                };
                edges_from_value(value, slug, &lf.imports_resolved, &path_to_slug, &mut cb);
            }
        }
        for a in slot_blocks(slice, "association") {
            let id = block_name_or(a, "?");
            if let Some(value) = field_value(a, "over") {
                let mut cb = |dst: String| {
                    let _ = writeln!(out, "  {} -.constrains.-> {}", pmid(slug, id), dst);
                };
                edges_from_value(value, slug, &lf.imports_resolved, &path_to_slug, &mut cb);
            }
        }
        for e in slot_blocks(slice, "event") {
            let id = block_name_or(e, "?");
            if let Some(value) = field_value(e, "mutates") {
                let mut cb = |dst: String| {
                    let _ = writeln!(out, "  {} ==mutates==> {}", pmid(slug, id), dst);
                };
                edges_from_value(value, slug, &lf.imports_resolved, &path_to_slug, &mut cb);
            }
        }
        for fs in slot_blocks(slice, "forbidden_state") {
            let id = block_name_or(fs, "?");
            if let Some(FieldValue::Map { entries, .. }) = field_value(fs, "cells") {
                for MapEntry { key, .. } in entries {
                    let _ = writeln!(out, "  {} -.touches.-> {}", pmid(slug, id), pmid(slug, &key.name));
                }
            }
            if let Some(FieldValue::Ident(kt_ref)) = field_value(fs, "kill_test") {
                if kt_ref.name != "TODO" {
                    let _ = writeln!(out, "  {} ==kill==> {}", pmid(slug, &kt_ref.name), pmid(slug, id));
                }
            }
        }
        for pf in slot_blocks(slice, "post_failure") {
            let id = block_name_or(pf, "?");
            if let Some(ev) = field_ident_str(pf, "event") {
                let _ = writeln!(out, "  {} -.documents.-> {}", pmid(slug, id), pmid(slug, ev));
            }
        }

        // Import edges between slice subgraph "anchors" — visualise the
        // dependency graph at the slice-level too.
        for import in &slice.imports {
            if let Some(target_path) = lf.imports_resolved.get(import.alias.name.as_str()) {
                if let Some(target_slug) = path_to_slug.get(target_path.as_path()) {
                    let _ = writeln!(
                        out,
                        "  {}_box -.imports as {}.-> {}_box",
                        mid(slug),
                        import.alias.name,
                        mid(target_slug),
                    );
                }
            }
        }
    }

    out
}

/// For each ident or qualified ident appearing in a field value, call `emit`
/// with the appropriate slug-prefixed Mermaid id (resolving the alias
/// against the importing file's `imports_resolved` map). Uses a trait
/// object for the callback so recursion doesn't blow the type-instantiation
/// limit.
fn edges_from_value(
    value: &FieldValue,
    local_slug: &str,
    imports_resolved: &std::collections::BTreeMap<String, std::path::PathBuf>,
    path_to_slug: &std::collections::BTreeMap<&std::path::Path, &str>,
    emit: &mut dyn FnMut(String),
) {
    match value {
        FieldValue::Ident(i) => emit(pmid(local_slug, &i.name)),
        FieldValue::QualifiedIdent { alias, name, .. } => {
            if let Some(target_path) = imports_resolved.get(alias.name.as_str()) {
                if let Some(target_slug) = path_to_slug.get(target_path.as_path()) {
                    emit(pmid(target_slug, &name.name));
                }
            }
        }
        FieldValue::List { items, .. } => {
            for it in items {
                edges_from_value(it, local_slug, imports_resolved, path_to_slug, emit);
            }
        }
        _ => {}
    }
}

fn mermaid_escape(s: &str) -> String {
    // Inside Mermaid quoted node labels: `<` and `>` need escaping (HTML),
    // `"` needs escaping (closes the label). `|` is fine inside quoted
    // labels — only special in arrow contexts.
    s.replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn slot_blocks<'a>(slice: &'a SliceDecl, kind: &str) -> Vec<&'a BlockDecl> {
    slice
        .items
        .iter()
        .filter_map(|i| match i {
            BlockItem::Block(b) if b.kind.name == kind => Some(b),
            _ => None,
        })
        .collect()
}

fn anonymous_block<'a>(slice: &'a SliceDecl, kind: &str) -> Option<&'a BlockDecl> {
    slice.items.iter().find_map(|i| match i {
        BlockItem::Block(b) if b.kind.name == kind && b.name.is_none() => Some(b),
        _ => None,
    })
}

fn nested_block<'a>(block: &'a BlockDecl, kind: &str) -> Option<&'a BlockDecl> {
    block.items.iter().find_map(|i| match i {
        BlockItem::Block(b) if b.kind.name == kind => Some(b),
        _ => None,
    })
}

fn field_value<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a FieldValue> {
    block.items.iter().find_map(|i| match i {
        BlockItem::Field(f) if f.key.name == key => Some(&f.value),
        _ => None,
    })
}

fn field_is_true(block: &BlockDecl, key: &str) -> bool {
    matches!(field_value(block, key), Some(FieldValue::Bool { value: true, .. }))
}

fn field_ident_str<'a>(block: &'a BlockDecl, key: &str) -> Option<&'a str> {
    match field_value(block, key) {
        Some(FieldValue::Ident(i)) => Some(i.name.as_str()),
        _ => None,
    }
}

fn string_field(block: &BlockDecl, key: &str) -> Option<String> {
    match field_value(block, key) {
        Some(FieldValue::String { value, .. }) => Some(value.clone()),
        _ => None,
    }
}

fn block_name_or<'b>(block: &'b BlockDecl, fallback: &'b str) -> &'b str {
    match &block.name {
        Some(BlockName::Ident(i)) => i.name.as_str(),
        _ => fallback,
    }
}

fn list_field_inline(block: &BlockDecl, key: &str) -> String {
    match field_value(block, key) {
        Some(FieldValue::List { items, .. }) => list_idents_inline(items),
        _ => String::new(),
    }
}

fn list_idents_inline(items: &[FieldValue]) -> String {
    let parts: Vec<String> = items
        .iter()
        .filter_map(|v| match v {
            FieldValue::Ident(i) => Some(format!("`{}`", i.name)),
            other => Some(format!("`{}`", value_inline(other))),
        })
        .collect();
    parts.join(", ")
}

/// Render a FieldValue in compact inline form for embedding in prose.
fn value_inline(value: &FieldValue) -> String {
    match value {
        FieldValue::Ident(i) => i.name.clone(),
        FieldValue::Bool { value, .. } => value.to_string(),
        FieldValue::Int { value, .. } => value.to_string(),
        FieldValue::String { value, .. } => format!("\"{}\"", value),
        FieldValue::List { items, .. } => {
            let parts: Vec<String> = items.iter().map(value_inline).collect();
            format!("[{}]", parts.join(", "))
        }
        FieldValue::Map { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|MapEntry { key, value, .. }| format!("{}: {}", key.name, value_inline(value)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        FieldValue::TypeApp { head, params, alternation, .. } => {
            let sep = if *alternation { " | " } else { ", " };
            let inner: Vec<String> = params.iter().map(value_inline).collect();
            format!("{}<{}>", head.name, inner.join(sep))
        }
        FieldValue::Call { head, args, .. } => {
            let inner: Vec<String> = args.iter().map(value_inline).collect();
            format!("{}({})", head.name, inner.join(", "))
        }
        FieldValue::QualifiedIdent { alias, name, .. } => {
            format!("{}.{}", alias.name, name.name)
        }
    }
}

fn quote(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.contains('\n') {
        format!("\n  > {}\n", trimmed.replace('\n', "\n  > "))
    } else {
        format!("> {trimmed}")
    }
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn pretty_label(s: &str) -> String {
    s.replace('_', " ")
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
    fn markdown_renders_canonical_fixture() {
        let f = parse_fixture();
        let md = render_markdown(&f);
        // Sanity: headers for every populated slot kind.
        for header in [
            "# rule_lifecycle_minimal",
            "## Cells",
            "## Derived",
            "## Associations",
            "## Events",
            "## Post-failure rows",
            "## Forbidden states",
            "## Kill-tests",
        ] {
            assert!(md.contains(header), "missing header `{header}`");
        }
        // Sanity: cell ids appear inline.
        assert!(md.contains("`rule_state`"));
        assert!(md.contains("`rule_active`"));
        assert!(md.contains("`rule_changelogs`"));
        // Type vocab survives.
        assert!(md.contains("enum<draft | published | archived>"));
        // Table for events.
        assert!(md.contains("| step | op | fallible | mutates | precondition |"));
    }

    #[test]
    fn mermaid_renders_canonical_fixture() {
        let f = parse_fixture();
        let g = render_mermaid(&f);
        assert!(g.starts_with("graph TD"));
        // Cell node shape.
        assert!(g.contains("rule_state[("));
        // Derived node shape.
        assert!(g.contains("rule_is_live{{"));
        // Event node + edges.
        assert!(g.contains("promote(["));
        assert!(g.contains("promote ==mutates==> rule_state"));
        // Forbidden state + kill-test.
        assert!(g.contains("⛔ fs_archived_active"));
        assert!(g.contains("kt_archived_active ==kill==> fs_archived_active"));
    }

    #[test]
    fn markdown_handles_empty_slice() {
        let pr = parse("slice empty { }");
        let md = render_markdown(&pr.file);
        assert!(md.contains("# empty"));
    }
}
