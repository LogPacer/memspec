//! Integration test for the CLI's "wrong file type" heuristic.
//!
//! The heuristic itself lives in the CLI binary; this integration test
//! invokes the binary via `cargo run` semantics and checks that:
//! - A markdown file produces ONE friendly E0006 diagnostic
//! - A .memspec file with bad content still gets honest reporting

use std::process::Command;

fn cli() -> Command {
    let bin = env!("CARGO_BIN_EXE_memspec");
    Command::new(bin)
}

#[test]
fn markdown_file_gets_friendly_e0006_not_diagnostic_flood() {
    // Synthesise a markdown file in tempdir — independent of slicer output.
    let dir = std::env::temp_dir().join(format!("memspec_md_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plan_file = dir.join("slice-plan.md");
    std::fs::write(
        &plan_file,
        "# Slice plan — example\n\
         \n\
         **Decomposition rationale:** ecosystem schema drift.\n\
         \n\
         ## Slices\n\
         \n\
         ### 1. `api-contract` — owner of strong-params allowlists\n\
         - Owns: candidate_attributes, review_params\n\
         - File: `api-contract.memspec`\n\
         \n\
         ## Dependency graph\n\
         \n\
         api-contract <- mcp-contract <- consumer-contract\n",
    )
    .unwrap();

    let out = cli()
        .args(["walk", plan_file.to_str().unwrap(), "--json"])
        .output()
        .expect("walk markdown");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Exit code: 2 (parse-class — file isn't lexable as a .memspec).
    assert_eq!(out.status.code(), Some(2), "expected exit 2 for non-.memspec file");

    // Should have exactly ONE diagnostic of code memspec/E0006.
    assert!(
        stdout.contains("memspec/E0006"),
        "expected E0006 diagnostic; got:\n{stdout}"
    );
    let e0003_count = stdout.matches("memspec/E0003").count();
    assert_eq!(
        e0003_count, 0,
        "expected E0006 to replace E0003 flood; got {e0003_count} E0003s"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn real_memspec_with_bad_content_still_reports_honestly() {
    let dir = std::env::temp_dir().join(format!("memspec_wrong_file_type_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("typo.memspec");
    std::fs::write(
        &f,
        r#"slice s {
            cell foo { type: enum<a | b | c> mutable: true }
            derived d { derives_from: [ghost_cell] derivation: "..." }
        }"#,
    )
    .unwrap();

    let out = cli()
        .args(["walk", f.to_str().unwrap(), "--json"])
        .output()
        .expect("walk typo");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Exit 3 — semantic error (unresolved cell ref). The heuristic must
    // NOT trigger here (real .memspec, just bad content).
    assert_eq!(out.status.code(), Some(3), "expected exit 3 for unresolved cell");
    assert!(
        stdout.contains("memspec/E0252"),
        "expected E0252 unresolved-cell-ref; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("memspec/E0006"),
        "heuristic should NOT trigger on real .memspec with bad content"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
