//! `.memspec` v0 parser.
//!
//! Three layers:
//! - [`lexer`]: source text → tokens with spans.
//! - [`parser`]: tokens → AST. Stubbed for v0-day-1; lands next.
//! - [`analysis`]: AST → diagnostics (structural + coherence + symmetric-failure).
//!   Stubbed for v0-day-1.
//!
//! Format reference: `docs/grammar-v0.md` at repo root.

pub mod analysis;
pub mod ast;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

pub use analysis::{AnalysisResult, analyze};
pub use diagnostic::{Diagnostic, Severity};
pub use lexer::tokenize;
pub use span::Span;
pub use token::{Token, TokenKind};
