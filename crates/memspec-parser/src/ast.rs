//! AST types for the `.memspec` v0 grammar.
//!
//! v0 uses a uniform block-item shape (`BlockDecl { kind, name, items }`)
//! borrowed in spirit from allium's parser. Semantic classification per
//! slot — required field validation, type-vocab checks, ref-resolution —
//! happens in the [`crate::analysis`] pass, not the parser.
//!
//! The parser is stubbed for v0-day-1; these types exist so the lexer
//! and CLI compile against a stable target.

use crate::span::Span;

/// A parsed `.memspec` file. Always exactly one slice for v0.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct File {
    pub span: Span,
    pub slice: Option<SliceDecl>,
}

/// Top-level `slice IDENT { ... }` block.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct SliceDecl {
    pub span: Span,
    pub name: Ident,
    /// `use "<path>" as <alias>` declarations at the top of the slice
    /// body. Empty for slices without imports.
    pub imports: Vec<Import>,
    pub items: Vec<BlockItem>,
}

/// `use "<path>" as <alias>` — cross-slice import.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct Import {
    pub span: Span,
    /// Relative path to the imported `.memspec` file. Resolved by the
    /// loader against the importing file's directory.
    pub path: String,
    pub path_span: Span,
    pub alias: Ident,
}

/// A named identifier with span (e.g. `rule_state`, `promote`, `s1_update`).
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct Ident {
    pub span: Span,
    pub name: String,
}

/// One item inside a block. Either a slot declaration (`cell`, `event`, …),
/// a sub-block (`step`, `meta`, `walk`, `cells_after`), or a key-value
/// field (`type:`, `mutates:`, `kind:`, …).
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockItem {
    Block(BlockDecl),
    Field(Field),
}

/// `keyword [name] { items }` — the uniform shape every named/anonymous
/// block lowers to.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct BlockDecl {
    pub span: Span,
    pub kind: Ident,
    /// `None` for anonymous blocks (`meta { ... }`, `cells_after_pre_rollback { ... }`).
    pub name: Option<BlockName>,
    pub items: Vec<BlockItem>,
}

/// A block's name slot. Most blocks are named by an identifier; `walk N`
/// uses an integer literal as its name.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockName {
    Ident(Ident),
    Int { span: Span, value: i64 },
}

impl BlockName {
    pub fn span(&self) -> Span {
        match self {
            Self::Ident(i) => i.span,
            Self::Int { span, .. } => *span,
        }
    }
}

/// `key: value` — typed semantically by the analyzer based on the
/// containing block's slot kind.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct Field {
    pub span: Span,
    pub key: Ident,
    pub value: FieldValue,
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldValue {
    Ident(Ident),
    String { span: Span, value: String, is_triple: bool },
    Int { span: Span, value: i64 },
    Bool { span: Span, value: bool },
    /// `[a, b, c]` or multi-line list. Elements are themselves field values.
    List { span: Span, items: Vec<FieldValue> },
    /// `{ key: value, key: value }` — used for `cells:` / `cells_after:` /
    /// `impl_hints:` maps.
    Map { span: Span, entries: Vec<MapEntry> },
    /// Type-application like `enum<draft | published | archived>` or
    /// `set<RuleChangelog>`. The opening identifier is the type
    /// constructor; `params` are the comma-or-pipe separated arguments.
    TypeApp {
        span: Span,
        head: Ident,
        params: Vec<FieldValue>,
        /// `true` if separated by `|` (alternation, e.g. enum variants);
        /// `false` if separated by `,` (positional, e.g. map params).
        alternation: bool,
    },
    /// `keyword(arg)` — for `enforced_by: event_handler(promote)`.
    Call {
        span: Span,
        head: Ident,
        args: Vec<FieldValue>,
    },
    /// `alias.id` — qualified reference to a declaration in an imported slice.
    QualifiedIdent {
        span: Span,
        alias: Ident,
        name: Ident,
    },
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct MapEntry {
    pub span: Span,
    pub key: Ident,
    pub value: FieldValue,
}

impl FieldValue {
    pub fn span(&self) -> Span {
        match self {
            Self::Ident(i) => i.span,
            Self::String { span, .. }
            | Self::Int { span, .. }
            | Self::Bool { span, .. }
            | Self::List { span, .. }
            | Self::Map { span, .. }
            | Self::TypeApp { span, .. }
            | Self::Call { span, .. }
            | Self::QualifiedIdent { span, .. } => *span,
        }
    }
}
