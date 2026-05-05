//! `memspec` CLI binary.
//!
//! v0-day-1 surface: `walk` exercises the lexer end-to-end and reports
//! token count + lexer diagnostics. Full structural / coherence /
//! symmetric-failure passes land as the parser and analyzer fill in.
//!
//! Exit codes (per `docs/grammar-v0.md`):
//!   0 — walk-complete (no diagnostics, all checks passed)
//!   1 — walk-incomplete (recoverable; diagnostics in JSON)
//!   2 — parse error (file syntactically invalid)
//!   3 — schema/coherence error (semantic check failed)
//!   4 — I/O error
//!
//! v0-day-1 only emits 0 / 2 / 4 (no parser, no analyzer yet).

#[cfg(feature = "experimental-revisions")]
use std::io::Write;
#[cfg(feature = "experimental-revisions")]
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use schemars::{JsonSchema, schema_for};

mod tui;
#[cfg(feature = "experimental-revisions")]
use memspec_parser::analysis::revisions;
use memspec_parser::{
    Diagnostic, Severity,
    analysis::{
        analyze_working_set, diff as d,
        loader::{FsLoader, load_with_imports},
        query as q, render, suggest as s,
    },
    ast::{BlockItem, File},
    parser,
    span::SourceMap,
};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "memspec", version, about = "Discipline-enforcing spec framework", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum RenderFormat {
    /// Markdown (narrative, paste-into-PR).
    Md,
    /// Mermaid graph (paste into any markdown that supports Mermaid).
    Graph,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the structural + coherence + symmetric-failure walk over a .memspec file.
    /// Follows `use "..."` imports by default; use --single-file to opt out.
    Walk {
        /// Path to a .memspec file.
        file: PathBuf,
        /// Emit machine-readable JSON diagnostics on stdout.
        #[arg(long)]
        json: bool,
        /// Don't follow `use "..."` imports — only walk this single file.
        #[arg(long)]
        single_file: bool,
    },
    /// Interactive TUI viewer — explore slices, slots, diagnostics in a lazygit-style layout.
    /// Pass a `.memspec` file OR a directory (discovers sibling .memspec files; navigate between
    /// them in the Slices pane). Defaults to the current directory. Read-only; reload with `r`.
    View {
        /// Path to a .memspec file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Propose the next missing slot/clause as a fill-in template. Replaces LSP completion.
    Suggest {
        /// Path to a .memspec file.
        file: PathBuf,
    },
    /// Emit JSON Schema for the CLI's `--json` output shapes (one $defs per report type).
    /// Lets agents version against the contract.
    Schema {
        /// Emit JSON Schema (the only supported format today).
        #[arg(long, default_value_t = true)]
        json_schema: bool,
    },
    /// Per-walk diff. Reports what was added/changed/killed/superseded in the range (from, to].
    Diff {
        /// Path to a .memspec file.
        file: PathBuf,
        /// Start of the diff range (exclusive). Use 0 to include the first walk.
        #[arg(long)]
        from: i64,
        /// End of the diff range (inclusive).
        #[arg(long)]
        to: i64,
    },
    /// Render a parsed slice in a human-readable format.
    Render {
        /// Path to a .memspec file.
        file: PathBuf,
        /// Output format. Default: markdown.
        #[arg(long, value_enum, default_value_t = RenderFormat::Md)]
        format: RenderFormat,
        /// Walk imports and render the entire working set as one document.
        #[arg(long)]
        aggregate: bool,
    },
    /// Inspect a parsed slice. JSON-only output, exits non-zero on parse failure.
    Query {
        /// Path to a .memspec file.
        file: PathBuf,
        /// List every declared ID, grouped by slot kind.
        #[arg(long, conflicts_with_all = ["by_id", "refs_to", "gaps"])]
        list_ids: bool,
        /// Show the full declaration JSON for a given ID.
        #[arg(long, conflicts_with_all = ["list_ids", "refs_to", "gaps"])]
        by_id: Option<String>,
        /// Show every site that references the given ID.
        #[arg(long, conflicts_with_all = ["list_ids", "by_id", "gaps"])]
        refs_to: Option<String>,
        /// Show structured gap analysis (unkilled forbidden states, missing post_failure, unused cells).
        #[arg(long, conflicts_with_all = ["list_ids", "by_id", "refs_to"])]
        gaps: bool,
    },
    /// Debug-only experiments. Hidden unless compiled with `experimental-revisions`.
    #[cfg(feature = "experimental-revisions")]
    Experimental {
        #[command(subcommand)]
        command: ExperimentalCommand,
    },
}

#[cfg(feature = "experimental-revisions")]
#[derive(Subcommand, Debug)]
enum ExperimentalCommand {
    /// Build a revision-1 genesis manifest from an existing .memspec file.
    Genesis {
        /// Path to a .memspec file.
        file: PathBuf,
        /// Human reason stored on the genesis revision.
        #[arg(long, default_value = "initial import")]
        reason: String,
        /// Optional author label for the revision manifest.
        #[arg(long)]
        author: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Append inline revisions for the semantic changes currently present in a .memspec file.
    SynthesizeRevision {
        /// Path to a .memspec file.
        file: PathBuf,
        /// Human reason stored on the appended revision.
        #[arg(long, default_value = "automated edit via watcher")]
        reason: String,
        /// Optional author label for the revision entry.
        #[arg(long)]
        author: Option<String>,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Walk {
            file,
            json,
            single_file,
        } => walk(&file, json, single_file),
        Command::Render {
            file,
            format,
            aggregate,
        } => render_cmd(&file, format, aggregate),
        Command::Diff { file, from, to } => diff_cmd(&file, from, to),
        Command::Suggest { file } => suggest_cmd(&file),
        Command::View { path } => match tui::run(path) {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("memspec view: {e}");
                ExitCode::from(4)
            }
        },
        Command::Schema { json_schema: _ } => schema_cmd(),
        Command::Query {
            file,
            list_ids,
            by_id,
            refs_to,
            gaps,
        } => query(&file, list_ids, by_id, refs_to, gaps),
        #[cfg(feature = "experimental-revisions")]
        Command::Experimental { command } => experimental(command),
    }
}

fn schema_cmd() -> ExitCode {
    // Compose one document with `$defs` for every report type the CLI emits.
    // Agents can `--json` against any of these and version against the schema.
    let mut combined = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "memspec CLI JSON output schemas",
        "description": "Combined schemas for memspec walk/query/render/diff/suggest --json output. Agents version against this contract; codes never get reused.",
        "$defs": serde_json::Map::new(),
    });

    let defs = combined
        .get_mut("$defs")
        .and_then(|v| v.as_object_mut())
        .expect("defs object");

    fn add<T: JsonSchema>(name: &str, defs: &mut serde_json::Map<String, serde_json::Value>) {
        let s = schema_for!(T);
        if let Ok(v) = serde_json::to_value(&s) {
            defs.insert(name.to_owned(), v);
        }
    }

    // Walk reports
    add::<JsonReport>("walk_single", defs);
    add::<JsonMultiReport>("walk_multi", defs);
    // Query reports
    add::<memspec_parser::analysis::query::ListIdsReport>("query_list_ids", defs);
    add::<memspec_parser::analysis::query::RefsReport>("query_refs_to", defs);
    add::<memspec_parser::analysis::query::GapsReport>("query_gaps", defs);
    // Diff
    add::<memspec_parser::analysis::diff::DiffReport>("diff", defs);
    // Suggest
    add::<memspec_parser::analysis::suggest::SuggestReport>("suggest", defs);
    #[cfg(feature = "experimental-revisions")]
    add::<memspec_parser::analysis::revisions::GenesisRevisionReport>(
        "experimental_genesis_revision",
        defs,
    );
    #[cfg(feature = "experimental-revisions")]
    add::<memspec_parser::analysis::revisions::RevisionAppendReport>(
        "experimental_revision_append",
        defs,
    );
    // Common types referenced by everything
    add::<memspec_parser::Diagnostic>("diagnostic", defs);
    add::<memspec_parser::Severity>("severity", defs);
    add::<memspec_parser::Span>("span", defs);

    match serde_json::to_string_pretty(&combined) {
        Ok(s) => {
            println!("{s}");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("memspec: failed to emit schema: {e}");
            ExitCode::from(4)
        }
    }
}

#[cfg(feature = "experimental-revisions")]
fn experimental(command: ExperimentalCommand) -> ExitCode {
    match command {
        ExperimentalCommand::Genesis {
            file,
            reason,
            author,
            json,
        } => genesis_cmd(&file, reason, author, json),
        ExperimentalCommand::SynthesizeRevision {
            file,
            reason,
            author,
            json,
        } => synthesize_revision_cmd(&file, reason, author, json),
    }
}

#[cfg(feature = "experimental-revisions")]
fn genesis_cmd(path: &PathBuf, reason: String, author: Option<String>, json: bool) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };

    let parse_result = parser::parse(&source);
    let file = parse_result.file;
    let analysis = memspec_parser::analyze(&file);
    let mut diagnostics = parse_result.diagnostics;
    diagnostics.extend(analysis.diagnostics);
    if let Some(d) = maybe_wrong_file_type(&diagnostics, &source, path) {
        diagnostics = vec![d];
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        eprintln!(
            "memspec experimental genesis: {} has diagnostics; run `memspec walk {} --json` first",
            path.display(),
            path.display(),
        );
        return exit_for_diagnostics(&diagnostics);
    }

    let report = revisions::build_genesis_revision(
        &file,
        &source,
        Some(path.display().to_string()),
        reason,
        author,
    );

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("memspec: failed to serialize genesis revision: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        println!("memspec experimental genesis: {}", path.display());
        println!(
            "  slice:                {}",
            report.slice.as_deref().unwrap_or("<none>")
        );
        println!(
            "  revision_number:      {}",
            report.revision.revision_number
        );
        println!("  result_hash:          {}", report.revision.result_hash);
        println!(
            "  patch_format_version: {}",
            report.revision.patch_format_version
        );
        println!("  ops:                  {}", report.revision.ops.len());
    }

    ExitCode::from(0)
}

#[cfg(feature = "experimental-revisions")]
fn synthesize_revision_cmd(
    path: &PathBuf,
    reason: String,
    author: Option<String>,
    json: bool,
) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };

    let synthesis = match revisions::synthesize_revision_source(
        &source,
        Some(path.display().to_string()),
        reason,
        author,
    ) {
        Ok(synthesis) => synthesis,
        Err(err) => {
            eprintln!("memspec experimental synthesize-revision: {}", err.message);
            print_revision_diagnostics(&source, &err.diagnostics);
            return exit_for_diagnostics(&err.diagnostics);
        }
    };

    if synthesis.report.no_op {
        if json {
            match serde_json::to_string_pretty(&synthesis.report) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("memspec: failed to serialize revision append report: {e}");
                    return ExitCode::from(4);
                }
            }
        } else {
            println!("{}", synthesis.report.message);
        }
        return ExitCode::from(0);
    }

    #[cfg(debug_assertions)]
    let mut new_source = synthesis.new_source;
    #[cfg(not(debug_assertions))]
    let new_source = synthesis.new_source;
    #[cfg(debug_assertions)]
    if std::env::var_os("MEMSPEC_SYNTH_REPLAY_CHECK_CORRUPT_FOR_TEST").is_some() {
        corrupt_last_result_hash_for_test(&mut new_source);
    }

    if let Err(e) = atomic_write(path, new_source.as_bytes()) {
        eprintln!("memspec: cannot atomically write {}: {e}", path.display());
        return ExitCode::from(4);
    }

    let written = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {} after write: {e}", path.display());
            return ExitCode::from(4);
        }
    };
    let parse_result = parser::parse(&written);
    let analysis = memspec_parser::analyze(&parse_result.file);
    let mut diagnostics = parse_result.diagnostics;
    diagnostics.extend(analysis.diagnostics);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        eprintln!(
            "memspec experimental synthesize-revision: freshly written file failed replay check"
        );
        print_revision_diagnostics(&written, &diagnostics);
        return exit_for_diagnostics(&diagnostics);
    }

    if json {
        match serde_json::to_string_pretty(&synthesis.report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("memspec: failed to serialize revision append report: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        let revision_number = synthesis
            .report
            .revisions
            .last()
            .map(|revision| revision.revision_number)
            .unwrap_or(0);
        println!("appended revision {revision_number} to {}", path.display());
    }

    ExitCode::from(0)
}

#[cfg(feature = "experimental-revisions")]
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    let dir = std::fs::File::open(parent)?;
    dir.sync_all()?;
    Ok(())
}

#[cfg(feature = "experimental-revisions")]
fn print_revision_diagnostics(source: &str, diagnostics: &[Diagnostic]) {
    let map = SourceMap::new(source);
    for diagnostic in diagnostics {
        let lc = map.line_col(diagnostic.span.start.min(source.len()));
        eprintln!(
            "  [{}] {}:{} {}: {}",
            diagnostic.code,
            lc.line,
            lc.col,
            diagnostic.severity.as_str(),
            diagnostic.message
        );
        if let Some(hint) = &diagnostic.hint {
            eprintln!("        hint: {hint}");
        }
    }
}

#[cfg(all(feature = "experimental-revisions", debug_assertions))]
fn corrupt_last_result_hash_for_test(source: &mut String) {
    let needle = "result_hash: \"sha256:";
    let Some(offset) = source.rfind(needle) else {
        return;
    };
    let pos = offset + needle.len();
    if pos >= source.len() {
        return;
    }
    let replacement = if source.as_bytes()[pos] == b'0' {
        "1"
    } else {
        "0"
    };
    source.replace_range(pos..pos + 1, replacement);
}

fn suggest_cmd(path: &PathBuf) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };
    let report = s::suggest_from_source(&source);
    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            println!("{json}");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("memspec: failed to serialize suggestion: {e}");
            ExitCode::from(4)
        }
    }
}

fn diff_cmd(path: &PathBuf, from: i64, to: i64) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };
    let pr = parser::parse(&source);
    if pr.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        eprintln!(
            "memspec: cannot diff — file has parse errors. Run `memspec walk {} --json` first.",
            path.display(),
        );
        return ExitCode::from(2);
    }
    let report = match d::diff(&pr.file, from, to) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("memspec: {e}");
            return ExitCode::from(1);
        }
    };
    match serde_json::to_string_pretty(&report) {
        Ok(s) => {
            println!("{s}");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("memspec: failed to serialize diff: {e}");
            ExitCode::from(4)
        }
    }
}

fn render_cmd(path: &PathBuf, format: RenderFormat, aggregate: bool) -> ExitCode {
    if aggregate {
        return render_aggregate(path, format);
    }
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };
    let pr = parser::parse(&source);
    if pr.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        eprintln!(
            "memspec: cannot render — file has parse errors. Run `memspec walk {} --json` to see them.",
            path.display(),
        );
        return ExitCode::from(2);
    }
    let out = match format {
        RenderFormat::Md => render::render_markdown(&pr.file),
        RenderFormat::Graph => render::render_mermaid(&pr.file),
    };
    print!("{out}");
    ExitCode::from(0)
}

fn render_aggregate(path: &PathBuf, format: RenderFormat) -> ExitCode {
    let loader = FsLoader;
    let ws = load_with_imports(&loader, path);

    // If any file failed to parse, surface and bail (same policy as render).
    let any_parse_err = ws
        .files
        .iter()
        .any(|lf| lf.diagnostics.iter().any(|d| d.severity == Severity::Error));
    if any_parse_err {
        eprintln!(
            "memspec: cannot render aggregate — one or more files have parse errors. Run `memspec walk {} --json` to see them.",
            path.display(),
        );
        return ExitCode::from(2);
    }

    let out = match format {
        RenderFormat::Md => render::render_markdown_aggregate(&ws),
        RenderFormat::Graph => render::render_mermaid_aggregate(&ws),
    };
    print!("{out}");
    ExitCode::from(0)
}

fn query(
    path: &PathBuf,
    list_ids: bool,
    by_id: Option<String>,
    refs_to: Option<String>,
    gaps: bool,
) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };
    let pr = parser::parse(&source);
    if pr.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        eprintln!(
            "memspec: cannot query — file has parse errors. Run `memspec walk {} --json` to see them.",
            path.display(),
        );
        return ExitCode::from(2);
    }

    let json = if list_ids {
        serde_json::to_value(q::list_ids(&pr.file))
    } else if let Some(id) = by_id.as_deref() {
        match q::by_id(&pr.file, id) {
            Some(report) => serde_json::to_value(report),
            None => {
                eprintln!("memspec: no declaration named `{id}` in {}", path.display());
                return ExitCode::from(1);
            }
        }
    } else if let Some(id) = refs_to.as_deref() {
        serde_json::to_value(q::refs_to(&pr.file, id))
    } else if gaps {
        serde_json::to_value(q::gaps(&pr.file))
    } else {
        eprintln!("memspec query: pick one of --list-ids | --by-id <id> | --refs-to <id> | --gaps");
        return ExitCode::from(1);
    };

    match json {
        Ok(v) => match serde_json::to_string_pretty(&v) {
            Ok(s) => {
                println!("{s}");
                ExitCode::from(0)
            }
            Err(e) => {
                eprintln!("memspec: failed to serialize report: {e}");
                ExitCode::from(4)
            }
        },
        Err(e) => {
            eprintln!("memspec: failed to build report: {e}");
            ExitCode::from(4)
        }
    }
}

fn walk(path: &PathBuf, json: bool, single_file: bool) -> ExitCode {
    if single_file {
        return walk_single(path, json);
    }
    walk_multi(path, json)
}

fn walk_single(path: &PathBuf, json: bool) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memspec: cannot read {}: {e}", path.display());
            return ExitCode::from(4);
        }
    };

    let parse_result = parser::parse(&source);
    let analysis = memspec_parser::analyze(&parse_result.file);
    let mut diagnostics = parse_result.diagnostics;
    diagnostics.extend(analysis.diagnostics);
    if let Some(d) = maybe_wrong_file_type(&diagnostics, &source, path) {
        diagnostics = vec![d];
    }
    let map = SourceMap::new(&source);

    if json {
        let report = JsonReport::build(
            path.display().to_string(),
            &parse_result.file,
            &diagnostics,
            &map,
        );
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("memspec: failed to serialize report: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        print_text_report(path, &parse_result.file, &diagnostics, &map);
    }

    exit_for_diagnostics(&diagnostics)
}

fn walk_multi(path: &PathBuf, json: bool) -> ExitCode {
    let loader = FsLoader;
    let ws = load_with_imports(&loader, path);
    let mut analysis = analyze_working_set(&ws);

    // For each file in the WS, swap in the friendly diagnostic if the file
    // is obviously not a .memspec (markdown, etc.). Keeps honest reporting
    // when the file IS a real .memspec with bad content somewhere.
    for lf in &ws.files {
        if let Some(diags) = analysis.by_file.get_mut(&lf.path) {
            if let Some(d) = maybe_wrong_file_type(diags, &lf.source, &lf.path) {
                *diags = vec![d];
            }
        }
    }

    if json {
        let report = JsonMultiReport::build(&ws, &analysis);
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("memspec: failed to serialize report: {e}");
                return ExitCode::from(4);
            }
        }
    } else {
        print_multi_text_report(&ws, &analysis);
    }

    // Aggregate diagnostics for exit-code decision.
    let mut all: Vec<&Diagnostic> = Vec::new();
    for diags in analysis.by_file.values() {
        all.extend(diags.iter());
    }
    exit_for_diagnostics_refs(&all)
}

/// Heuristic: when a file produces a flood of E0003 unexpected-character
/// errors AND contains no `slice` keyword, it's almost certainly not a
/// `.memspec` file (most often: markdown). Replace the diagnostic flood
/// with one friendly E0006 so the user sees the actual problem instead of
/// hundreds of "unexpected `#`" lines.
///
/// Honest reporting is preserved when the file IS a real `.memspec` with
/// bad content somewhere — that produces a few E0003s, not hundreds.
fn maybe_wrong_file_type(
    diagnostics: &[Diagnostic],
    source: &str,
    path: &PathBuf,
) -> Option<Diagnostic> {
    let lex_count = diagnostics
        .iter()
        .filter(|d| d.code == "memspec/E0003")
        .count();
    if lex_count < 10 {
        return None;
    }
    // If the source contains a `slice` declaration, it's a real (or
    // partially-typed) .memspec; emit honest diagnostics.
    let looks_like_memspec = source.contains("\nslice ")
        || source.starts_with("slice ")
        || source.contains("\nuse \"")
        || source.starts_with("use \"");
    if looks_like_memspec {
        return None;
    }

    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let extension_note = if extension == "md" {
        " (file is `.md` — looks like markdown)"
    } else if extension == "memspec" {
        ""
    } else if extension.is_empty() {
        " (file has no extension)"
    } else {
        " (file extension is not `.memspec`)"
    };

    let span_end = source.len().min(80);
    Some(
        Diagnostic::error(
            "memspec/E0006",
            memspec_parser::Span::new(0, span_end),
            format!(
                "this doesn't look like a .memspec file{extension_note}: found {lex_count} unexpected-character errors and no `slice` declaration"
            ),
        )
        .with_hint(
            "memspec walk expects a .memspec file (the DSL). See `docs/grammar-v0.md` for the format, or run `memspec render <file.memspec> --format md` to see what one looks like rendered.",
        ),
    )
}

fn exit_for_diagnostics(diagnostics: &[Diagnostic]) -> ExitCode {
    let refs: Vec<&Diagnostic> = diagnostics.iter().collect();
    exit_for_diagnostics_refs(&refs)
}

fn exit_for_diagnostics_refs(diagnostics: &[&Diagnostic]) -> ExitCode {
    // Numeric ranges: E0001–E0099 lex, E0100–E0199 parse, E0200+ semantic.
    // Both lex and parse → exit 2 (parse-class). Semantic → exit 3.
    let mut has_parse_class = false;
    let mut has_semantic = false;
    for d in diagnostics {
        if d.severity != Severity::Error {
            continue;
        }
        let n = d
            .code
            .strip_prefix("memspec/E")
            .and_then(|s| s.parse::<u32>().ok());
        match n {
            Some(n) if n < 200 => has_parse_class = true,
            Some(_) => has_semantic = true,
            None => has_semantic = true, // unrecognised code → fail safely
        }
    }
    if has_parse_class {
        ExitCode::from(2)
    } else if has_semantic {
        ExitCode::from(3)
    } else {
        ExitCode::from(0)
    }
}

fn print_multi_text_report(
    ws: &memspec_parser::analysis::loader::WorkingSet,
    analysis: &memspec_parser::analysis::WorkingSetAnalysis,
) {
    let n = ws.files.len();
    println!(
        "memspec walk: {n} file(s) loaded (root: {})",
        ws.root.display()
    );

    for lf in &ws.files {
        let diagnostics = analysis
            .by_file
            .get(&lf.path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let counts = SliceCounts::from_file(&lf.file);
        let errors = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warnings = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        let infos = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .count();
        let map = SourceMap::new(&lf.source);

        println!();
        println!("── {} ──", lf.path.display());
        if let Some(name) = counts.slice_name.as_deref() {
            println!("  slice:                {name}");
        }
        println!("  cells:                {}", counts.cells);
        println!("  derived:              {}", counts.derived);
        println!("  associations:         {}", counts.associations);
        println!(
            "  events:               {} (steps: {})",
            counts.events, counts.steps
        );
        println!("  post_failure rows:    {}", counts.post_failure);
        println!("  forbidden_states:     {}", counts.forbidden_states);
        println!("  kill_tests:           {}", counts.kill_tests);
        println!("  walks declared:       {}", counts.walks);
        println!("  diagnostics:          {errors} error(s), {warnings} warning(s), {infos} info");

        let status = if errors > 0 {
            "walk-incomplete"
        } else if warnings > 0 {
            "walk-complete (with warnings)"
        } else {
            "walk-complete"
        };
        println!("  status:               {status}");

        if !diagnostics.is_empty() {
            for d in diagnostics {
                let lc = map.line_col(d.span.start);
                println!(
                    "  [{}] {}:{}:{}  {}: {}",
                    d.code,
                    lf.path.display(),
                    lc.line,
                    lc.col,
                    d.severity.as_str(),
                    d.message,
                );
                if let Some(hint) = &d.hint {
                    println!("        hint: {hint}");
                }
            }
        }
    }
}

fn print_text_report(path: &PathBuf, file: &File, diagnostics: &[Diagnostic], map: &SourceMap<'_>) {
    let counts = SliceCounts::from_file(file);
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let infos = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .count();

    println!("memspec walk: {}", path.display());
    if let Some(slice_name) = counts.slice_name.as_deref() {
        println!("  slice:                {slice_name}");
    } else {
        println!("  slice:                <none>");
    }
    println!("  cells:                {}", counts.cells);
    println!("  derived:              {}", counts.derived);
    println!("  associations:         {}", counts.associations);
    println!(
        "  events:               {} (steps: {})",
        counts.events, counts.steps
    );
    println!("  post_failure rows:    {}", counts.post_failure);
    println!("  forbidden_states:     {}", counts.forbidden_states);
    println!("  kill_tests:           {}", counts.kill_tests);
    println!("  walks declared:       {}", counts.walks);
    println!("  diagnostics:          {errors} error(s), {warnings} warning(s), {infos} info");

    let status = if errors > 0 {
        "walk-incomplete"
    } else if warnings > 0 {
        "walk-complete (with warnings)"
    } else {
        "walk-complete"
    };
    println!("  status:               {status}");

    if !diagnostics.is_empty() {
        println!();
        for d in diagnostics {
            let lc = map.line_col(d.span.start);
            println!(
                "  [{}] {}:{}:{}  {}: {}",
                d.code,
                path.display(),
                lc.line,
                lc.col,
                d.severity.as_str(),
                d.message,
            );
            if let Some(hint) = &d.hint {
                println!("        hint: {hint}");
            }
        }
    }
}

#[derive(Default, Serialize, JsonSchema)]
struct SliceCounts {
    slice_name: Option<String>,
    cells: usize,
    derived: usize,
    associations: usize,
    events: usize,
    steps: usize,
    post_failure: usize,
    forbidden_states: usize,
    kill_tests: usize,
    walks: usize,
}

impl SliceCounts {
    fn from_file(file: &File) -> Self {
        let mut counts = Self::default();
        let Some(slice) = &file.slice else {
            return counts;
        };
        counts.slice_name = Some(slice.name.name.clone());
        for item in &slice.items {
            let BlockItem::Block(b) = item else { continue };
            match b.kind.name.as_str() {
                "cell" => counts.cells += 1,
                "derived" => counts.derived += 1,
                "association" => counts.associations += 1,
                "event" => {
                    counts.events += 1;
                    counts.steps += b
                        .items
                        .iter()
                        .filter(|i| matches!(i, BlockItem::Block(b) if b.kind.name == "step"))
                        .count();
                }
                "post_failure" => counts.post_failure += 1,
                "forbidden_state" => counts.forbidden_states += 1,
                "kill_test" => counts.kill_tests += 1,
                "walk" => counts.walks += 1,
                _ => {}
            }
        }
        counts
    }
}

#[derive(Serialize, JsonSchema)]
struct JsonReport {
    file: String,
    slice: SliceCounts,
    diagnostics: Vec<JsonDiagnostic>,
    summary: JsonSummary,
}

#[derive(Serialize, JsonSchema)]
struct JsonDiagnostic {
    code: &'static str,
    severity: &'static str,
    message: String,
    location: JsonLocation,
    hint: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct JsonLocation {
    line: u32,
    col: u32,
    byte_start: usize,
    byte_end: usize,
}

#[derive(Serialize, JsonSchema)]
struct JsonSummary {
    errors: usize,
    warnings: usize,
    info: usize,
}

#[derive(Serialize, JsonSchema)]
struct JsonMultiReport {
    root: String,
    files: Vec<JsonMultiFileEntry>,
    summary: JsonSummary,
}

#[derive(Serialize, JsonSchema)]
struct JsonMultiFileEntry {
    path: String,
    slice: SliceCounts,
    diagnostics: Vec<JsonDiagnostic>,
}

impl JsonMultiReport {
    fn build(
        ws: &memspec_parser::analysis::loader::WorkingSet,
        analysis: &memspec_parser::analysis::WorkingSetAnalysis,
    ) -> Self {
        let mut errors = 0;
        let mut warnings = 0;
        let mut info = 0;
        let mut entries = Vec::new();
        for lf in &ws.files {
            let diagnostics = analysis
                .by_file
                .get(&lf.path)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let map = SourceMap::new(&lf.source);
            let json_diags: Vec<_> = diagnostics
                .iter()
                .map(|d| {
                    match d.severity {
                        Severity::Error => errors += 1,
                        Severity::Warning => warnings += 1,
                        Severity::Info => info += 1,
                    }
                    let lc = map.line_col(d.span.start);
                    JsonDiagnostic {
                        code: d.code,
                        severity: d.severity.as_str(),
                        message: d.message.clone(),
                        location: JsonLocation {
                            line: lc.line,
                            col: lc.col,
                            byte_start: d.span.start,
                            byte_end: d.span.end,
                        },
                        hint: d.hint.clone(),
                    }
                })
                .collect();
            entries.push(JsonMultiFileEntry {
                path: lf.path.display().to_string(),
                slice: SliceCounts::from_file(&lf.file),
                diagnostics: json_diags,
            });
        }
        Self {
            root: ws.root.display().to_string(),
            files: entries,
            summary: JsonSummary {
                errors,
                warnings,
                info,
            },
        }
    }
}

impl JsonReport {
    fn build(file: String, ast: &File, diagnostics: &[Diagnostic], map: &SourceMap<'_>) -> Self {
        let mut errors = 0;
        let mut warnings = 0;
        let mut info = 0;
        let json_diags: Vec<_> = diagnostics
            .iter()
            .map(|d| {
                match d.severity {
                    Severity::Error => errors += 1,
                    Severity::Warning => warnings += 1,
                    Severity::Info => info += 1,
                }
                let lc = map.line_col(d.span.start);
                JsonDiagnostic {
                    code: d.code,
                    severity: d.severity.as_str(),
                    message: d.message.clone(),
                    location: JsonLocation {
                        line: lc.line,
                        col: lc.col,
                        byte_start: d.span.start,
                        byte_end: d.span.end,
                    },
                    hint: d.hint.clone(),
                }
            })
            .collect();
        Self {
            file,
            slice: SliceCounts::from_file(ast),
            diagnostics: json_diags,
            summary: JsonSummary {
                errors,
                warnings,
                info,
            },
        }
    }
}
