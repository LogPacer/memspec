//! Byte-offset spans for tokens, AST nodes, and diagnostics.
//!
//! Spans are byte-ranges into the original source string. Line/column
//! resolution is done lazily by [`SourceMap::line_col`] to keep tokens
//! cheap.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, schemars::JsonSchema)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const DUMMY: Self = Self { start: 0, end: 0 };

    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Smallest span containing both `self` and `other`.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Slice the source string this span points into.
    #[must_use]
    pub fn slice<'s>(self, source: &'s str) -> &'s str {
        &source[self.start..self.end]
    }
}

/// 1-indexed line/column pair (display form).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Lazy line-offset index over a single source string.
///
/// Built once per file; resolves byte-offset → `LineCol` in `O(log lines)`.
pub struct SourceMap<'s> {
    source: &'s str,
    line_starts: Vec<usize>,
}

impl<'s> SourceMap<'s> {
    pub fn new(source: &'s str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { source, line_starts }
    }

    pub fn source(&self) -> &'s str {
        self.source
    }

    /// Convert a byte offset to a 1-indexed (line, column) pair.
    pub fn line_col(&self, offset: usize) -> LineCol {
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line_idx];
        let col_bytes = offset - line_start;
        // Column is 1-indexed by char (not byte). Walk the line slice.
        let line_slice = &self.source[line_start..offset.min(self.source.len())];
        let col_chars: u32 = line_slice.chars().count().try_into().unwrap_or(u32::MAX);
        LineCol {
            line: u32::try_from(line_idx + 1).unwrap_or(u32::MAX),
            col: col_chars + 1,
        }
        .with_min_col(col_bytes)
    }
}

impl LineCol {
    fn with_min_col(mut self, _byte_col: usize) -> Self {
        if self.col == 0 {
            self.col = 1;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_join_and_slice() {
        let s = Span::new(0, 5);
        let t = Span::new(3, 10);
        assert_eq!(s.join(t), Span::new(0, 10));
        let src = "hello world";
        assert_eq!(s.slice(src), "hello");
    }

    #[test]
    fn source_map_line_col() {
        let src = "abc\ndefg\nhi";
        let map = SourceMap::new(src);
        assert_eq!(map.line_col(0), LineCol { line: 1, col: 1 });
        assert_eq!(map.line_col(2), LineCol { line: 1, col: 3 });
        assert_eq!(map.line_col(4), LineCol { line: 2, col: 1 });
        assert_eq!(map.line_col(9), LineCol { line: 3, col: 1 });
    }
}
