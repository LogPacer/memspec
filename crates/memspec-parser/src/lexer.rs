//! Hand-rolled single-pass lexer for `.memspec`.
//!
//! Produces a [`TokenStream`] (tokens + diagnostics) over a UTF-8 source.
//! The lexer is deliberately tolerant of in-progress edits: it never
//! aborts on a bad character — it emits a diagnostic and moves on.
//! Recovery boundary is the next whitespace.

use crate::diagnostic::{Diagnostic, codes};
use crate::span::Span;
use crate::token::{Token, TokenKind};

#[derive(Debug)]
pub struct TokenStream {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Top-level entry point. Tokenizes a UTF-8 string in a single pass.
pub fn tokenize(source: &str) -> TokenStream {
    let mut lexer = Lexer::new(source);
    lexer.run();
    TokenStream {
        tokens: lexer.tokens,
        diagnostics: lexer.diagnostics,
    }
}

struct Lexer<'s> {
    source: &'s str,
    bytes: &'s [u8],
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'s> Lexer<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(&mut self) {
        while self.pos < self.bytes.len() {
            self.skip_trivia();
            if self.pos >= self.bytes.len() {
                break;
            }
            self.scan_one();
        }
        let end = self.bytes.len();
        self.tokens.push(Token::new(TokenKind::Eof, Span::new(end, end)));
    }

    /// Skip whitespace and comments. Mutates `self.pos`.
    fn skip_trivia(&mut self) {
        loop {
            // Whitespace.
            while self.pos < self.bytes.len() && is_whitespace(self.bytes[self.pos]) {
                self.pos += 1;
            }
            if self.pos + 1 < self.bytes.len() && self.bytes[self.pos] == b'/' {
                match self.bytes[self.pos + 1] {
                    b'/' => {
                        // Line comment to EOL.
                        self.pos += 2;
                        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                            self.pos += 1;
                        }
                        continue;
                    }
                    b'*' => {
                        let start = self.pos;
                        self.pos += 2;
                        let mut closed = false;
                        while self.pos + 1 < self.bytes.len() {
                            if self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/' {
                                self.pos += 2;
                                closed = true;
                                break;
                            }
                            self.pos += 1;
                        }
                        if !closed {
                            // Consume to end-of-input and emit diagnostic.
                            self.pos = self.bytes.len();
                            self.diagnostics.push(Diagnostic::error(
                                codes::E_LEX_UNTERMINATED_BLOCK_COMMENT,
                                Span::new(start, self.pos),
                                "unterminated block comment",
                            ));
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            break;
        }
    }

    fn scan_one(&mut self) {
        let start = self.pos;
        let b = self.bytes[self.pos];
        match b {
            b'{' => self.emit_punct(TokenKind::LBrace),
            b'}' => self.emit_punct(TokenKind::RBrace),
            b'[' => self.emit_punct(TokenKind::LBracket),
            b']' => self.emit_punct(TokenKind::RBracket),
            b'(' => self.emit_punct(TokenKind::LParen),
            b')' => self.emit_punct(TokenKind::RParen),
            b'<' => self.emit_punct(TokenKind::LAngle),
            b'>' => self.emit_punct(TokenKind::RAngle),
            b',' => self.emit_punct(TokenKind::Comma),
            b':' => self.emit_punct(TokenKind::Colon),
            b';' => self.emit_punct(TokenKind::Semi),
            b'.' => self.emit_punct(TokenKind::Dot),
            b'|' => self.emit_punct(TokenKind::Pipe),
            b'=' => self.emit_punct(TokenKind::Eq),
            b'-' => {
                if self.peek(1) == Some(b'>') {
                    self.pos += 2;
                    self.tokens.push(Token::new(TokenKind::Arrow, Span::new(start, self.pos)));
                } else {
                    // Lone '-' isn't valid in v0; emit diagnostic and skip.
                    self.pos += 1;
                    self.diagnostics.push(Diagnostic::error(
                        codes::E_LEX_UNEXPECTED_CHAR,
                        Span::new(start, self.pos),
                        "unexpected character `-` (expected `->`)",
                    ));
                }
            }
            b'"' => self.scan_string(start),
            b'0'..=b'9' => self.scan_int(start),
            _ if is_ident_start(b) => self.scan_identifier(start),
            _ => {
                // Unknown character: emit diagnostic, advance one byte (or
                // one full UTF-8 codepoint if multibyte).
                let advance = utf8_codepoint_len(b);
                self.pos += advance;
                self.diagnostics.push(Diagnostic::error(
                    codes::E_LEX_UNEXPECTED_CHAR,
                    Span::new(start, self.pos),
                    format!(
                        "unexpected character `{}`",
                        self.source.get(start..self.pos).unwrap_or("?")
                    ),
                ));
            }
        }
    }

    fn emit_punct(&mut self, kind: TokenKind) {
        let start = self.pos;
        self.pos += 1;
        self.tokens.push(Token::new(kind, Span::new(start, self.pos)));
    }

    fn scan_identifier(&mut self, start: usize) {
        while self.pos < self.bytes.len() && is_ident_cont(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let span = Span::new(start, self.pos);
        let text = span.slice(self.source).to_owned();
        self.tokens.push(Token::new(TokenKind::Identifier(text), span));
    }

    fn scan_int(&mut self, start: usize) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let span = Span::new(start, self.pos);
        let text = span.slice(self.source);
        match text.parse::<i64>() {
            Ok(n) => self.tokens.push(Token::new(TokenKind::IntLit(n), span)),
            Err(_) => {
                self.diagnostics.push(Diagnostic::error(
                    codes::E_LEX_INVALID_INT,
                    span,
                    format!("invalid integer literal `{text}`"),
                ));
            }
        }
    }

    fn scan_string(&mut self, start: usize) {
        // Detect triple-quote: """..."""
        let is_triple = self.bytes.get(start + 1) == Some(&b'"')
            && self.bytes.get(start + 2) == Some(&b'"');
        let opener_len = if is_triple { 3 } else { 1 };
        self.pos = start + opener_len;

        let mut value = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                self.diagnostics.push(Diagnostic::error(
                    codes::E_LEX_UNTERMINATED_STRING,
                    Span::new(start, self.pos),
                    "unterminated string literal",
                ));
                break;
            }
            let b = self.bytes[self.pos];
            if is_triple {
                if b == b'"'
                    && self.bytes.get(self.pos + 1) == Some(&b'"')
                    && self.bytes.get(self.pos + 2) == Some(&b'"')
                {
                    self.pos += 3;
                    break;
                }
                value.push(b as char);
                self.pos += 1;
            } else {
                match b {
                    b'"' => {
                        self.pos += 1;
                        break;
                    }
                    b'\\' => {
                        self.pos += 1;
                        if self.pos >= self.bytes.len() {
                            self.diagnostics.push(Diagnostic::error(
                                codes::E_LEX_UNTERMINATED_STRING,
                                Span::new(start, self.pos),
                                "unterminated string literal (trailing backslash)",
                            ));
                            break;
                        }
                        let esc = self.bytes[self.pos];
                        self.pos += 1;
                        match esc {
                            b'n' => value.push('\n'),
                            b't' => value.push('\t'),
                            b'r' => value.push('\r'),
                            b'\\' => value.push('\\'),
                            b'"' => value.push('"'),
                            b'0' => value.push('\0'),
                            other => {
                                self.diagnostics.push(Diagnostic::error(
                                    codes::E_LEX_INVALID_ESCAPE,
                                    Span::new(self.pos - 2, self.pos),
                                    format!("invalid escape sequence `\\{}`", other as char),
                                ));
                                value.push(other as char);
                            }
                        }
                    }
                    b'\n' => {
                        self.diagnostics.push(Diagnostic::error(
                            codes::E_LEX_UNTERMINATED_STRING,
                            Span::new(start, self.pos),
                            "unterminated string literal (newline before closing quote — use triple-quoted string for multi-line)",
                        ));
                        break;
                    }
                    _ => {
                        let cp_len = utf8_codepoint_len(b);
                        let chunk = &self.source[self.pos..self.pos + cp_len];
                        value.push_str(chunk);
                        self.pos += cp_len;
                    }
                }
            }
        }

        self.tokens.push(Token::new(
            TokenKind::StringLit { value, is_triple },
            Span::new(start, self.pos),
        ));
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }
}

const fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

const fn is_ident_start(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'_')
}

const fn is_ident_cont(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
}

/// UTF-8 codepoint length from the leading byte.
const fn utf8_codepoint_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1, // invalid lead byte; advance 1 to make progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_punctuation() {
        let stream = tokenize("{ } [ ] : , | < >");
        let kinds: Vec<_> = stream
            .tokens
            .iter()
            .map(|t| t.kind.clone())
            .filter(|k| !matches!(k, TokenKind::Eof))
            .collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Colon,
                TokenKind::Comma,
                TokenKind::Pipe,
                TokenKind::LAngle,
                TokenKind::RAngle,
            ]
        );
        assert!(stream.diagnostics.is_empty());
    }

    #[test]
    fn tokenizes_identifiers_and_keywords() {
        let stream = tokenize("slice rule_state cell published");
        let names: Vec<_> = stream
            .tokens
            .iter()
            .filter_map(|t| match &t.kind {
                TokenKind::Identifier(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["slice", "rule_state", "cell", "published"]);
    }

    #[test]
    fn tokenizes_string_literals() {
        let stream = tokenize(r#""hello \"world\"" "plain""#);
        let strings: Vec<_> = stream
            .tokens
            .iter()
            .filter_map(|t| match &t.kind {
                TokenKind::StringLit { value, is_triple } => Some((value.clone(), *is_triple)),
                _ => None,
            })
            .collect();
        assert_eq!(
            strings,
            vec![
                ("hello \"world\"".to_owned(), false),
                ("plain".to_owned(), false),
            ]
        );
        assert!(stream.diagnostics.is_empty());
    }

    #[test]
    fn tokenizes_triple_quoted_string() {
        let src = r#""""multi
line
string""""#;
        let stream = tokenize(src);
        let s = stream
            .tokens
            .iter()
            .find_map(|t| match &t.kind {
                TokenKind::StringLit { value, is_triple: true } => Some(value.clone()),
                _ => None,
            })
            .expect("expected triple-quoted string");
        assert_eq!(s, "multi\nline\nstring");
        assert!(stream.diagnostics.is_empty());
    }

    #[test]
    fn skips_comments() {
        let src = "// line comment\n/* block\n  comment */ slice";
        let stream = tokenize(src);
        let names: Vec<_> = stream
            .tokens
            .iter()
            .filter_map(|t| match &t.kind {
                TokenKind::Identifier(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["slice"]);
        assert!(stream.diagnostics.is_empty());
    }

    #[test]
    fn unterminated_string_emits_diagnostic_but_continues() {
        let stream = tokenize("\"unclosed\nslice next");
        assert!(
            stream.diagnostics.iter().any(|d| d.code == codes::E_LEX_UNTERMINATED_STRING),
            "expected unterminated-string diagnostic"
        );
        // Recovery: subsequent identifier still tokenized.
        assert!(stream.tokens.iter().any(|t| matches!(&t.kind, TokenKind::Identifier(s) if s == "next")));
    }

    #[test]
    fn arrow_token() {
        let stream = tokenize("a -> b");
        let kinds: Vec<_> = stream
            .tokens
            .iter()
            .map(|t| t.kind.clone())
            .filter(|k| !matches!(k, TokenKind::Eof))
            .collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Identifier("a".to_owned()),
                TokenKind::Arrow,
                TokenKind::Identifier("b".to_owned()),
            ]
        );
    }

    #[test]
    fn lexes_canonical_fixture_without_errors() {
        let src = include_str!("../tests/fixtures/rule_lifecycle_minimal.memspec");
        let stream = tokenize(src);
        assert!(
            stream.diagnostics.is_empty(),
            "expected no lex diagnostics on canonical fixture, got: {:?}",
            stream.diagnostics
        );
        // Sanity: at least one `slice` identifier and the EOF.
        assert!(stream.tokens.iter().any(|t| matches!(&t.kind, TokenKind::Identifier(s) if s == "slice")));
        assert!(stream.tokens.last().is_some_and(|t| matches!(t.kind, TokenKind::Eof)));
    }
}
