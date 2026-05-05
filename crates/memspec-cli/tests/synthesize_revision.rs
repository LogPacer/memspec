#![cfg(feature = "experimental-revisions")]

use std::process::Command;

use memspec_parser::analysis::revisions::source_sha256;

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

fn hash(source: &str) -> String {
    source_sha256(source)
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

fn write_initialized_fixture(
    before_without_revisions: &str,
    current_without_revisions: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, path) = write_fixture(before_without_revisions);
    let output = synthesize(&path);
    assert!(
        output.status.success(),
        "initial synth failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let initialized = std::fs::read_to_string(&path).expect("read initialized fixture");
    let revisions = extract_revisions_block(&initialized);
    std::fs::write(
        &path,
        with_revisions_block(current_without_revisions, &revisions),
    )
    .expect("write current fixture");
    (dir, path)
}

fn with_revisions_block(source_without_revisions: &str, revisions_block: &str) -> String {
    let insert_at = source_without_revisions
        .rfind('}')
        .expect("slice close brace");
    let mut out = String::new();
    out.push_str(&source_without_revisions[..insert_at]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(revisions_block);
    if !revisions_block.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&source_without_revisions[insert_at..]);
    out
}

fn extract_revisions_block(source: &str) -> String {
    let start = source.find("  revisions {").expect("revisions block");
    let end = matching_brace_end(source, start).expect("revisions closing brace");
    source[start..end].to_owned()
}

fn extract_revision_block(source: &str, revision: u64) -> String {
    let needle = format!("    revision {revision} {{");
    let start = source.find(&needle).expect("revision block");
    let end = matching_brace_end(source, start).expect("revision closing brace");
    source[start..end].to_owned()
}

fn append_to_revisions_block(source: &str, entry: &str) -> String {
    let start = source.find("  revisions {").expect("revisions block");
    let insert_at = matching_brace_start(source, start).expect("revisions closing brace");
    let mut out = String::new();
    out.push_str(&source[..insert_at]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(entry);
    out.push_str(&source[insert_at..]);
    out
}

fn matching_brace_end(source: &str, start: usize) -> Option<usize> {
    matching_brace_start(source, start).map(|idx| idx + 1)
}

fn matching_brace_start(source: &str, start: usize) -> Option<usize> {
    let mut depth = 0_i32;
    let mut saw_open = false;
    for (idx, ch) in source[start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                saw_open = true;
            }
            '}' if saw_open => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn last_result_hash(source: &str) -> String {
    let needle = "result_hash: \"";
    let start = source.rfind(needle).expect("result_hash") + needle.len();
    let end = source[start..].find('"').expect("result hash end") + start;
    source[start..end].to_owned()
}

fn empty_projection_for_slice(name: &str) -> String {
    format!("slice {name} {{\n}}\n")
}

fn line_count(source: &str) -> usize {
    if source.is_empty() {
        return 0;
    }
    source.bytes().filter(|b| *b == b'\n').count() + usize::from(!source.ends_with('\n'))
}

fn revision_one_empty_for_slice(name: &str) -> String {
    let empty = empty_projection_for_slice(name);
    let empty_hash = hash(&empty);
    format!(
        "  revisions {{\n    revision 1 {{\n      base_hash: null\n      result_hash: {}\n      ops: [\n        {{ op: \"genesis_from_materialized_view\", source_hash: {}, byte_len: {}, line_count: {} }},\n      ]\n      reason: \"fixture empty baseline\"\n    }}\n  }}\n",
        q(&empty_hash),
        q(&empty_hash),
        empty.len(),
        line_count(&empty),
    )
}

#[test]
fn synth_appends_revision_and_replay_round_trips_to_current_text() {
    let before = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let after = "slice s {\n  cell c { type: boolean mutable: true }\n  cell added { type: boolean mutable: true }\n}\n";
    let (_dir, path) = write_initialized_fixture(before, after);

    let output = synthesize(&path);
    assert!(
        output.status.success(),
        "synthesize failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let written = std::fs::read_to_string(&path).expect("read synthesized fixture");
    let appended = extract_revision_block(&written, 3);
    assert!(appended.contains(r#"op: "add_block""#), "{appended}");
    assert!(!appended.contains("source:"), "{appended}");

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
        let (_dir, path) = write_initialized_fixture(before, after);
        let output = synthesize(&path);
        assert!(
            output.status.success(),
            "{tag} synth failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let written = std::fs::read_to_string(&path).expect("read synthesized fixture");
        let appended = extract_revision_block(&written, 3);
        assert_eq!(
            appended.matches(&format!(r#"op: "{tag}""#)).count(),
            1,
            "{tag} not emitted exactly once in appended revision:\n{appended}"
        );
        let walk = run_memspec(&["walk", path.to_str().expect("utf8 path")]);
        assert!(walk.status.success(), "{tag} walk failed");
    }
}

#[test]
fn synth_no_op_comment_edit_returns_zero_without_appending() {
    let before = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let after = "slice s {\n  // comment-only edit\n  cell c { type: boolean mutable: true }\n}\n";
    let (_dir, path) = write_initialized_fixture(before, after);

    let output = synthesize(&path);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no semantic change"),
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let written = std::fs::read_to_string(&path).expect("read fixture");
    assert_eq!(written.matches("revision ").count(), 2);
}

#[test]
fn analyzer_rejects_invalid_replay_op_even_when_hashes_match() {
    let before = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let (_dir, path) = write_initialized_fixture(before, before);
    let initialized = std::fs::read_to_string(&path).expect("read initialized fixture");
    let last_hash = last_result_hash(&initialized);
    let bogus_revision = format!(
        "    revision 3 {{\n      base_hash: {}\n      result_hash: {}\n      ops: [{{ op: \"remove_block\", kind: \"cell\", name: \"missing\" }}]\n      reason: \"bogus replay target\"\n    }}\n",
        q(&last_hash),
        q(&last_hash),
    );
    std::fs::write(
        &path,
        append_to_revisions_block(&initialized, &bogus_revision),
    )
    .expect("write bogus revision");

    let walk = run_memspec(&["walk", path.to_str().expect("utf8 path"), "--json"]);
    assert!(
        !walk.status.success(),
        "bogus replay target should fail walk"
    );
    let stdout = String::from_utf8_lossy(&walk.stdout);
    assert!(stdout.contains("memspec/E0503"), "{stdout}");
    assert!(stdout.contains("remove_block target"), "{stdout}");
}

#[test]
fn synth_revision_two_bytes_match_golden_fixture() {
    let current = "slice s {\n  cell c { type: boolean mutable: true }\n}\n";
    let source = with_revisions_block(current, &revision_one_empty_for_slice("s"));
    let (_dir, path) = write_fixture(&source);

    let output = synthesize(&path);
    assert!(
        output.status.success(),
        "synthesize failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let written = std::fs::read_to_string(&path).expect("read synthesized fixture");
    let revision_two = extract_revision_block(&written, 2);
    let expected = include_str!("golden/synth_revision_2.snippet").trim_end();
    assert_eq!(revision_two, expected);
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
    let (_dir, path) = write_initialized_fixture(before, after);

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
