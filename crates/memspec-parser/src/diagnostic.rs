//! Diagnostics produced by the lexer, parser, and analyzer.
//!
//! Codes are namespaced strings (`memspec/E####`, `memspec/W####`,
//! `memspec/I####`) — never reused once allocated. The CLI's `--json`
//! output shape is built from [`Diagnostic`].

use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span,
            hint: None,
        }
    }

    pub fn warning(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            span,
            hint: None,
        }
    }

    pub fn info(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Info,
            message: message.into(),
            span,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

// Lexer diagnostic codes (E0001 – E0099 reserved for lex errors).
// Parser diagnostic codes (E0100 – E0199 reserved for parse errors).
// Analyzer diagnostic codes (E0200+ reserved for semantic checks).
pub mod codes {
    // Lexer
    pub const E_LEX_UNTERMINATED_STRING: &str = "memspec/E0001";
    pub const E_LEX_UNTERMINATED_BLOCK_COMMENT: &str = "memspec/E0002";
    pub const E_LEX_UNEXPECTED_CHAR: &str = "memspec/E0003";
    pub const E_LEX_INVALID_ESCAPE: &str = "memspec/E0004";
    pub const E_LEX_INVALID_INT: &str = "memspec/E0005";
    pub const E_LEX_NOT_A_MEMSPEC: &str = "memspec/E0006";

    // Parser
    pub const E_PARSE_EXPECTED_TOKEN: &str = "memspec/E0100";
    pub const E_PARSE_EXPECTED_IDENT: &str = "memspec/E0101";
    pub const E_PARSE_EXPECTED_FIELD_VALUE: &str = "memspec/E0102";
    pub const E_PARSE_UNCLOSED_BLOCK: &str = "memspec/E0103";
    pub const E_PARSE_UNEXPECTED_TOKEN: &str = "memspec/E0104";
    pub const E_PARSE_EMPTY_FILE: &str = "memspec/E0105";
    pub const E_PARSE_MULTIPLE_SLICES: &str = "memspec/E0106";
    pub const E_PARSE_EXPECTED_SLICE: &str = "memspec/E0107";
    pub const E_PARSE_USE_AFTER_DECL: &str = "memspec/E0108";
    pub const E_PARSE_EXPECTED_AS: &str = "memspec/E0109";
    pub const E_PARSE_DUPLICATE_IMPORT_ALIAS: &str = "memspec/E0110";

    // Analyzer — structural completeness (E0200 – E0249)
    pub const E_STRUCT_MISSING_FIELD: &str = "memspec/E0200";
    pub const E_STRUCT_EMPTY_SLICE: &str = "memspec/E0201";
    pub const E_STRUCT_EVENT_NO_STEPS: &str = "memspec/E0202";
    pub const E_STRUCT_FORBIDDEN_STATE_BODY: &str = "memspec/E0203";
    pub const I_STRUCT_KILL_TEST_TODO: &str = "memspec/I0220";

    // Analyzer — coherence (E0250 – E0299)
    pub const E_COH_DUPLICATE_ID: &str = "memspec/E0250";
    pub const E_COH_DUPLICATE_STEP_ID: &str = "memspec/E0251";
    pub const E_COH_UNRESOLVED_CELL_REF: &str = "memspec/E0252";
    pub const E_COH_UNRESOLVED_EVENT_REF: &str = "memspec/E0253";
    pub const E_COH_UNRESOLVED_STEP_REF: &str = "memspec/E0254";
    pub const E_COH_UNRESOLVED_FORBIDDEN_REF: &str = "memspec/E0255";
    pub const E_COH_UNRESOLVED_KILL_TEST_REF: &str = "memspec/E0256";
    pub const E_COH_BIPARTITE_MISMATCH: &str = "memspec/E0257";
    pub const E_COH_DERIVATION_CYCLE: &str = "memspec/E0258";
    pub const E_COH_STEP_NOT_FALLIBLE: &str = "memspec/E0259";

    // Analyzer — coherence warnings (W0270 – W0299)
    pub const W_COH_UNUSED_CELL: &str = "memspec/W0270";
    pub const W_COH_EVENT_EMPTY_MUTATES: &str = "memspec/W0271";
    pub const W_COH_REDUNDANT_KILL_TEST: &str = "memspec/W0272";

    // Analyzer — symmetric-failure (E0300 – E0349)
    pub const E_SYMFAIL_MISSING_POST_FAILURE: &str = "memspec/E0300";
    pub const E_SYMFAIL_MISSING_ROLLBACK_PAIR: &str = "memspec/E0301";

    // Loader / cross-slice imports (E0400 – E0449)
    pub const E_LOADER_NOT_FOUND: &str = "memspec/E0400";
    pub const E_LOADER_IMPORT_CYCLE: &str = "memspec/E0401";
    pub const E_LOADER_UNRESOLVED_ALIAS: &str = "memspec/E0402";
    pub const E_LOADER_QUALIFIED_REF_UNRESOLVED: &str = "memspec/E0403";

    // Composition warnings (W0273 – W0299)
    pub const W_COMP_UNUSED_IMPORT: &str = "memspec/W0273";
    pub const W_COMP_DUPLICATE_IMPORT_TARGET: &str = "memspec/W0274";
    pub const W_COMP_IMPORTED_ID_SHADOWED: &str = "memspec/W0275";
}
