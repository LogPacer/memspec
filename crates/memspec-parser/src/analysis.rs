//! Analyzer passes — structural completeness, coherence, symmetric-failure.
//!
//! Per the role-separation lock in CLAUDE.md, these passes operate on the
//! AST alone. They never open source files or run tests.
//!
//! Passes are layered:
//! 1. [`structural`] — required-field presence per slot. The shallowest
//!    pass; runs first because later passes assume slot bodies are
//!    well-shaped enough to walk.
//! 2. `coherence` — ID uniqueness, ref resolution, type-domain validity,
//!    derivation cycles. (Pending.)
//! 3. `symmetric_failure` — post_failure coverage for events with ≥2
//!    fallible steps. (Pending.)

pub mod coherence;
pub mod composition;
pub mod cross_slice;
pub mod diff;
pub mod loader;
pub mod query;
pub mod render;
#[cfg(feature = "experimental-revisions")]
pub mod revisions;
pub mod structural;
pub mod suggest;
pub mod symmetric_failure;

#[cfg(all(feature = "experimental-revisions", not(debug_assertions)))]
compile_error!(
    "experimental-revisions is a debug-only prototype and must not be built for release profiles"
);

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::ast::File;
use crate::diagnostic::Diagnostic;

use loader::WorkingSet;

#[derive(Debug, Default)]
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
}

/// Single-file analysis. Runs structural + coherence + symmetric-failure
/// against ONE file's AST. Qualified refs (`alias.id`) are NOT resolved by
/// this entry point — call [`analyze_working_set`] when imports are in play.
///
/// Pass ordering per `docs/grammar-v0.md`:
/// 1. **structural** — required-field presence per slot.
/// 2. **coherence** — symbol table, ref resolution, derivation acyclicity,
///    bipartite consistency, warnings.
/// 3. **symmetric-failure** — post_failure coverage for events with ≥2
///    fallible steps.
pub fn analyze(file: &File) -> AnalysisResult {
    let mut diagnostics = Vec::new();
    structural::run(file, &mut diagnostics);
    coherence::run(file, &mut diagnostics);
    symmetric_failure::run(file, &mut diagnostics);
    AnalysisResult { diagnostics }
}

/// Multi-file analysis. Runs the per-file passes against EVERY file in the
/// working set, then runs cross-slice qualified-ref resolution. Diagnostics
/// keyed by canonical path. Loader-emitted diagnostics (parse errors,
/// missing imports, cycles) are merged in. False-positive `W0270` (unused
/// cell) warnings are suppressed when the cell IS referenced via a
/// qualified ref from another file in the working set.
pub fn analyze_working_set(ws: &WorkingSet) -> WorkingSetAnalysis {
    let mut by_file: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();

    // Per-file passes — start each file's diagnostic vector with whatever
    // the loader already attached (parse errors, etc.).
    for lf in &ws.files {
        let mut diagnostics = lf.diagnostics.clone();
        structural::run(&lf.file, &mut diagnostics);
        coherence::run(&lf.file, &mut diagnostics);
        symmetric_failure::run(&lf.file, &mut diagnostics);
        by_file.insert(lf.path.clone(), diagnostics);
    }

    // Cross-slice qualified-ref resolution diagnostics.
    for (path, mut diags) in cross_slice::resolve(ws) {
        by_file.entry(path).or_default().append(&mut diags);
    }

    // Phase B composition warnings (unused imports, duplicate targets,
    // imported-id-shadowed-by-local-id).
    for (path, mut diags) in composition::run(ws) {
        by_file.entry(path).or_default().append(&mut diags);
    }

    // Suppress W0270 unused-cell warnings on cells consumed via qualified
    // refs from other files. The single-file coherence pass can't see
    // cross-slice usage; this post-process makes it whole.
    let cross_refs = cross_slice::cross_referenced_ids(ws);
    for (path, ids) in &cross_refs {
        if let Some(diags) = by_file.get_mut(path) {
            diags.retain(|d| {
                if d.code != crate::diagnostic::codes::W_COH_UNUSED_CELL {
                    return true;
                }
                // Message format: "cell `<id>` is declared but never referenced"
                let cell_id = d.message.split('`').nth(1).unwrap_or("");
                !ids.contains(cell_id)
            });
        }
    }

    WorkingSetAnalysis { by_file }
}

#[derive(Debug, Default)]
pub struct WorkingSetAnalysis {
    pub by_file: BTreeMap<PathBuf, Vec<Diagnostic>>,
}

impl WorkingSetAnalysis {
    pub fn total_diagnostics(&self) -> usize {
        self.by_file.values().map(Vec::len).sum()
    }

    pub fn has_errors(&self) -> bool {
        self.by_file
            .values()
            .flatten()
            .any(|d| d.severity == crate::diagnostic::Severity::Error)
    }
}
