//! Recursive-descent parser for `.memspec` v0.
//!
//! Consumes the lexer's token stream and produces a [`File`] AST. The
//! parser is **structurally tolerant**: per the grammar paper's "parser
//! refusal vs. analyzer refusal" lock, the parser only enforces *syntactic*
//! shape. Semantic checks (required fields per slot, ID uniqueness,
//! ref resolution, symmetric-failure coverage) live in the analyzer.
//!
//! Recovery: on unexpected token, emit a diagnostic and skip ahead to the
//! next plausible recovery point (matching `}` or end of file). The parse
//! does not abort on the first error — agents authoring incrementally
//! get the full diagnostic set per round-trip.

use crate::ast::{
    BlockDecl, BlockItem, BlockName, Field, FieldValue, File, Ident, Import, MapEntry, SliceDecl,
};
use crate::diagnostic::{Diagnostic, codes};
use crate::lexer::tokenize;
use crate::span::Span;
use crate::token::{Token, TokenKind};

#[derive(Debug)]
pub struct ParseResult {
    pub file: File,
    pub diagnostics: Vec<Diagnostic>,
}

/// Tokenize then parse. Returns the AST + the union of lexer + parser
/// diagnostics. The AST is always returned (possibly empty) so downstream
/// passes can still inspect partial results.
pub fn parse(source: &str) -> ParseResult {
    let stream = tokenize(source);
    let mut parser = Parser::new(&stream.tokens);
    let file = parser.parse_file();
    let mut diagnostics = stream.diagnostics;
    diagnostics.append(&mut parser.diagnostics);
    ParseResult { file, diagnostics }
}

struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'t> Parser<'t> {
    fn new(tokens: &'t [Token]) -> Self {
        Self { tokens, pos: 0, diagnostics: Vec::new() }
    }

    // ---------------------------------------------------------------------
    // Cursor primitives
    // ---------------------------------------------------------------------

    fn peek(&self) -> &Token {
        // Lexer always appends an EOF token; safe to index.
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_at(&self, offset: usize) -> &Token {
        let idx = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[idx]
    }

    fn bump(&mut self) -> &Token {
        let t = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if !matches!(t.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        t
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    /// Try to consume a specific punctuation/keyword-shaped token. Returns
    /// `Some(span)` if matched and consumed; `None` otherwise.
    fn eat(&mut self, kind: &TokenKind) -> Option<Span> {
        if self.peek_kind() == kind {
            let span = self.peek().span;
            self.bump();
            Some(span)
        } else {
            None
        }
    }

    /// Require a specific token. Emits a diagnostic if missing; returns the
    /// span of the actual or expected position so callers can still chain.
    fn expect(&mut self, kind: &TokenKind) -> Span {
        if let Some(span) = self.eat(kind) {
            return span;
        }
        let actual = self.peek();
        let span = actual.span;
        self.diagnostics.push(Diagnostic::error(
            codes::E_PARSE_EXPECTED_TOKEN,
            span,
            format!("expected {}, found {}", kind.label(), actual.kind.label()),
        ));
        span
    }

    fn expect_ident(&mut self) -> Option<Ident> {
        let token = self.peek();
        if let TokenKind::Identifier(name) = &token.kind {
            let id = Ident { span: token.span, name: name.clone() };
            self.bump();
            Some(id)
        } else {
            self.diagnostics.push(Diagnostic::error(
                codes::E_PARSE_EXPECTED_IDENT,
                token.span,
                format!("expected identifier, found {}", token.kind.label()),
            ));
            None
        }
    }

    /// Skip ahead to the next `}` (consuming it) or to EOF, whichever comes
    /// first. Used as a panic-recovery boundary inside a block.
    #[allow(dead_code)]
    fn skip_to_block_end(&mut self) {
        let mut depth: i32 = 1;
        while !self.at_eof() {
            match self.peek_kind() {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    self.bump();
                    if depth <= 0 {
                        return;
                    }
                    continue;
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ---------------------------------------------------------------------
    // Top-level
    // ---------------------------------------------------------------------

    fn parse_file(&mut self) -> File {
        let start = self.peek().span.start;
        if self.at_eof() {
            self.diagnostics.push(Diagnostic::error(
                codes::E_PARSE_EMPTY_FILE,
                Span::new(start, start),
                "empty file: expected a top-level `slice` declaration",
            ));
            return File { span: Span::new(start, start), slice: None };
        }

        let slice = self.parse_slice();

        // Reject any further top-level content.
        if !self.at_eof() {
            let extra_span = self.peek().span;
            self.diagnostics.push(
                Diagnostic::error(
                    codes::E_PARSE_MULTIPLE_SLICES,
                    extra_span,
                    "only one top-level `slice` declaration is allowed per file",
                )
                .with_hint("v0 grammar permits one slice per .memspec file"),
            );
            // Drain to EOF so the diagnostic is the only one.
            while !self.at_eof() {
                self.bump();
            }
        }

        let end = self.peek().span.start;
        File { span: Span::new(start, end), slice }
    }

    fn parse_slice(&mut self) -> Option<SliceDecl> {
        // Expect `slice IDENT { ... }`
        let kw = self.peek();
        let TokenKind::Identifier(kw_name) = &kw.kind else {
            self.diagnostics.push(Diagnostic::error(
                codes::E_PARSE_EXPECTED_SLICE,
                kw.span,
                format!("expected `slice` keyword, found {}", kw.kind.label()),
            ));
            return None;
        };
        if kw_name != "slice" {
            self.diagnostics.push(
                Diagnostic::error(
                    codes::E_PARSE_EXPECTED_SLICE,
                    kw.span,
                    format!("expected top-level `slice` declaration, found `{}`", kw_name),
                )
                .with_hint("a .memspec file must begin with `slice IDENT { ... }`"),
            );
            return None;
        }
        let kw_span = kw.span;
        self.bump();

        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace);
        let imports = self.parse_imports();
        let items = self.parse_block_items();
        let close_span = self.expect(&TokenKind::RBrace);

        Some(SliceDecl {
            span: kw_span.join(close_span),
            name,
            imports,
            items,
        })
    }

    /// Parse zero or more `use "<path>" as <alias>` declarations at the
    /// top of the slice body. Stops at the first non-`use` token.
    fn parse_imports(&mut self) -> Vec<Import> {
        let mut imports: Vec<Import> = Vec::new();
        loop {
            let head = self.peek();
            let TokenKind::Identifier(name) = &head.kind else { break };
            if name != "use" {
                break;
            }
            let kw_span = head.span;
            self.bump();

            // Path string literal
            let path_token = self.peek();
            let (path_value, path_span) = match &path_token.kind {
                TokenKind::StringLit { value, .. } => {
                    let span = path_token.span;
                    let v = value.clone();
                    self.bump();
                    (v, span)
                }
                _ => {
                    self.diagnostics.push(Diagnostic::error(
                        codes::E_PARSE_EXPECTED_TOKEN,
                        path_token.span,
                        format!(
                            "expected string-literal path after `use`, found {}",
                            path_token.kind.label()
                        ),
                    ));
                    // Try to recover: skip one token so we don't infinite-loop.
                    self.bump();
                    continue;
                }
            };

            // `as` keyword
            let as_token = self.peek();
            let TokenKind::Identifier(as_name) = &as_token.kind else {
                self.diagnostics.push(Diagnostic::error(
                    codes::E_PARSE_EXPECTED_AS,
                    as_token.span,
                    format!("expected `as` after import path, found {}", as_token.kind.label()),
                ));
                continue;
            };
            if as_name != "as" {
                self.diagnostics.push(Diagnostic::error(
                    codes::E_PARSE_EXPECTED_AS,
                    as_token.span,
                    format!("expected `as` keyword after import path, found `{as_name}`"),
                ));
                // Recover: skip the unexpected token and continue.
                self.bump();
                continue;
            }
            self.bump();

            let Some(alias) = self.expect_ident() else {
                continue;
            };

            // Duplicate-alias check.
            if imports.iter().any(|i| i.alias.name == alias.name) {
                self.diagnostics.push(
                    Diagnostic::error(
                        codes::E_PARSE_DUPLICATE_IMPORT_ALIAS,
                        alias.span,
                        format!("duplicate import alias `{}`", alias.name),
                    )
                    .with_hint("each `use` declaration must have a distinct alias within the slice"),
                );
            }

            let import_span = kw_span.join(alias.span);
            imports.push(Import {
                span: import_span,
                path: path_value,
                path_span,
                alias,
            });
        }

        // If a `use` appears AFTER another decl, it's a programming error —
        // surface it as a diagnostic on subsequent passes through this loop
        // boundary. We catch this in `parse_block_items` by emitting a
        // diagnostic if it sees `use` mid-body. (Implemented below.)
        imports
    }

    // ---------------------------------------------------------------------
    // Block bodies
    // ---------------------------------------------------------------------

    fn parse_block_items(&mut self) -> Vec<BlockItem> {
        let mut items = Vec::new();
        while !self.at_eof() && !matches!(self.peek_kind(), TokenKind::RBrace) {
            match self.parse_block_item() {
                Some(it) => items.push(it),
                None => {
                    // Recovery: consume one token to make progress; retry loop.
                    if !self.at_eof() {
                        self.bump();
                    }
                }
            }
        }
        items
    }

    /// One item in a block body — either a field (`key: value`) or a nested
    /// block (`keyword [name] { ... }`). Determined by 2-token lookahead on
    /// the leading identifier.
    fn parse_block_item(&mut self) -> Option<BlockItem> {
        let head = self.peek();
        let TokenKind::Identifier(head_name) = &head.kind else {
            self.diagnostics.push(Diagnostic::error(
                codes::E_PARSE_UNEXPECTED_TOKEN,
                head.span,
                format!("expected identifier (field name or block keyword), found {}", head.kind.label()),
            ));
            return None;
        };

        // Catch `use` declarations that appear AFTER the first slot decl
        // (imports must precede everything else inside a slice).
        if head_name == "use" {
            self.diagnostics.push(
                Diagnostic::error(
                    codes::E_PARSE_USE_AFTER_DECL,
                    head.span,
                    "`use` declarations must appear at the top of the slice body, before any other declarations",
                )
                .with_hint("move all `use \"...\" as <alias>` lines to immediately after the slice's opening `{`"),
            );
            // Skip the use clause: `use "..." as IDENT`
            self.bump(); // use
            if matches!(self.peek_kind(), TokenKind::StringLit { .. }) {
                self.bump();
            }
            if matches!(self.peek_kind(), TokenKind::Identifier(s) if s == "as") {
                self.bump();
            }
            if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
                self.bump();
            }
            return None;
        }

        // 2-token lookahead.
        let next = &self.peek_at(1).kind;
        match next {
            TokenKind::Colon => self.parse_field().map(BlockItem::Field),
            TokenKind::LBrace
            | TokenKind::Identifier(_)
            | TokenKind::IntLit(_) => self.parse_block_decl().map(BlockItem::Block),
            other => {
                // Couldn't disambiguate. Emit diagnostic against the second
                // token and consume the head to avoid an infinite loop.
                let span = self.peek_at(1).span;
                self.diagnostics.push(Diagnostic::error(
                    codes::E_PARSE_UNEXPECTED_TOKEN,
                    span,
                    format!(
                        "expected `:` (field), `{{` (anonymous block), or identifier/integer (named block) after `{}`, found {}",
                        head.kind.label(), other.label(),
                    ),
                ));
                self.bump();
                None
            }
        }
    }

    fn parse_field(&mut self) -> Option<Field> {
        let key = self.expect_ident()?;
        self.expect(&TokenKind::Colon);
        let value = self.parse_field_value()?;
        let span = key.span.join(value.span());
        Some(Field { span, key, value })
    }

    fn parse_block_decl(&mut self) -> Option<BlockDecl> {
        let kind = self.expect_ident()?;
        let name = match self.peek_kind() {
            TokenKind::LBrace => None,
            TokenKind::Identifier(_) => {
                let id = self.expect_ident()?;
                Some(BlockName::Ident(id))
            }
            TokenKind::IntLit(value) => {
                let span = self.peek().span;
                let v = *value;
                self.bump();
                Some(BlockName::Int { span, value: v })
            }
            other => {
                let span = self.peek().span;
                self.diagnostics.push(Diagnostic::error(
                    codes::E_PARSE_UNEXPECTED_TOKEN,
                    span,
                    format!("expected block name (identifier or integer) or `{{`, found {}", other.label()),
                ));
                return None;
            }
        };
        self.expect(&TokenKind::LBrace);
        let items = self.parse_block_items();
        let close_span = if matches!(self.peek_kind(), TokenKind::RBrace) {
            let s = self.peek().span;
            self.bump();
            s
        } else {
            self.diagnostics.push(Diagnostic::error(
                codes::E_PARSE_UNCLOSED_BLOCK,
                kind.span,
                format!("unclosed `{}` block (expected `}}`)", kind.name),
            ));
            kind.span
        };
        Some(BlockDecl {
            span: kind.span.join(close_span),
            kind,
            name,
            items,
        })
    }

    // ---------------------------------------------------------------------
    // Field values
    // ---------------------------------------------------------------------

    fn parse_field_value(&mut self) -> Option<FieldValue> {
        let token = self.peek();
        match &token.kind {
            TokenKind::LBracket => Some(self.parse_list()),
            TokenKind::LBrace => Some(self.parse_map()),
            TokenKind::StringLit { value, is_triple } => {
                let span = token.span;
                let v = value.clone();
                let triple = *is_triple;
                self.bump();
                Some(FieldValue::String { span, value: v, is_triple: triple })
            }
            TokenKind::IntLit(n) => {
                let span = token.span;
                let v = *n;
                self.bump();
                Some(FieldValue::Int { span, value: v })
            }
            TokenKind::Identifier(name) => {
                let head = Ident { span: token.span, name: name.clone() };
                self.bump();
                // Disambiguate by lookahead.
                match self.peek_kind() {
                    TokenKind::LAngle => Some(self.parse_type_app(head)),
                    TokenKind::LParen => Some(self.parse_call(head)),
                    TokenKind::Dot => {
                        // `alias.id` qualified reference.
                        self.bump(); // consume the dot
                        let Some(name_id) = self.expect_ident() else {
                            return Some(ident_to_value(head));
                        };
                        let span = head.span.join(name_id.span);
                        Some(FieldValue::QualifiedIdent {
                            span,
                            alias: head,
                            name: name_id,
                        })
                    }
                    _ => Some(ident_to_value(head)),
                }
            }
            other => {
                self.diagnostics.push(Diagnostic::error(
                    codes::E_PARSE_EXPECTED_FIELD_VALUE,
                    token.span,
                    format!("expected field value, found {}", other.label()),
                ));
                None
            }
        }
    }

    fn parse_list(&mut self) -> FieldValue {
        let open_span = self.expect(&TokenKind::LBracket);
        let mut items = Vec::new();
        while !self.at_eof() && !matches!(self.peek_kind(), TokenKind::RBracket) {
            if let Some(v) = self.parse_field_value() {
                items.push(v);
            } else if !self.at_eof() {
                self.bump();
            }
            // Optional comma; trailing allowed.
            if !matches!(self.peek_kind(), TokenKind::RBracket) {
                let _ = self.eat(&TokenKind::Comma);
            }
        }
        let close_span = self.expect(&TokenKind::RBracket);
        FieldValue::List { span: open_span.join(close_span), items }
    }

    fn parse_map(&mut self) -> FieldValue {
        let open_span = self.expect(&TokenKind::LBrace);
        let mut entries = Vec::new();
        while !self.at_eof() && !matches!(self.peek_kind(), TokenKind::RBrace) {
            let Some(key) = self.expect_ident() else {
                if !self.at_eof() {
                    self.bump();
                }
                continue;
            };
            self.expect(&TokenKind::Colon);
            let value = match self.parse_field_value() {
                Some(v) => v,
                None => continue,
            };
            let span = key.span.join(value.span());
            entries.push(MapEntry { span, key, value });
            if !matches!(self.peek_kind(), TokenKind::RBrace) {
                let _ = self.eat(&TokenKind::Comma);
            }
        }
        let close_span = self.expect(&TokenKind::RBrace);
        FieldValue::Map { span: open_span.join(close_span), entries }
    }

    /// `IDENT < param (sep param)* >` — separator is `|` (alternation) or
    /// `,` (positional). The first separator encountered locks the kind.
    fn parse_type_app(&mut self, head: Ident) -> FieldValue {
        let _lt = self.expect(&TokenKind::LAngle);
        let mut params = Vec::new();
        let mut alternation = false;
        let mut saw_separator = false;

        if !matches!(self.peek_kind(), TokenKind::RAngle) {
            if let Some(v) = self.parse_field_value() {
                params.push(v);
            }
            loop {
                match self.peek_kind() {
                    TokenKind::Pipe => {
                        if !saw_separator {
                            alternation = true;
                            saw_separator = true;
                        }
                        self.bump();
                    }
                    TokenKind::Comma => {
                        if !saw_separator {
                            alternation = false;
                            saw_separator = true;
                        }
                        self.bump();
                    }
                    _ => break,
                }
                if matches!(self.peek_kind(), TokenKind::RAngle) {
                    break;
                }
                if let Some(v) = self.parse_field_value() {
                    params.push(v);
                } else {
                    break;
                }
            }
        }
        let close_span = self.expect(&TokenKind::RAngle);
        FieldValue::TypeApp {
            span: head.span.join(close_span),
            head,
            params,
            alternation,
        }
    }

    /// `IDENT ( arg (, arg)* )`
    fn parse_call(&mut self, head: Ident) -> FieldValue {
        let _lp = self.expect(&TokenKind::LParen);
        let mut args = Vec::new();
        if !matches!(self.peek_kind(), TokenKind::RParen) {
            if let Some(v) = self.parse_field_value() {
                args.push(v);
            }
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                if matches!(self.peek_kind(), TokenKind::RParen) {
                    break;
                }
                if let Some(v) = self.parse_field_value() {
                    args.push(v);
                } else {
                    break;
                }
            }
        }
        let close_span = self.expect(&TokenKind::RParen);
        FieldValue::Call {
            span: head.span.join(close_span),
            head,
            args,
        }
    }
}

/// Lower a bare identifier to either a Bool (for `true`/`false`) or an
/// Ident value (everything else; semantic checks happen later).
fn ident_to_value(id: Ident) -> FieldValue {
    match id.name.as_str() {
        "true" => FieldValue::Bool { span: id.span, value: true },
        "false" => FieldValue::Bool { span: id.span, value: false },
        _ => FieldValue::Ident(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BlockName, FieldValue};

    fn parse_str(src: &str) -> ParseResult {
        parse(src)
    }

    fn assert_no_diagnostics(r: &ParseResult) {
        assert!(
            r.diagnostics.is_empty(),
            "unexpected diagnostics: {:#?}",
            r.diagnostics
        );
    }

    #[test]
    fn parses_minimal_slice() {
        let r = parse_str("slice empty { }");
        assert_no_diagnostics(&r);
        let s = r.file.slice.expect("slice present");
        assert_eq!(s.name.name, "empty");
        assert!(s.items.is_empty());
    }

    #[test]
    fn parses_single_field() {
        let r = parse_str(r#"slice s { meta { title: "hello" } }"#);
        assert_no_diagnostics(&r);
        let slice = r.file.slice.unwrap();
        assert_eq!(slice.items.len(), 1);
        let BlockItem::Block(meta) = &slice.items[0] else { panic!("expected block") };
        assert_eq!(meta.kind.name, "meta");
        assert!(meta.name.is_none());
        let BlockItem::Field(f) = &meta.items[0] else { panic!("expected field") };
        assert_eq!(f.key.name, "title");
        let FieldValue::String { value, is_triple, .. } = &f.value else { panic!("string expected") };
        assert_eq!(value, "hello");
        assert!(!is_triple);
    }

    #[test]
    fn parses_named_block() {
        let r = parse_str("slice s { cell foo { type: boolean } }");
        assert_no_diagnostics(&r);
        let cell = match &r.file.slice.unwrap().items[0] {
            BlockItem::Block(b) => b.clone(),
            _ => panic!(),
        };
        assert_eq!(cell.kind.name, "cell");
        let BlockName::Ident(n) = cell.name.unwrap() else { panic!() };
        assert_eq!(n.name, "foo");
    }

    #[test]
    fn parses_walk_block_with_int_name() {
        let r = parse_str("slice s { walk 1 { summary: \"first\" } }");
        assert_no_diagnostics(&r);
        let walk = match &r.file.slice.unwrap().items[0] {
            BlockItem::Block(b) => b.clone(),
            _ => panic!(),
        };
        assert_eq!(walk.kind.name, "walk");
        let BlockName::Int { value, .. } = walk.name.unwrap() else { panic!() };
        assert_eq!(value, 1);
    }

    #[test]
    fn parses_type_app_with_alternation() {
        let r = parse_str("slice s { cell c { type: enum<draft | published | archived> } }");
        assert_no_diagnostics(&r);
        let f = first_field_named(&r, "type");
        let FieldValue::TypeApp { head, params, alternation, .. } = f else { panic!() };
        assert_eq!(head.name, "enum");
        assert!(alternation);
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn parses_type_app_positional() {
        let r = parse_str("slice s { cell c { type: map<draft, archived> } }");
        assert_no_diagnostics(&r);
        let f = first_field_named(&r, "type");
        let FieldValue::TypeApp { alternation, params, .. } = f else { panic!() };
        assert!(!alternation);
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn parses_list_of_idents() {
        let r = parse_str("slice s { event e { mutates: [a, b, c] } }");
        assert_no_diagnostics(&r);
        let f = first_field_named(&r, "mutates");
        let FieldValue::List { items, .. } = f else { panic!() };
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn parses_map_value() {
        let r = parse_str(r#"slice s { cell c { impl_hints: { rust: "Vec<u8>", ruby: "Array" } } }"#);
        assert_no_diagnostics(&r);
        let f = first_field_named(&r, "impl_hints");
        let FieldValue::Map { entries, .. } = f else { panic!() };
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parses_bool_literals() {
        let r = parse_str("slice s { cell c { mutable: true default_active: false } }");
        assert_no_diagnostics(&r);
        let mutable = first_field_named(&r, "mutable");
        assert!(matches!(mutable, FieldValue::Bool { value: true, .. }));
        let default_active = first_field_named(&r, "default_active");
        assert!(matches!(default_active, FieldValue::Bool { value: false, .. }));
    }

    #[test]
    fn parses_call_form() {
        let r = parse_str("slice s { association a { enforced_by: event_handler(promote) } }");
        assert_no_diagnostics(&r);
        let f = first_field_named(&r, "enforced_by");
        let FieldValue::Call { head, args, .. } = f else { panic!() };
        assert_eq!(head.name, "event_handler");
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn parses_anonymous_block() {
        let r = parse_str(
            r#"slice s {
                post_failure pf {
                    cells_after_pre_rollback {
                        a: published
                        b: true
                    }
                }
            }"#,
        );
        assert_no_diagnostics(&r);
        let pf = match &r.file.slice.unwrap().items[0] {
            BlockItem::Block(b) => b.clone(),
            _ => panic!(),
        };
        let inner = match &pf.items[0] {
            BlockItem::Block(b) => b.clone(),
            _ => panic!(),
        };
        assert_eq!(inner.kind.name, "cells_after_pre_rollback");
        assert!(inner.name.is_none());
        assert_eq!(inner.items.len(), 2);
    }

    #[test]
    fn empty_file_emits_diagnostic() {
        let r = parse_str("");
        assert!(r.diagnostics.iter().any(|d| d.code == codes::E_PARSE_EMPTY_FILE));
    }

    #[test]
    fn missing_top_level_slice_emits_diagnostic() {
        let r = parse_str("cell foo { }");
        assert!(r.diagnostics.iter().any(|d| d.code == codes::E_PARSE_EXPECTED_SLICE));
    }

    #[test]
    fn extra_top_level_decl_emits_diagnostic() {
        let r = parse_str("slice a { } slice b { }");
        assert!(r.diagnostics.iter().any(|d| d.code == codes::E_PARSE_MULTIPLE_SLICES));
    }

    #[test]
    fn parses_use_declaration() {
        let r = parse_str(r#"slice s { use "./other.memspec" as o }"#);
        assert_no_diagnostics(&r);
        let slice = r.file.slice.unwrap();
        assert_eq!(slice.imports.len(), 1);
        assert_eq!(slice.imports[0].path, "./other.memspec");
        assert_eq!(slice.imports[0].alias.name, "o");
        assert!(slice.items.is_empty());
    }

    #[test]
    fn parses_multiple_use_declarations() {
        let r = parse_str(
            r#"slice s {
                use "./a.memspec" as a
                use "./b.memspec" as b
            }"#,
        );
        assert_no_diagnostics(&r);
        let slice = r.file.slice.unwrap();
        assert_eq!(slice.imports.len(), 2);
        assert_eq!(slice.imports[0].alias.name, "a");
        assert_eq!(slice.imports[1].alias.name, "b");
    }

    #[test]
    fn parses_qualified_ref_in_field_value() {
        let r = parse_str(
            r#"slice s {
                use "./other.memspec" as o
                derived d {
                    derives_from: [o.cell_a, local_b]
                    derivation: "..."
                }
                cell local_b { type: boolean mutable: true }
            }"#,
        );
        assert_no_diagnostics(&r);
        let slice = r.file.slice.unwrap();
        let derived = match &slice.items[0] {
            BlockItem::Block(b) => b.clone(),
            _ => panic!(),
        };
        let FieldValue::List { items, .. } = field_of(&derived, "derives_from") else { panic!() };
        let FieldValue::QualifiedIdent { alias, name, .. } = &items[0] else { panic!() };
        assert_eq!(alias.name, "o");
        assert_eq!(name.name, "cell_a");
        let FieldValue::Ident(i) = &items[1] else { panic!() };
        assert_eq!(i.name, "local_b");
    }

    #[test]
    fn duplicate_import_alias_emits_diagnostic() {
        let r = parse_str(
            r#"slice s {
                use "./a.memspec" as x
                use "./b.memspec" as x
            }"#,
        );
        assert!(r.diagnostics.iter().any(|d| d.code == codes::E_PARSE_DUPLICATE_IMPORT_ALIAS));
    }

    #[test]
    fn use_after_decl_emits_diagnostic() {
        let r = parse_str(
            r#"slice s {
                cell c { type: boolean mutable: true }
                use "./other.memspec" as o
            }"#,
        );
        assert!(r.diagnostics.iter().any(|d| d.code == codes::E_PARSE_USE_AFTER_DECL));
    }

    #[test]
    fn use_without_as_emits_diagnostic() {
        let r = parse_str(r#"slice s { use "./a.memspec" foo }"#);
        assert!(r.diagnostics.iter().any(|d| d.code == codes::E_PARSE_EXPECTED_AS));
    }

    fn field_of(block: &BlockDecl, key: &str) -> FieldValue {
        block
            .items
            .iter()
            .find_map(|i| match i {
                BlockItem::Field(f) if f.key.name == key => Some(f.value.clone()),
                _ => None,
            })
            .expect("field present")
    }

    #[test]
    fn parses_canonical_fixture_without_diagnostics() {
        let src = include_str!("../tests/fixtures/rule_lifecycle_minimal.memspec");
        let r = parse_str(src);
        assert!(
            r.diagnostics.is_empty(),
            "expected canonical fixture to parse cleanly, got: {:#?}",
            r.diagnostics
        );
        let slice = r.file.slice.expect("expected a slice");
        assert_eq!(slice.name.name, "rule_lifecycle_minimal");
        // Spot-check: meta + walk + 3 cells + 1 derived + 1 association
        // + 2 events + 1 post_failure + 1 forbidden_state + 1 kill_test
        assert!(slice.items.len() >= 11, "expected ≥11 top-level items, got {}", slice.items.len());
    }

    // ----- helpers -----

    fn first_field_named(r: &ParseResult, key: &str) -> FieldValue {
        fn find(items: &[BlockItem], key: &str) -> Option<FieldValue> {
            for it in items {
                match it {
                    BlockItem::Field(f) if f.key.name == key => return Some(f.value.clone()),
                    BlockItem::Block(b) => {
                        if let Some(v) = find(&b.items, key) {
                            return Some(v);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        find(&r.file.slice.as_ref().unwrap().items, key)
            .unwrap_or_else(|| panic!("field `{}` not found", key))
    }
}
