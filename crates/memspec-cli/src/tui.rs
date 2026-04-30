//! Terminal UI — interactive viewer for `.memspec` working sets.
//!
//! Layout matches the lazygit / lazydocker / portman convention:
//!   ┌─────────────────────────────┬────────────────────────────────┐
//!   │ [1] Slices (working set)    │ [ Detail ]                     │
//!   │                             │   Block AST for the selected   │
//!   ├─────────────────────────────┤   ID — fields, refs, types.    │
//!   │ [2] Slot tree               │                                │
//!   │   cells / derived /         │                                │
//!   │   events / forbidden / ...  │                                │
//!   ├─────────────────────────────┼────────────────────────────────┤
//!   │ [3] Diagnostics + gaps      │ [ References ]                 │
//!   │                             │   refs-to <selected ID>        │
//!   └─────────────────────────────┴────────────────────────────────┘
//!   [ 1/2/3 focus  j/k move  Tab cycle  l/h col  r reload  q quit ]
//!
//! Read-only. Calls the parser/analyzer in-process; never mutates files.
//! When the user wants to edit, they alt-tab to their editor and hit `r`.

use std::io::{Result as IoResult, Stdout, stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use memspec_parser::Severity;
use memspec_parser::analysis::loader::{FsLoader, WorkingSet, load_with_imports};
use memspec_parser::analysis::{WorkingSetAnalysis, analyze_working_set, query as q};
use memspec_parser::ast::{BlockDecl, BlockItem, BlockName, FieldValue, MapEntry};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(path: PathBuf) -> IoResult<()> {
    let mut terminal = enter_tui()?;
    let result = run_loop(&mut terminal, path);
    let _ = leave_tui(&mut terminal);
    result
}

fn enter_tui() -> IoResult<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn leave_tui(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> IoResult<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    path: PathBuf,
) -> IoResult<()> {
    let mut state = State::new(path);
    state.reload();

    loop {
        terminal.draw(|f| view(f, &state))?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Release {
                    continue;
                }
                if handle_key(k, &mut state) == Outcome::Quit {
                    break;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Slices,
    Slots,
    Diagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Continue,
    Quit,
}

struct State {
    /// Original invocation target (file or directory). Retained for the
    /// status bar / future "Restart at invocation" affordance.
    #[allow(dead_code)]
    invocation: PathBuf,
    /// Directory we discover sibling `.memspec` files from. Retained for
    /// future watch mode (notify-rs subscribes to this dir).
    #[allow(dead_code)]
    discovery_dir: PathBuf,
    /// Discovered `.memspec` files in `discovery_dir`, sorted alphabetically.
    /// Pane 1 lists these; selecting one switches `active_root` + reloads.
    discovered: Vec<PathBuf>,
    /// Currently-active root — its working set is what panes 2/3 display.
    active_root: PathBuf,

    ws: Option<WorkingSet>,
    analysis: Option<WorkingSetAnalysis>,
    load_error: Option<String>,

    focused: Pane,
    slices_state: ListState,
    slots_state: ListState,
    diags_state: ListState,

    /// Cached flat lists derived from ws + analysis. Rebuilt on reload.
    slot_rows: Vec<SlotRow>,
    diag_rows: Vec<DiagRow>,
}

#[derive(Clone, Debug)]
struct SlotRow {
    indent: usize,
    label: String,
    /// Selecting this row in the right pane shows… what? Cell/event/etc id.
    target: Option<SlotTarget>,
}

#[derive(Clone, Debug)]
enum SlotTarget {
    /// Top-level slot block in the currently-selected slice.
    Block { id: String },
    /// A nested step inside an event (id = `event.step`).
    Step { event: String, step: String },
}

#[derive(Clone, Debug)]
struct DiagRow {
    severity: Severity,
    code: String,
    message: String,
    file: PathBuf,
    line: u32,
    col: u32,
}

impl State {
    fn new(invocation: PathBuf) -> Self {
        let (discovery_dir, active_root, discovered) = discover_siblings(&invocation);
        let mut slices_state = ListState::default();
        if !discovered.is_empty() {
            // Highlight whichever slice is the active root.
            let idx = discovered
                .iter()
                .position(|p| p == &active_root)
                .unwrap_or(0);
            slices_state.select(Some(idx));
        }
        Self {
            invocation,
            discovery_dir,
            discovered,
            active_root,
            ws: None,
            analysis: None,
            load_error: None,
            focused: Pane::Slices,
            slices_state,
            slots_state: ListState::default(),
            diags_state: ListState::default(),
            slot_rows: Vec::new(),
            diag_rows: Vec::new(),
        }
    }

    fn reload(&mut self) {
        let loader = FsLoader;
        let ws = load_with_imports(&loader, &self.active_root);
        let analysis = analyze_working_set(&ws);
        self.ws = Some(ws);
        self.analysis = Some(analysis);
        self.load_error = None;
        self.rebuild_slot_rows();
        self.rebuild_diag_rows();
    }

    /// Switch which discovered slice is active. Reloads its working set.
    fn set_active_root(&mut self, idx: usize) {
        if let Some(p) = self.discovered.get(idx) {
            self.active_root = p.clone();
            self.reload();
        }
    }

    fn current_slice(&self) -> Option<&memspec_parser::analysis::loader::LoadedFile> {
        let ws = self.ws.as_ref()?;
        ws.files.iter().find(|lf| lf.path == self.active_root)
    }
}

/// Resolve the invocation path to (discovery_dir, active_root, discovered_paths).
///
/// - If `invocation` is a file: parent dir is the discovery dir; active_root
///   is the file itself; discovered = all `.memspec` siblings + the file.
/// - If `invocation` is a directory: dir is the discovery dir; active_root
///   defaults to the alphabetically-first `.memspec` (or the invocation
///   itself if no .memspec files are found, in which case loading will fail
///   loudly via the loader's error path).
fn discover_siblings(invocation: &std::path::Path) -> (PathBuf, PathBuf, Vec<PathBuf>) {
    let (dir, default_root) = if invocation.is_dir() {
        (invocation.to_path_buf(), invocation.to_path_buf())
    } else {
        let parent = invocation
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        (parent, invocation.to_path_buf())
    };

    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("memspec") {
                found.push(path);
            }
        }
    }
    found.sort();

    // If invocation is a file but it isn't in `found` (extension mismatch
    // etc.), still include it so the user can view what they pointed at.
    if !invocation.is_dir() && !found.contains(&default_root) {
        found.insert(0, default_root.clone());
    }

    let active_root = if invocation.is_dir() {
        found.first().cloned().unwrap_or(default_root)
    } else {
        default_root
    };

    (dir, active_root, found)
}

impl State {

    fn rebuild_slot_rows(&mut self) {
        let rows = self.compute_slot_rows();
        let select = if rows.is_empty() { None } else { Some(0) };
        self.slot_rows = rows;
        self.slots_state.select(select);
    }

    /// Pure compute step — borrows self immutably so the caller can swap in
    /// the result without conflicting with `&mut self.slot_rows`.
    fn compute_slot_rows(&self) -> Vec<SlotRow> {
        let mut rows = Vec::new();
        let Some(slice) = self.current_slice().and_then(|lf| lf.file.slice.as_ref()) else {
            return rows;
        };

        for kind in &[
            "cell",
            "derived",
            "association",
            "event",
            "post_failure",
            "forbidden_state",
            "kill_test",
        ] {
            let blocks: Vec<&BlockDecl> = slice
                .items
                .iter()
                .filter_map(|i| match i {
                    BlockItem::Block(b) if b.kind.name == *kind => Some(b),
                    _ => None,
                })
                .collect();
            if blocks.is_empty() {
                continue;
            }
            rows.push(SlotRow {
                indent: 0,
                label: format!("▼ {}s ({})", kind_pretty(kind), blocks.len()),
                target: None,
            });
            for b in blocks {
                let Some(BlockName::Ident(name)) = &b.name else { continue };
                rows.push(SlotRow {
                    indent: 1,
                    label: format!("{} {}", icon_for_kind(kind), name.name),
                    target: Some(SlotTarget::Block { id: name.name.clone() }),
                });
                if *kind == "event" {
                    for inner in &b.items {
                        if let BlockItem::Block(step) = inner {
                            if step.kind.name == "step" {
                                if let Some(BlockName::Ident(s)) = &step.name {
                                    rows.push(SlotRow {
                                        indent: 2,
                                        label: format!("  ↳ step {}", s.name),
                                        target: Some(SlotTarget::Step {
                                            event: name.name.clone(),
                                            step: s.name.clone(),
                                        }),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        rows
    }

    fn rebuild_diag_rows(&mut self) {
        self.diag_rows.clear();
        if let Some(err) = &self.load_error {
            self.diag_rows.push(DiagRow {
                severity: Severity::Error,
                code: "memspec/loader".into(),
                message: err.clone(),
                file: self.active_root.clone(),
                line: 0,
                col: 0,
            });
        }
        let Some(analysis) = self.analysis.as_ref() else { return };
        for (path, diags) in &analysis.by_file {
            for d in diags {
                let lc = compute_line_col(path, d.span.start);
                self.diag_rows.push(DiagRow {
                    severity: d.severity,
                    code: d.code.to_string(),
                    message: d.message.clone(),
                    file: path.clone(),
                    line: lc.0,
                    col: lc.1,
                });
            }
        }
        self.diag_rows.sort_by(|a, b| {
            severity_rank(a.severity)
                .cmp(&severity_rank(b.severity))
                .then(a.file.cmp(&b.file))
                .then(a.line.cmp(&b.line))
        });
        if !self.diag_rows.is_empty() {
            self.diags_state.select(Some(0));
        } else {
            self.diags_state.select(None);
        }
    }
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

fn handle_key(k: KeyEvent, state: &mut State) -> Outcome {
    if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
        return Outcome::Quit;
    }
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => return Outcome::Quit,
        KeyCode::Char('1') => state.focused = Pane::Slices,
        KeyCode::Char('2') => state.focused = Pane::Slots,
        KeyCode::Char('3') => state.focused = Pane::Diagnostics,
        KeyCode::Tab => {
            state.focused = match state.focused {
                Pane::Slices => Pane::Slots,
                Pane::Slots => Pane::Diagnostics,
                Pane::Diagnostics => Pane::Slices,
            };
        }
        KeyCode::BackTab => {
            state.focused = match state.focused {
                Pane::Slices => Pane::Diagnostics,
                Pane::Slots => Pane::Slices,
                Pane::Diagnostics => Pane::Slots,
            };
        }
        KeyCode::Char('r') => state.reload(),
        KeyCode::Char('j') | KeyCode::Down => move_selection(state, 1),
        KeyCode::Char('k') | KeyCode::Up => move_selection(state, -1),
        KeyCode::Char('g') | KeyCode::Home => move_selection_to(state, 0),
        KeyCode::Char('G') | KeyCode::End => move_selection_to(state, isize::MAX),
        _ => {}
    }
    Outcome::Continue
}

fn move_selection(state: &mut State, delta: isize) {
    let (cur, len) = match state.focused {
        Pane::Slices => (state.slices_state.selected(), state.discovered.len()),
        Pane::Slots => (state.slots_state.selected(), state.slot_rows.len()),
        Pane::Diagnostics => (state.diags_state.selected(), state.diag_rows.len()),
    };
    if len == 0 {
        return;
    }
    let next = match cur {
        None => 0,
        Some(c) => {
            let c = c as isize + delta;
            c.clamp(0, len as isize - 1) as usize
        }
    };
    apply_selection(state, next);
}

fn move_selection_to(state: &mut State, target: isize) {
    let len = match state.focused {
        Pane::Slices => state.discovered.len(),
        Pane::Slots => state.slot_rows.len(),
        Pane::Diagnostics => state.diag_rows.len(),
    };
    if len == 0 {
        return;
    }
    let idx = if target < 0 {
        0
    } else {
        (target as usize).min(len - 1)
    };
    apply_selection(state, idx);
}

fn apply_selection(state: &mut State, idx: usize) {
    match state.focused {
        Pane::Slices => {
            state.slices_state.select(Some(idx));
            // Auto-switch active root when the cursor moves in the slices
            // pane — same UX shape as lazygit (selecting a branch shows its
            // commits). set_active_root reloads the working set.
            state.set_active_root(idx);
        }
        Pane::Slots => state.slots_state.select(Some(idx)),
        Pane::Diagnostics => state.diags_state.select(Some(idx)),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn view(f: &mut ratatui::Frame, state: &State) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[0]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),     // slices
            Constraint::Percentage(50), // slots
            Constraint::Min(0),        // diags
        ])
        .split(body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Min(0)])
        .split(body[1]);

    draw_slices(f, state, left[0]);
    draw_slots(f, state, left[1]);
    draw_diagnostics(f, state, left[2]);
    draw_detail(f, state, right[0]);
    draw_references(f, state, right[1]);
    draw_status(f, state, chunks[1]);
}

fn draw_slices(f: &mut ratatui::Frame, state: &State, area: Rect) {
    let items: Vec<ListItem> = state
        .discovered
        .iter()
        .map(|p| {
            // Pull the slice's declared name from the active working set if
            // it's currently loaded; otherwise just show the file basename.
            let display_name = state
                .ws
                .as_ref()
                .and_then(|ws| ws.files.iter().find(|lf| &lf.path == p))
                .and_then(|lf| lf.file.slice.as_ref())
                .map(|s| s.name.name.clone())
                .unwrap_or_else(|| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("<file>")
                        .to_owned()
                });

            let active = *p == state.active_root;
            let in_ws = state
                .ws
                .as_ref()
                .map(|ws| ws.files.iter().any(|lf| &lf.path == p))
                .unwrap_or(false);

            let marker = if active {
                "  ◉"  // active root
            } else if in_ws {
                "  ↳"  // imported by active root
            } else {
                "   "
            };
            ListItem::new(format!("{marker} {display_name}"))
        })
        .collect();

    let block = panel_block("[1] Slices", state.focused == Pane::Slices);
    let mut list_state = state.slices_state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style())
            .highlight_symbol("▶ "),
        area,
        &mut list_state,
    );
}

fn draw_slots(f: &mut ratatui::Frame, state: &State, area: Rect) {
    let items: Vec<ListItem> = state
        .slot_rows
        .iter()
        .map(|r| {
            let pad = "  ".repeat(r.indent);
            ListItem::new(format!("{pad}{}", r.label))
        })
        .collect();

    let block = panel_block("[2] Slots", state.focused == Pane::Slots);
    let mut list_state = state.slots_state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style())
            .highlight_symbol("▶ "),
        area,
        &mut list_state,
    );
}

fn draw_diagnostics(f: &mut ratatui::Frame, state: &State, area: Rect) {
    let items: Vec<ListItem> = state
        .diag_rows
        .iter()
        .map(|d| {
            let icon = match d.severity {
                Severity::Error => "✗",
                Severity::Warning => "⚠",
                Severity::Info => "ℹ",
            };
            let style = match d.severity {
                Severity::Error => Style::default().fg(Color::Red),
                Severity::Warning => Style::default().fg(Color::Yellow),
                Severity::Info => Style::default().fg(Color::Cyan),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), style),
                Span::styled(d.code.clone(), style.add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::raw(d.message.clone()),
            ]))
        })
        .collect();

    let block = panel_block(
        &format!("[3] Diagnostics ({})", state.diag_rows.len()),
        state.focused == Pane::Diagnostics,
    );
    let mut list_state = state.diags_state.clone();
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style())
            .highlight_symbol("▶ "),
        area,
        &mut list_state,
    );
}

fn draw_detail(f: &mut ratatui::Frame, state: &State, area: Rect) {
    let title = match state.focused {
        Pane::Slices => "Detail (slice)",
        Pane::Slots => "Detail (slot)",
        Pane::Diagnostics => "Detail (diagnostic)",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let text = match state.focused {
        Pane::Slices => detail_for_slice(state),
        Pane::Slots => detail_for_slot(state),
        Pane::Diagnostics => detail_for_diag(state),
    };

    f.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_references(f: &mut ratatui::Frame, state: &State, area: Rect) {
    let block = Block::default()
        .title("Graph (neighbors of selected node)")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let lines = match state.focused {
        Pane::Slots => graph_for_slot(state),
        _ => vec![Line::from(Span::styled(
            "(focus the Slots pane and select an id to see its graph neighbors)",
            Style::default().fg(Color::Gray),
        ))],
    };

    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_status(f: &mut ratatui::Frame, state: &State, area: Rect) {
    let (errors, warnings, infos) = match state.analysis.as_ref() {
        Some(a) => count_diagnostics(a),
        None => (0usize, 0usize, 0usize),
    };
    let walk_status = if errors > 0 {
        Span::styled("walk-incomplete", Style::default().fg(Color::Red))
    } else if warnings > 0 {
        Span::styled(
            "walk-complete (with warnings)",
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::styled("walk-complete", Style::default().fg(Color::Green))
    };
    let line = Line::from(vec![
        walk_status,
        Span::raw("  │  "),
        Span::styled(format!("{errors}E"), Style::default().fg(Color::Red)),
        Span::raw(" "),
        Span::styled(format!("{warnings}W"), Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        Span::styled(format!("{infos}I"), Style::default().fg(Color::Cyan)),
        Span::raw("  │  "),
        Span::styled(
            "1/2/3 focus  Tab cycle  j/k move  r reload  q quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// Detail content
// ---------------------------------------------------------------------------

fn detail_for_slice(state: &State) -> Vec<Line<'static>> {
    let Some(lf) = state.current_slice() else {
        return vec![Line::from("(no slice selected)")];
    };
    let Some(slice) = lf.file.slice.as_ref() else {
        return vec![Line::from("(slice failed to parse)")];
    };
    let mut lines = vec![
        section_line("slice"),
        kv_line("name", &slice.name.name),
        kv_line("path", &lf.path.display().to_string()),
    ];
    if !slice.imports.is_empty() {
        lines.push(section_line("imports"));
        for imp in &slice.imports {
            lines.push(Line::from(vec![
                Span::raw("  · "),
                Span::styled(imp.alias.name.clone(), Style::default().fg(Color::Cyan)),
                Span::raw(" → "),
                Span::raw(imp.path.clone()),
            ]));
        }
    }
    lines.push(section_line("counts"));
    let counts = count_slots(slice);
    for (label, n) in counts {
        lines.push(kv_line(label, &n.to_string()));
    }
    lines
}

fn detail_for_slot(state: &State) -> Vec<Line<'static>> {
    let idx = state.slots_state.selected();
    let Some(idx) = idx else {
        return vec![Line::from("(no slot selected)")];
    };
    let Some(row) = state.slot_rows.get(idx) else {
        return vec![Line::from("(no slot selected)")];
    };
    let Some(target) = &row.target else {
        return vec![Line::from(format!("{}", row.label))];
    };
    let Some(slice) = state.current_slice().and_then(|lf| lf.file.slice.as_ref()) else {
        return vec![Line::from("(slice unavailable)")];
    };

    match target {
        SlotTarget::Block { id } => {
            for item in &slice.items {
                if let BlockItem::Block(b) = item {
                    if let Some(BlockName::Ident(name)) = &b.name {
                        if name.name == *id {
                            return render_block(b);
                        }
                    }
                }
            }
            vec![Line::from(format!("(id `{id}` not found)"))]
        }
        SlotTarget::Step { event, step } => {
            for item in &slice.items {
                if let BlockItem::Block(b) = item {
                    if b.kind.name == "event" {
                        if let Some(BlockName::Ident(name)) = &b.name {
                            if name.name == *event {
                                for inner in &b.items {
                                    if let BlockItem::Block(s) = inner {
                                        if s.kind.name == "step" {
                                            if let Some(BlockName::Ident(sn)) = &s.name {
                                                if sn.name == *step {
                                                    let mut out = vec![section_line(
                                                        &format!("event {event} → step {step}"),
                                                    )];
                                                    out.extend(render_block(s));
                                                    return out;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            vec![Line::from(format!("(step `{step}` not found in `{event}`)"))]
        }
    }
}

fn detail_for_diag(state: &State) -> Vec<Line<'static>> {
    let Some(idx) = state.diags_state.selected() else {
        return vec![Line::from("(no diagnostic selected)")];
    };
    let Some(d) = state.diag_rows.get(idx) else {
        return vec![Line::from("(no diagnostic selected)")];
    };
    let mut lines = vec![
        section_line(&format!("{} {}", d.severity.as_str(), d.code)),
        kv_line(
            "location",
            &format!("{}:{}:{}", d.file.display(), d.line, d.col),
        ),
        Line::from(""),
        Line::from(d.message.clone()),
    ];
    // Hint, if present — diagnostics carry an optional hint we want to show.
    if let Some(diags) = state.analysis.as_ref().and_then(|a| a.by_file.get(&d.file)) {
        if let Some(orig) = diags
            .iter()
            .find(|x| x.code == d.code && x.message == d.message)
        {
            if let Some(hint) = &orig.hint {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("hint: ", Style::default().fg(Color::Cyan)),
                    Span::raw(hint.clone()),
                ]));
            }
        }
    }
    lines
}

/// Bidirectional graph view for the selected slot. Shows both:
/// - **Outgoing** edges: cell-ids / event-ids / etc. that THIS node points
///   at via its fields (derives_from, over, mutates, cells, forbidden,
///   kill_test, event, step).
/// - **Incoming** edges: every site in the slice that references THIS node's
///   id (via `query::refs_to`).
fn graph_for_slot(state: &State) -> Vec<Line<'static>> {
    let idx = state.slots_state.selected();
    let Some(idx) = idx else { return vec![Line::from("(no slot selected)")]; };
    let Some(row) = state.slot_rows.get(idx) else { return vec![] };
    let Some(target) = &row.target else { return vec![Line::from(row.label.clone())] };
    let id = match target {
        SlotTarget::Block { id } => id.clone(),
        SlotTarget::Step { step, .. } => step.clone(),
    };
    let Some(lf) = state.current_slice() else { return vec![] };
    let Some(slice) = lf.file.slice.as_ref() else { return vec![] };

    let block = match target {
        SlotTarget::Block { id } => find_top_block(slice, id),
        SlotTarget::Step { event, step } => find_step_block(slice, event, step),
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                id.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  ("),
            Span::styled(
                block
                    .map(|b| b.kind.name.clone())
                    .unwrap_or_else(|| "?".into()),
                Style::default().fg(Color::Magenta),
            ),
            Span::raw(")"),
        ]),
        Line::from(""),
    ];

    // Outgoing — fields of the selected block that hold ident references.
    lines.push(Line::from(Span::styled(
        "  outgoing →",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));
    let outbound = if let Some(b) = block {
        collect_outbound(b)
    } else {
        Vec::new()
    };
    if outbound.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no outgoing references)",
            Style::default().fg(Color::Gray),
        )));
    } else {
        for (field, ref_repr) in outbound {
            lines.push(Line::from(vec![
                Span::raw("    · via "),
                Span::styled(field, Style::default().fg(Color::Yellow)),
                Span::raw(" → "),
                Span::styled(ref_repr, Style::default().fg(Color::White)),
            ]));
        }
    }

    lines.push(Line::from(""));

    // Incoming — refs_to.
    lines.push(Line::from(Span::styled(
        "  ← incoming",
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
    )));
    let report = q::refs_to(&lf.file, &id);
    if report.references.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no references)",
            Style::default().fg(Color::Gray),
        )));
    } else {
        for r in &report.references {
            lines.push(Line::from(vec![
                Span::raw("    · "),
                Span::styled(r.from.clone(), Style::default().fg(Color::Green)),
                Span::raw("  via "),
                Span::styled(r.field.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
    }

    lines
}

fn find_top_block<'a>(
    slice: &'a memspec_parser::ast::SliceDecl,
    id: &str,
) -> Option<&'a BlockDecl> {
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            if let Some(BlockName::Ident(n)) = &b.name {
                if n.name == id {
                    return Some(b);
                }
            }
        }
    }
    None
}

fn find_step_block<'a>(
    slice: &'a memspec_parser::ast::SliceDecl,
    event: &str,
    step: &str,
) -> Option<&'a BlockDecl> {
    let event_block = find_top_block(slice, event)?;
    for item in &event_block.items {
        if let BlockItem::Block(b) = item {
            if b.kind.name == "step" {
                if let Some(BlockName::Ident(n)) = &b.name {
                    if n.name == step {
                        return Some(b);
                    }
                }
            }
        }
    }
    None
}

/// Walk a block's fields + nested anonymous blocks and collect (field-name,
/// referenced-id-display) pairs for every `Ident` and `QualifiedIdent` that
/// appears in a field value or as a map key (cells_after / cells maps).
fn collect_outbound(block: &BlockDecl) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk_block_outbound(block, &mut out);
    out
}

fn walk_block_outbound(block: &BlockDecl, out: &mut Vec<(String, String)>) {
    for item in &block.items {
        match item {
            BlockItem::Field(f) => {
                let key = f.key.name.clone();
                walk_value_outbound(&key, &f.value, out);
            }
            BlockItem::Block(inner) => {
                // Anonymous map-like blocks (cells_after_*, meta) have field
                // KEYS that are themselves cell-id references in the
                // post_failure / forbidden_state context.
                let key = format!("{}.<keys>", inner.kind.name);
                for item in &inner.items {
                    if let BlockItem::Field(f) = item {
                        out.push((key.clone(), f.key.name.clone()));
                    }
                }
                // Plus their values, which may also be idents.
                walk_block_outbound(inner, out);
            }
        }
    }
}

fn walk_value_outbound(field: &str, value: &FieldValue, out: &mut Vec<(String, String)>) {
    match value {
        FieldValue::Ident(i) => out.push((field.to_owned(), i.name.clone())),
        FieldValue::QualifiedIdent { alias, name, .. } => {
            out.push((field.to_owned(), format!("{}.{}", alias.name, name.name)))
        }
        FieldValue::List { items, .. } => {
            for it in items {
                walk_value_outbound(field, it, out);
            }
        }
        FieldValue::Map { entries, .. } => {
            for MapEntry { key, value, .. } in entries {
                out.push((format!("{field}.<key>"), key.name.clone()));
                walk_value_outbound(field, value, out);
            }
        }
        FieldValue::TypeApp { params, .. } => {
            for p in params {
                walk_value_outbound(field, p, out);
            }
        }
        FieldValue::Call { args, .. } => {
            for a in args {
                walk_value_outbound(field, a, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn panel_block(title: impl Into<String>, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style)
}

fn highlight_style() -> Style {
    Style::default()
        .bg(Color::Rgb(40, 40, 60))
        .add_modifier(Modifier::BOLD)
}

fn section_line(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("── {label} ──"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn kv_line(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {key:<14}"),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(value.to_owned()),
    ])
}

fn render_block(block: &BlockDecl) -> Vec<Line<'static>> {
    let kind_label = block.kind.name.clone();
    let name_label = match &block.name {
        Some(BlockName::Ident(i)) => i.name.clone(),
        Some(BlockName::Int { value, .. }) => value.to_string(),
        None => "<anon>".to_string(),
    };
    let mut lines = vec![section_line(&format!("{kind_label} {name_label}"))];
    render_block_items(&block.items, 0, &mut lines);
    lines
}

/// Threshold for inline value rendering. Values whose plain-text inline
/// length exceeds this trigger pretty-printed multi-line form (lists as
/// bullets, maps as key/value pairs on their own lines). Picked to fit
/// roughly half a typical 120-col terminal — leaves room for the key
/// column + indent without ratatui's wrap clobbering alignment.
const INLINE_VALUE_THRESHOLD: usize = 50;

/// Recursively render fields and nested blocks. Indent grows with nesting
/// depth so inline-expanded steps and anonymous blocks (cells_after_*,
/// meta) stay visually scoped. Field-name column width adapts to the
/// longest key in the current block so short values stay column-aligned
/// even when keys vary in length (e.g. `construction_only` next to
/// `mutates`). Long values break to multi-line bullet form so they don't
/// wrap into column 1.
fn render_block_items(items: &[BlockItem], depth: usize, out: &mut Vec<Line<'static>>) {
    let indent_str = "  ".repeat(depth + 1);
    let max_key = items
        .iter()
        .filter_map(|i| match i {
            BlockItem::Field(f) => Some(f.key.name.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);

    for item in items {
        match item {
            BlockItem::Field(f) => {
                let inline_len = value_inline_len(&f.value);
                let go_multiline = should_go_multiline(&f.value, inline_len);

                if go_multiline {
                    // Key on its own line; value pretty-printed below at
                    // depth+2 indent (one extra step deeper than fields).
                    out.push(Line::from(vec![
                        Span::raw(indent_str.clone()),
                        Span::styled(
                            f.key.name.clone(),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    render_value_multiline(&f.value, depth + 2, out);
                } else {
                    let pad = " ".repeat(max_key.saturating_sub(f.key.name.len()) + 2);
                    let mut spans = vec![
                        Span::raw(indent_str.clone()),
                        Span::styled(
                            f.key.name.clone(),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(pad),
                    ];
                    spans.extend(value_spans(&f.value));
                    out.push(Line::from(spans));
                }
            }
            BlockItem::Block(inner) => {
                let inner_kind = inner.kind.name.clone();
                let inner_name = match &inner.name {
                    Some(BlockName::Ident(i)) => i.name.clone(),
                    Some(BlockName::Int { value, .. }) => value.to_string(),
                    None => "(anon)".to_string(),
                };
                out.push(Line::from(vec![
                    Span::raw(indent_str.clone()),
                    Span::styled(
                        format!("▸ {inner_kind} {inner_name}"),
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                render_block_items(&inner.items, depth + 1, out);
            }
        }
    }
}

/// Heuristic for when to break a value to multi-line:
/// - Lists: > 3 items OR inline length > threshold.
/// - Maps: any non-empty (1 entry can still be too wide if its value is
///   long) — defer to length check.
/// - Strings: > threshold characters.
/// - Everything else: keep inline; type-apps and calls are usually small.
fn should_go_multiline(value: &FieldValue, inline_len: usize) -> bool {
    match value {
        FieldValue::List { items, .. } => {
            items.len() > 3 || inline_len > INLINE_VALUE_THRESHOLD
        }
        FieldValue::Map { entries, .. } => {
            !entries.is_empty() && inline_len > INLINE_VALUE_THRESHOLD
        }
        FieldValue::String { .. } => inline_len > INLINE_VALUE_THRESHOLD,
        _ => false,
    }
}

/// Approximate text length of `value` if rendered inline. Used by the
/// multi-line decision; doesn't have to be exact, just close enough to
/// distinguish "short" from "long".
fn value_inline_len(value: &FieldValue) -> usize {
    match value {
        FieldValue::Ident(i) => i.name.len(),
        FieldValue::Bool { value, .. } => if *value { 4 } else { 5 },
        FieldValue::Int { value, .. } => value.to_string().len(),
        FieldValue::String { value, .. } => value.len() + 2,
        FieldValue::List { items, .. } => {
            // [item, item, item]
            2 + items.iter().map(value_inline_len).sum::<usize>()
                + 2 * items.len().saturating_sub(1)
        }
        FieldValue::Map { entries, .. } => {
            4 + entries
                .iter()
                .map(|MapEntry { key, value, .. }| key.name.len() + 2 + value_inline_len(value))
                .sum::<usize>()
                + 2 * entries.len().saturating_sub(1)
        }
        FieldValue::TypeApp { head, params, .. } => {
            head.name.len()
                + 2
                + params.iter().map(value_inline_len).sum::<usize>()
                + 3 * params.len().saturating_sub(1)
        }
        FieldValue::Call { head, args, .. } => {
            head.name.len()
                + 2
                + args.iter().map(value_inline_len).sum::<usize>()
                + 2 * args.len().saturating_sub(1)
        }
        FieldValue::QualifiedIdent { alias, name, .. } => alias.name.len() + 1 + name.name.len(),
    }
}

/// Pretty-print a complex value across multiple lines. Each list item / map
/// entry gets its own Line at `indent_depth`; ratatui's wrap can still
/// break very long string values further but at least the key/value
/// structural alignment survives.
fn render_value_multiline(value: &FieldValue, indent_depth: usize, out: &mut Vec<Line<'static>>) {
    let indent_str = "  ".repeat(indent_depth);
    match value {
        FieldValue::List { items, .. } => {
            for it in items {
                let mut spans = vec![
                    Span::raw(indent_str.clone()),
                    Span::styled("· ", Style::default().fg(Color::Gray)),
                ];
                if should_go_multiline(it, value_inline_len(it)) {
                    // Nested list/map item — emit a placeholder marker and
                    // recurse one deeper.
                    spans.push(Span::styled(
                        "(nested)",
                        Style::default().fg(Color::Gray),
                    ));
                    out.push(Line::from(spans));
                    render_value_multiline(it, indent_depth + 1, out);
                } else {
                    spans.extend(value_spans(it));
                    out.push(Line::from(spans));
                }
            }
        }
        FieldValue::Map { entries, .. } => {
            let max_key = entries
                .iter()
                .map(|e| e.key.name.len())
                .max()
                .unwrap_or(0);
            for MapEntry { key, value, .. } in entries {
                let pad = " ".repeat(max_key.saturating_sub(key.name.len()) + 2);
                if should_go_multiline(value, value_inline_len(value)) {
                    out.push(Line::from(vec![
                        Span::raw(indent_str.clone()),
                        Span::styled(
                            key.name.clone(),
                            Style::default().fg(Color::Cyan),
                        ),
                    ]));
                    render_value_multiline(value, indent_depth + 1, out);
                } else {
                    let mut spans = vec![
                        Span::raw(indent_str.clone()),
                        Span::styled(
                            key.name.clone(),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(pad),
                    ];
                    spans.extend(value_spans(value));
                    out.push(Line::from(spans));
                }
            }
        }
        FieldValue::String { value, .. } => {
            // Long string — put it on its own line. Ratatui wrap still
            // applies, but at least the key sits on its own line above.
            out.push(Line::from(vec![
                Span::raw(indent_str.clone()),
                Span::styled(
                    format!("\"{value}\""),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }
        // Other complex value kinds rarely need multi-line; fall back to
        // single-line spans for them.
        _ => {
            let mut spans = vec![Span::raw(indent_str.clone())];
            spans.extend(value_spans(value));
            out.push(Line::from(spans));
        }
    }
}

/// Render a FieldValue as a sequence of styled spans — strings get yellow,
/// bools get cyan, qualified idents (alias.id) keep the alias dim and the
/// name highlighted, type apps get green for the head etc. Long string
/// values are kept on a single span so the surrounding Paragraph's wrap
/// can flow them across lines naturally.
fn value_spans(value: &FieldValue) -> Vec<Span<'static>> {
    match value {
        FieldValue::Ident(i) => vec![Span::styled(
            i.name.clone(),
            Style::default().fg(Color::White),
        )],
        FieldValue::Bool { value, .. } => vec![Span::styled(
            value.to_string(),
            Style::default().fg(Color::Cyan),
        )],
        FieldValue::Int { value, .. } => vec![Span::styled(
            value.to_string(),
            Style::default().fg(Color::LightBlue),
        )],
        FieldValue::String { value, .. } => vec![Span::styled(
            format!("\"{value}\""),
            Style::default().fg(Color::Yellow),
        )],
        FieldValue::List { items, .. } => {
            let mut out = vec![Span::raw("[")];
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(Span::raw(", "));
                }
                out.extend(value_spans(it));
            }
            out.push(Span::raw("]"));
            out
        }
        FieldValue::Map { entries, .. } => {
            let mut out = vec![Span::raw("{ ")];
            for (i, MapEntry { key, value, .. }) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(Span::raw(", "));
                }
                out.push(Span::styled(
                    key.name.clone(),
                    Style::default().fg(Color::Cyan),
                ));
                out.push(Span::raw(": "));
                out.extend(value_spans(value));
            }
            out.push(Span::raw(" }"));
            out
        }
        FieldValue::TypeApp {
            head,
            params,
            alternation,
            ..
        } => {
            let sep = if *alternation { " | " } else { ", " };
            let mut out = vec![
                Span::styled(
                    head.name.clone(),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::raw("<"),
            ];
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push(Span::raw(sep));
                }
                out.extend(value_spans(p));
            }
            out.push(Span::raw(">"));
            out
        }
        FieldValue::Call { head, args, .. } => {
            let mut out = vec![
                Span::styled(head.name.clone(), Style::default().fg(Color::Green)),
                Span::raw("("),
            ];
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push(Span::raw(", "));
                }
                out.extend(value_spans(a));
            }
            out.push(Span::raw(")"));
            out
        }
        FieldValue::QualifiedIdent { alias, name, .. } => vec![
            Span::styled(
                alias.name.clone(),
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled(".", Style::default().fg(Color::Gray)),
            Span::styled(
                name.name.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ],
    }
}

#[allow(dead_code)] // retained for future plain-text export modes
fn value_inline(value: &FieldValue) -> String {
    match value {
        FieldValue::Ident(i) => i.name.clone(),
        FieldValue::Bool { value, .. } => value.to_string(),
        FieldValue::Int { value, .. } => value.to_string(),
        FieldValue::String { value, .. } => format!("\"{value}\""),
        FieldValue::List { items, .. } => {
            let parts: Vec<String> = items.iter().map(value_inline).collect();
            format!("[{}]", parts.join(", "))
        }
        FieldValue::Map { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|MapEntry { key, value, .. }| format!("{}: {}", key.name, value_inline(value)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        FieldValue::TypeApp {
            head,
            params,
            alternation,
            ..
        } => {
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

fn count_slots(slice: &memspec_parser::ast::SliceDecl) -> Vec<(&'static str, usize)> {
    let mut by_kind = std::collections::BTreeMap::new();
    for item in &slice.items {
        if let BlockItem::Block(b) = item {
            *by_kind.entry(b.kind.name.as_str()).or_insert(0) += 1;
        }
    }
    let mut out = Vec::new();
    for kind in &[
        "cell",
        "derived",
        "association",
        "event",
        "post_failure",
        "forbidden_state",
        "kill_test",
        "walk",
    ] {
        if let Some(&n) = by_kind.get(*kind) {
            out.push((*kind, n));
        }
    }
    out
}

fn count_diagnostics(analysis: &WorkingSetAnalysis) -> (usize, usize, usize) {
    let mut e = 0;
    let mut w = 0;
    let mut i = 0;
    for diags in analysis.by_file.values() {
        for d in diags {
            match d.severity {
                Severity::Error => e += 1,
                Severity::Warning => w += 1,
                Severity::Info => i += 1,
            }
        }
    }
    (e, w, i)
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

fn kind_pretty(kind: &str) -> &'static str {
    match kind {
        "cell" => "Cell",
        "derived" => "Derived",
        "association" => "Association",
        "event" => "Event",
        "post_failure" => "Post-failure",
        "forbidden_state" => "Forbidden state",
        "kill_test" => "Kill-test",
        _ => "Other",
    }
}

fn icon_for_kind(kind: &str) -> &'static str {
    match kind {
        "cell" => "·",
        "derived" => "≡",
        "association" => "↔",
        "event" => "⚡",
        "post_failure" => "📜",
        "forbidden_state" => "⛔",
        "kill_test" => "🎯",
        _ => "?",
    }
}

fn compute_line_col(path: &PathBuf, byte_offset: usize) -> (u32, u32) {
    let Ok(source) = std::fs::read_to_string(path) else {
        return (0, 0);
    };
    let mut line = 1u32;
    let mut col = 1u32;
    let mut count = 0usize;
    for ch in source.chars() {
        if count >= byte_offset {
            break;
        }
        let l = ch.len_utf8();
        count += l;
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

