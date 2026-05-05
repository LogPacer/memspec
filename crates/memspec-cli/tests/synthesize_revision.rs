#![cfg(feature = "experimental-revisions")]

use std::process::Command;

use memspec_parser::{
    analysis::revisions::{canonical_source_projection, source_sha256},
    parser,
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_memspec")
}

fn run_memspec(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("run memspec")
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

fn canonical(source: &str) -> String {
    canonical_source_projection(&parser::parse(source).file)
}

fn hash(source: &str) -> String {
    source_sha256(source)
}

fn revision_one_for(before_without_revisions: &str) -> String {
    let projection = canonical(before_without_revisions);
    let result_hash = hash(&projection);
    format!(
        "  revisions {{\n    revision 1 {{\n      base_hash: null\n      result_hash: {}\n      ops: [{{ op: \"genesis_from_materialized_view\", source_hash: {}, byte_len: {}, line_count: 1 }}]\n      reason: \"fixture baseline\"\n      source: {}\n    }}\n  }}\n",
        q(&result_hash),
        q(&result_hash),
        projection.len(),
        q(&projection),
    )
}

fn with_revision_one(current_without_revisions: &str, before_without_revisions: &str) -> String {
    let insert_at = current_without_revisions
        .rfind('}')
        .expect("slice close brace");
    let mut out = String::new();
    out.push_str(&current_without_revisions[..insert_at]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&revision_one_for(before_without_revisions));
    out.push_str(&current_without_revisions[insert_at..]);
    out
}

fn write_fixture(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fixture.memspec");
    std::fs::write(&path, source).expect("write fixture");
    (dir, path)
}

fn synthesize(path: &std::path::Path) -> std::process::Output {
    run_memspec(&[
        "experimental",
        "synthesize-revision",
        path.to_str().expect("utf8 path"),
        "--reason",
        "test edit",
    ])
}

#[test]
fn synth_appends_revision_and_replay_round_trips_to_current_text() {
    let before = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let after = "slice s {\n  cell c { type: boolean mutable: true }\n  cell added { type: boolean mutable: true }\n}\n";
    let (_dir, path) = write_fixture(&with_revision_one(after, before));

    let output = synthesize(&path);
    assert!(
        output.status.success(),
        "synthesize failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let written = std::fs::read_to_string(&path).expect("read synthesized fixture");
    assert!(written.contains("revision 2"));
    assert!(written.contains(r#"op: "add_block""#));

    let walk = run_memspec(&["walk", path.to_str().expect("utf8 path")]);
    assert!(
        walk.status.success(),
        "walk failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&walk.stdout),
        String::from_utf8_lossy(&walk.stderr)
    );

    let second = synthesize(&path);
    assert!(second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("no semantic change"),
        "stdout:\n{}",
        String::from_utf8_lossy(&second.stdout)
    );
    let after_noop = std::fs::read_to_string(&path).expect("read no-op fixture");
    assert_eq!(
        written.matches("revision ").count(),
        after_noop.matches("revision ").count()
    );
}

#[test]
fn synth_emits_each_new_op_variant() {
    let cases = [
        (
            "add_block",
            "slice s {\n  cell c { type: boolean mutable: true }\n}\n",
            "slice s {\n  cell c { type: boolean mutable: true }\n  cell added { type: boolean mutable: true }\n}\n",
        ),
        (
            "remove_block",
            "slice s {\n  cell c { type: boolean mutable: true }\n  cell removed { type: boolean mutable: true }\n}\n",
            "slice s {\n  cell c { type: boolean mutable: true }\n}\n",
        ),
        (
            "modify_field",
            "slice s {\n  cell c { type: boolean mutable: true default: false }\n}\n",
            "slice s {\n  cell c { type: boolean mutable: true default: true }\n}\n",
        ),
        (
            "remove_field",
            "slice s {\n  cell c { type: boolean mutable: true default: false }\n}\n",
            "slice s {\n  cell c { type: boolean mutable: true }\n}\n",
        ),
        (
            "reorder_items",
            "slice s {\n  cell c { type: boolean mutable: true }\n  event e { mutates: [c] step s1 { op: \"one\" fallible: false mutates: [c] } step s2 { op: \"two\" fallible: false mutates: [c] } }\n}\n",
            "slice s {\n  cell c { type: boolean mutable: true }\n  event e { mutates: [c] step s2 { op: \"two\" fallible: false mutates: [c] } step s1 { op: \"one\" fallible: false mutates: [c] } }\n}\n",
        ),
    ];

    for (tag, before, after) in cases {
        let (_dir, path) = write_fixture(&with_revision_one(after, before));
        let output = synthesize(&path);
        assert!(
            output.status.success(),
            "{tag} synth failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let written = std::fs::read_to_string(&path).expect("read synthesized fixture");
        assert_eq!(
            written.matches(&format!(r#"op: "{tag}""#)).count(),
            1,
            "{tag} not emitted exactly once:\n{written}"
        );
        let walk = run_memspec(&["walk", path.to_str().expect("utf8 path")]);
        assert!(walk.status.success(), "{tag} walk failed");
    }
}

#[test]
fn synth_no_op_comment_edit_returns_zero_without_appending() {
    let before = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let after = "slice s {\n  // comment-only edit\n  cell c { type: boolean mutable: true }\n}\n";
    let (_dir, path) = write_fixture(&with_revision_one(after, before));

    let output = synthesize(&path);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no semantic change"),
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let written = std::fs::read_to_string(&path).expect("read fixture");
    assert_eq!(written.matches("revision ").count(), 1);
}

#[test]
fn synth_writes_atomically_via_tempfile_rename() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("tempfile::NamedTempFile::new_in"));
    assert!(source.contains("tmp.persist(path)"));
    assert!(source.contains("tmp.as_file_mut().sync_all()"));
    assert!(source.contains("dir.sync_all()"));
}

#[test]
fn experimental_synthesize_revision_help_exposes_json_flag() {
    let output = run_memspec(&["experimental", "synthesize-revision", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--json"), "{stdout}");
}

#[test]
fn synth_replay_check_failure_marks_invalid() {
    let before = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let after = "slice s {\n  cell c { type: boolean mutable: true }\n  cell added { type: boolean mutable: true }\n}\n";
    let (_dir, path) = write_fixture(&with_revision_one(after, before));

    let output = Command::new(bin())
        .args([
            "experimental",
            "synthesize-revision",
            path.to_str().expect("utf8 path"),
            "--reason",
            "test edit",
        ])
        .env("MEMSPEC_SYNTH_REPLAY_CHECK_CORRUPT_FOR_TEST", "1")
        .output()
        .expect("run corrupting synth");
    assert!(!output.status.success(), "corrupting synth should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("freshly written file failed replay check"),
        "{stderr}"
    );
    assert!(stderr.contains("terminal revision hash"), "{stderr}");

    let walk = run_memspec(&["walk", path.to_str().expect("utf8 path"), "--json"]);
    assert!(!walk.status.success(), "broken file should fail walk");
    let stdout = String::from_utf8_lossy(&walk.stdout);
    assert!(stdout.contains("memspec/E0502"), "{stdout}");
}
