//! Token kinds produced by the lexer.
//!
//! Predicate expression syntax (`==`, `AND`, `forall`, …) is **not** lexed
//! by v0 — predicates appear inside string literals (`derivation:`,
//! `invariant:`, `assertion:`) and are parsed by adapters or the
//! scrutinizer if needed. The .memspec lexer only handles the outer
//! block-item structure.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // Punctuation
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    LParen,    // (
    RParen,    // )
    LAngle,    // <
    RAngle,    // >
    Comma,     // ,
    Colon,     // :
    Semi,      // ;
    Dot,       // .
    Pipe,      // |
    Eq,        // =
    Arrow,     // ->

    // Literals
    Identifier(String),
    IntLit(i64),
    /// String literal, content with escapes already resolved.
    /// `is_triple` distinguishes `"..."` from `"""..."""`.
    StringLit { value: String, is_triple: bool },

    /// End of input. One emitted per token stream.
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub const fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn is_eof(&self) -> bool {
        matches!(self.kind, TokenKind::Eof)
    }
}

impl TokenKind {
    /// Display form for error messages and debug output.
    pub fn label(&self) -> &'static str {
        match self {
            Self::LBrace => "`{`",
            Self::RBrace => "`}`",
            Self::LBracket => "`[`",
            Self::RBracket => "`]`",
            Self::LParen => "`(`",
            Self::RParen => "`)`",
            Self::LAngle => "`<`",
            Self::RAngle => "`>`",
            Self::Comma => "`,`",
            Self::Colon => "`:`",
            Self::Semi => "`;`",
            Self::Dot => "`.`",
            Self::Pipe => "`|`",
            Self::Eq => "`=`",
            Self::Arrow => "`->`",
            Self::Identifier(_) => "identifier",
            Self::IntLit(_) => "integer literal",
            Self::StringLit { .. } => "string literal",
            Self::Eof => "end of input",
        }
    }
}
