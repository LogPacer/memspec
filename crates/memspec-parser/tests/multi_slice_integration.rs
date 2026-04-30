//! Integration test: load + analyze the multi-slice fixture end-to-end via
//! the actual filesystem loader.

use std::path::Path;

use memspec_parser::analysis::{analyze_working_set, loader::{FsLoader, load_with_imports}};
use memspec_parser::diagnostic::Severity;

#[test]
fn multi_slice_fixture_walks_clean_via_filesystem() {
    let root = Path::new("tests/fixtures/multi_slice/rule_audit.memspec");
    let loader = FsLoader;
    let ws = load_with_imports(&loader, root);

    assert_eq!(ws.files.len(), 2, "expected root + one imported file");

    let analysis = analyze_working_set(&ws);

    let errors: Vec<_> = analysis
        .by_file
        .values()
        .flatten()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "expected zero errors across the working set; got: {errors:#?}"
    );

    // The composed slice's qualified refs must have resolved cleanly.
    let audit_diags = analysis
        .by_file
        .iter()
        .find(|(p, _)| p.ends_with("rule_audit.memspec"))
        .map(|(_, d)| d)
        .expect("audit slice should be in analysis output");
    assert!(
        audit_diags.iter().all(|d| d.severity != Severity::Error),
        "rule_audit should walk clean of errors: {audit_diags:#?}"
    );
}

#[test]
fn missing_qualified_id_emits_diagnostic_via_fs_loader() {
    use std::fs;
    use std::path::PathBuf;

    // Write a temp slice that imports the canonical fixture but uses a bad id.
    let tmpdir = std::env::temp_dir().join(format!("memspec_test_{}", std::process::id()));
    fs::create_dir_all(&tmpdir).expect("mkdir");
    let lifecycle_src = fs::read_to_string("tests/fixtures/multi_slice/rule_lifecycle.memspec")
        .expect("read lifecycle");
    let lifecycle_path = tmpdir.join("rule_lifecycle.memspec");
    fs::write(&lifecycle_path, lifecycle_src).expect("write lifecycle");

    let bad_audit = r#"slice bad_audit {
  use "./rule_lifecycle.memspec" as lc
  meta { title: "bad" memspec_version: "0.1" }
  walk 1 { summary: "uses an id that doesn't exist" }
  cell c { type: boolean mutable: true }
  derived d {
    derives_from: [lc.does_not_exist]
    derivation: "..."
  }
}
"#;
    let audit_path: PathBuf = tmpdir.join("bad_audit.memspec");
    fs::write(&audit_path, bad_audit).expect("write audit");

    let loader = FsLoader;
    let ws = load_with_imports(&loader, &audit_path);
    let analysis = analyze_working_set(&ws);

    let any_unresolved = analysis
        .by_file
        .values()
        .flatten()
        .any(|d| d.code == memspec_parser::diagnostic::codes::E_LOADER_QUALIFIED_REF_UNRESOLVED);
    assert!(any_unresolved, "expected E0403 unresolved qualified ref");

    let _ = fs::remove_dir_all(&tmpdir);
}
