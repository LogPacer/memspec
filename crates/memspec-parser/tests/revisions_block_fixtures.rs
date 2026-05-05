#![cfg(feature = "experimental-revisions")]

use memspec_parser::{
    Severity,
    analysis::revisions::{canonical_source_projection, source_sha256},
    diagnostic::codes,
    parser,
};

fn analyze_source(source: &str) -> Vec<memspec_parser::Diagnostic> {
    let parse = parser::parse(source);
    let mut diagnostics = parse.diagnostics;
    diagnostics.extend(memspec_parser::analyze(&parse.file).diagnostics);
    diagnostics
}

fn has_code(diagnostics: &[memspec_parser::Diagnostic], code: &'static str) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.code == code)
}

fn projection_and_hash(source: &str) -> (String, String) {
    let file = parser::parse(source).file;
    let projection = canonical_source_projection(&file);
    let hash = source_sha256(&projection);
    (projection, hash)
}

fn q(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn base_source() -> &'static str {
    "slice s {\n  cell c { type: boolean mutable: true }\n}\n"
}

fn source_with_revisions(entries: &str) -> String {
    format!(
        "slice s {{\n  cell c {{ type: boolean mutable: true }}\n\n  revisions {{\n{entries}  }}\n}}\n"
    )
}

fn revision_entry(
    revision: u64,
    base_hash: Option<&str>,
    result_hash: &str,
    ops: &str,
    source: Option<&str>,
) -> String {
    let base = base_hash.map_or_else(|| "null".to_owned(), q);
    let source_field = source.map_or_else(String::new, |source| {
        format!("      source: {}\n", q(source))
    });
    format!(
        "    revision {revision} {{\n      base_hash: {base}\n      result_hash: {}\n      ops: [{ops}]\n      reason: \"test\"\n{source_field}    }}\n",
        q(result_hash),
    )
}

#[test]
fn parser_round_trips_revisions_block_fixture() {
    let (projection, hash) = projection_and_hash(base_source());
    let op = format!(
        "{{ op: \"genesis_from_materialized_view\", source_hash: {}, byte_len: {}, line_count: 1 }}",
        q(&hash),
        projection.len()
    );
    let source = source_with_revisions(&revision_entry(1, None, &hash, &op, Some(&projection)));

    let diagnostics = analyze_source(&source);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error),
        "expected clean revisions block, got {diagnostics:#?}"
    );
}

#[test]
fn analyzer_rejects_chain_break() {
    let (projection, hash) = projection_and_hash(base_source());
    let bad_base = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let entries = format!(
        "{}{}",
        revision_entry(1, None, bad_base, "", None),
        revision_entry(
            2,
            Some("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            &hash,
            "",
            Some(&projection)
        ),
    );
    let diagnostics = analyze_source(&source_with_revisions(&entries));

    assert!(
        has_code(&diagnostics, codes::E_REV_BASE_HASH_BREAK),
        "{diagnostics:#?}"
    );
}

#[test]
fn analyzer_rejects_number_gap_duplicate_and_missing_genesis() {
    let (_, hash) = projection_and_hash(base_source());
    for entries in [
        format!(
            "{}{}",
            revision_entry(
                1,
                None,
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                "",
                None
            ),
            revision_entry(
                3,
                Some("sha256:1111111111111111111111111111111111111111111111111111111111111111"),
                &hash,
                "",
                None
            ),
        ),
        format!(
            "{}{}{}",
            revision_entry(
                1,
                None,
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                "",
                None
            ),
            revision_entry(
                2,
                Some("sha256:1111111111111111111111111111111111111111111111111111111111111111"),
                "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                "",
                None
            ),
            revision_entry(
                2,
                Some("sha256:2222222222222222222222222222222222222222222222222222222222222222"),
                &hash,
                "",
                None
            ),
        ),
        format!(
            "{}{}",
            revision_entry(
                2,
                Some("sha256:1111111111111111111111111111111111111111111111111111111111111111"),
                "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                "",
                None
            ),
            revision_entry(
                3,
                Some("sha256:2222222222222222222222222222222222222222222222222222222222222222"),
                &hash,
                "",
                None
            ),
        ),
    ] {
        let diagnostics = analyze_source(&source_with_revisions(&entries));
        assert!(
            has_code(&diagnostics, codes::E_REV_NUMBER_GAP),
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn analyzer_rejects_terminal_hash_mismatch() {
    let (_, hash) = projection_and_hash(base_source());
    let wrong_hash = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    assert_ne!(hash, wrong_hash);
    let source = source_with_revisions(&revision_entry(1, None, wrong_hash, "", None));
    let diagnostics = analyze_source(&source);

    assert!(
        has_code(&diagnostics, codes::E_REV_TERMINAL_MISMATCH),
        "{diagnostics:#?}"
    );
}

#[test]
fn analyzer_warns_when_revisions_block_exceeds_threshold() {
    let (_, projection_hash) = projection_and_hash(base_source());
    let mut entries = String::new();
    let mut previous: Option<String> = None;
    for n in 1..=201_u64 {
        let result = if n == 201 {
            projection_hash.clone()
        } else {
            format!("sha256:{n:064x}")
        };
        entries.push_str(&revision_entry(n, previous.as_deref(), &result, "", None));
        previous = Some(result);
    }
    let diagnostics = analyze_source(&source_with_revisions(&entries));

    assert!(
        has_code(&diagnostics, codes::W_REV_LONG_CHAIN),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != Severity::Error),
        "long valid chain should warn without failing: {diagnostics:#?}"
    );
}

#[test]
fn walks_reorder_is_rejected() {
    let (_, hash) = projection_and_hash(base_source());
    let op = r#"{ op: "reorder_items", block_path: [{ kind: "walks", name: "" }], new_order: ["2", "1"] }"#;
    let diagnostics = analyze_source(&source_with_revisions(&revision_entry(
        1, None, &hash, op, None,
    )));

    assert!(
        has_code(&diagnostics, codes::E_REV_REORDER_FORBIDDEN_ON_WALKS),
        "{diagnostics:#?}"
    );
}

#[test]
fn canonical_hash_helper_is_public_and_unique() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates_root = manifest.parent().expect("workspace crates dir");
    let revisions_rs = manifest.join("src/analysis/revisions.rs");
    let revisions_source = std::fs::read_to_string(&revisions_rs).expect("read revisions.rs");
    assert!(revisions_source.contains("pub fn source_sha256"));

    let mut sha_invocations = 0;
    for crate_dir in std::fs::read_dir(crates_root).expect("read crates dir") {
        let crate_dir = crate_dir.expect("crate entry").path();
        if !crate_dir.is_dir() {
            continue;
        }
        for path in collect_rs_files(&crate_dir.join("src")) {
            let source = std::fs::read_to_string(&path).expect("read rust source");
            if source.contains("Sha256::new(") {
                assert_eq!(
                    path, revisions_rs,
                    "Sha256::new found outside revisions.rs: {path:?}"
                );
                sha_invocations += source.matches("Sha256::new(").count();
            }
        }
    }
    assert_eq!(sha_invocations, 1);
}

fn collect_rs_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            files.extend(collect_rs_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files
}
