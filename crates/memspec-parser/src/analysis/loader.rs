//! Cross-slice import loader.
//!
//! Resolves `use "<relative-path>" as <alias>` declarations to parsed
//! [`File`]s. Pluggable via the [`Loader`] trait so tests can substitute
//! an in-memory loader without touching the filesystem.
//!
//! Cycle detection: tracks canonical paths in a visited set; refuses to
//! re-enter a file that's currently being loaded along the dependency
//! chain.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::File;
use crate::diagnostic::{Diagnostic, codes};
use crate::parser;
use crate::span::Span;

/// Pluggable file loader. Production uses [`FsLoader`]; tests can use
/// [`InMemoryLoader`] to avoid disk I/O.
pub trait Loader {
    /// Read the file at `path` (which is the loader's own canonical form).
    /// Returns the source text or an error.
    fn read(&self, path: &Path) -> Result<String, LoadError>;

    /// Canonicalise an import-target path against the importing file's
    /// canonical path. The default impl uses path joining + `canonicalize`
    /// for the FS loader; in-memory loaders can override.
    fn canonicalize(&self, importing_from: &Path, target: &str) -> Result<PathBuf, LoadError> {
        let base = importing_from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let joined = base.join(target);
        Ok(joined)
    }
}

#[derive(Debug)]
pub enum LoadError {
    NotFound(PathBuf),
    Io(String),
    InvalidPath(String),
}

impl LoadError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound(p) => format!("imported file not found: {}", p.display()),
            Self::Io(e) => format!("io error reading import: {e}"),
            Self::InvalidPath(s) => format!("invalid import path: {s}"),
        }
    }
}

/// Filesystem loader. Reads via `std::fs::read_to_string` and canonicalises
/// via `std::fs::canonicalize` (which requires the file to exist).
pub struct FsLoader;

impl Loader for FsLoader {
    fn read(&self, path: &Path) -> Result<String, LoadError> {
        std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LoadError::NotFound(path.to_path_buf())
            } else {
                LoadError::Io(e.to_string())
            }
        })
    }

    fn canonicalize(&self, importing_from: &Path, target: &str) -> Result<PathBuf, LoadError> {
        let base = importing_from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let joined = base.join(target);
        // Best-effort canonicalisation. If the file doesn't exist, return
        // the joined path so the caller can emit a NotFound diagnostic.
        std::fs::canonicalize(&joined).or(Ok(joined))
    }
}

/// In-memory loader for tests and validation. Maps canonical-path strings
/// to source text.
pub struct InMemoryLoader {
    pub files: BTreeMap<PathBuf, String>,
}

impl InMemoryLoader {
    pub fn new() -> Self {
        Self { files: BTreeMap::new() }
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>, source: impl Into<String>) -> Self {
        self.files.insert(path.into(), source.into());
        self
    }
}

impl Default for InMemoryLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl Loader for InMemoryLoader {
    fn read(&self, path: &Path) -> Result<String, LoadError> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| LoadError::NotFound(path.to_path_buf()))
    }

    fn canonicalize(&self, importing_from: &Path, target: &str) -> Result<PathBuf, LoadError> {
        let base = importing_from
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(base.join(target))
    }
}

/// One file loaded into the working set, keyed by its canonical path.
pub struct LoadedFile {
    pub path: PathBuf,
    pub source: String,
    pub file: File,
    pub diagnostics: Vec<Diagnostic>,
    /// alias → canonical path of the imported file, populated by the loader
    /// when each `use` declaration successfully resolves. Failed imports are
    /// absent from this map (their diagnostics live in `diagnostics`).
    pub imports_resolved: BTreeMap<String, PathBuf>,
}

/// The full working set produced by [`load_with_imports`]: the root file
/// plus every transitively imported file, addressable by canonical path.
pub struct WorkingSet {
    pub root: PathBuf,
    /// Insertion order is import-discovery order; used by analyzers when
    /// they need a stable iteration order.
    pub files: Vec<LoadedFile>,
}

impl WorkingSet {
    pub fn root_file(&self) -> Option<&LoadedFile> {
        self.files.iter().find(|f| f.path == self.root)
    }

    pub fn lookup(&self, path: &Path) -> Option<&LoadedFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// Load the file at `root_path` and recursively load every imported file.
/// Returns the full working set; per-file diagnostics live in each
/// `LoadedFile`. Import failures are attributed to the IMPORTING file
/// (with the `use` statement's span).
pub fn load_with_imports(loader: &dyn Loader, root_path: &Path) -> WorkingSet {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut files: Vec<LoadedFile> = Vec::new();

    // Root file — failures attribute to a synthesised LoadedFile for the
    // root since there's no upstream importer to blame.
    match loader.read(root_path) {
        Ok(source) => {
            let pr = parser::parse(&source);
            files.push(LoadedFile {
                path: root_path.to_path_buf(),
                source,
                file: pr.file,
                diagnostics: pr.diagnostics,
                imports_resolved: BTreeMap::new(),
            });
            visited.insert(root_path.to_path_buf());
            walk_imports_of(loader, 0, &mut visited, &mut files, &mut vec![root_path.to_path_buf()]);
        }
        Err(e) => {
            files.push(LoadedFile {
                path: root_path.to_path_buf(),
                source: String::new(),
                file: File { span: Span::new(0, 0), slice: None },
                diagnostics: vec![Diagnostic::error(
                    codes::E_LOADER_NOT_FOUND,
                    Span::new(0, 0),
                    e.message(),
                )],
                imports_resolved: BTreeMap::new(),
            });
        }
    }

    WorkingSet { root: root_path.to_path_buf(), files }
}

/// Walk imports declared in `files[importer_idx]`. For each import:
/// canonicalise; check cycle/visited; read+parse the target; if anything
/// fails, attribute the diagnostic to the IMPORTER's `use` span.
fn walk_imports_of(
    loader: &dyn Loader,
    importer_idx: usize,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<LoadedFile>,
    chain: &mut Vec<PathBuf>,
) {
    let imports: Vec<crate::ast::Import> = files[importer_idx]
        .file
        .slice
        .as_ref()
        .map(|s| s.imports.clone())
        .unwrap_or_default();
    let importer_path = files[importer_idx].path.clone();

    for import in &imports {
        let target = match loader.canonicalize(&importer_path, &import.path) {
            Ok(p) => p,
            Err(e) => {
                files[importer_idx].diagnostics.push(Diagnostic::error(
                    codes::E_LOADER_NOT_FOUND,
                    import.path_span,
                    format!("cannot resolve import path: {}", e.message()),
                ));
                continue;
            }
        };

        if chain.contains(&target) {
            files[importer_idx].diagnostics.push(
                Diagnostic::error(
                    codes::E_LOADER_IMPORT_CYCLE,
                    import.path_span,
                    format!(
                        "import cycle detected: `{}` is already on the import chain",
                        target.display()
                    ),
                )
                .with_hint("break the cycle by removing the offending `use` declaration or restructuring the slices"),
            );
            continue;
        }

        if visited.contains(&target) {
            // Already loaded along a different path. No diagnostic, no
            // re-recursion — but still record the alias resolution so
            // the cross-slice pass can find the imported file.
            files[importer_idx]
                .imports_resolved
                .insert(import.alias.name.clone(), target.clone());
            continue;
        }

        match loader.read(&target) {
            Ok(source) => {
                let pr = parser::parse(&source);
                files.push(LoadedFile {
                    path: target.clone(),
                    source,
                    file: pr.file,
                    diagnostics: pr.diagnostics,
                    imports_resolved: BTreeMap::new(),
                });
                visited.insert(target.clone());
                files[importer_idx]
                    .imports_resolved
                    .insert(import.alias.name.clone(), target.clone());
                let new_idx = files.len() - 1;
                chain.push(target);
                walk_imports_of(loader, new_idx, visited, files, chain);
                chain.pop();
            }
            Err(e) => {
                files[importer_idx].diagnostics.push(Diagnostic::error(
                    codes::E_LOADER_NOT_FOUND,
                    import.path_span,
                    e.message(),
                ));
                // Don't mark as visited — a sibling might successfully resolve
                // the same path differently. (Edge case; harmless if we did.)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_loader_loads_single_file() {
        let loader = InMemoryLoader::new()
            .with_file("/root/a.memspec", "slice a { cell x { type: boolean mutable: true } }");
        let ws = load_with_imports(&loader, Path::new("/root/a.memspec"));
        assert_eq!(ws.files.len(), 1);
        assert_eq!(ws.files[0].path, PathBuf::from("/root/a.memspec"));
        assert!(ws.files[0].diagnostics.is_empty());
        assert!(ws.files[0].file.slice.is_some());
    }

    #[test]
    fn loader_follows_imports() {
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
        let ws = load_with_imports(&loader, Path::new("/root/main.memspec"));
        assert_eq!(ws.files.len(), 2);
        let paths: Vec<_> = ws.files.iter().map(|f| f.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("/root/main.memspec")));
        assert!(paths.contains(&PathBuf::from("/root/other.memspec")));
    }

    #[test]
    fn loader_detects_missing_import() {
        let loader = InMemoryLoader::new().with_file(
            "/root/main.memspec",
            r#"slice main {
                use "./missing.memspec" as m
            }"#,
        );
        let ws = load_with_imports(&loader, Path::new("/root/main.memspec"));
        let main = ws.lookup(Path::new("/root/main.memspec")).unwrap();
        assert!(
            main.diagnostics.iter().any(|d| d.code == codes::E_LOADER_NOT_FOUND),
            "expected NOT_FOUND diagnostic"
        );
    }

    #[test]
    fn loader_detects_import_cycle() {
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/a.memspec",
                r#"slice a { use "./b.memspec" as b }"#,
            )
            .with_file(
                "/root/b.memspec",
                r#"slice b { use "./a.memspec" as a }"#,
            );
        let ws = load_with_imports(&loader, Path::new("/root/a.memspec"));
        let cycle_diag = ws
            .files
            .iter()
            .flat_map(|f| f.diagnostics.iter())
            .find(|d| d.code == codes::E_LOADER_IMPORT_CYCLE);
        assert!(cycle_diag.is_some(), "expected import-cycle diagnostic");
    }

    #[test]
    fn loader_handles_diamond_imports() {
        // a -> b, a -> c, b -> d, c -> d. Each file loaded once.
        let loader = InMemoryLoader::new()
            .with_file(
                "/root/a.memspec",
                r#"slice a { use "./b.memspec" as b use "./c.memspec" as c }"#,
            )
            .with_file(
                "/root/b.memspec",
                r#"slice b { use "./d.memspec" as d }"#,
            )
            .with_file(
                "/root/c.memspec",
                r#"slice c { use "./d.memspec" as d }"#,
            )
            .with_file(
                "/root/d.memspec",
                "slice d { cell x { type: boolean mutable: true } }",
            );
        let ws = load_with_imports(&loader, Path::new("/root/a.memspec"));
        assert_eq!(ws.files.len(), 4, "expected 4 unique files in diamond");
    }
}
