use std::sync::Arc;

pub use svelte_syntax::SourceText;

use crate::error::{CompileError, DiagnosticKind};
use crate::SourceId;

/// Byte range in source text.
///
/// Replaces bare `(usize, usize)` tuples throughout validation and codegen,
/// giving field names (`start`, `end`) instead of positional access (`.0`, `.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

impl SourceSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Convert an OXC span (u32 offsets) into a `SourceSpan`.
    pub fn from_oxc(span: oxc_span::Span) -> Self {
        Self {
            start: span.start as usize,
            end: span.end as usize,
        }
    }

    /// Shift both start and end by a fixed byte offset.
    pub fn offset(self, offset: usize) -> Self {
        Self {
            start: self.start + offset,
            end: self.end + offset,
        }
    }

    /// Build a `CompileError` from this span, the full source text, and a diagnostic kind.
    pub fn to_compile_error(self, source: &str, kind: DiagnosticKind) -> CompileError {
        kind.to_compile_error_in(
            SourceText::new(SourceId::new(0), source, None),
            self.start,
            self.end,
        )
    }
}

impl From<oxc_span::Span> for SourceSpan {
    fn from(span: oxc_span::Span) -> Self {
        Self::from_oxc(span)
    }
}

impl From<(usize, usize)> for SourceSpan {
    fn from((start, end): (usize, usize)) -> Self {
        Self { start, end }
    }
}

/// A named identifier with its source location.
///
/// Replaces the `(Arc<str>, usize, usize)` triple returned by global-reference
/// and rune-name finders.
#[derive(Debug, Clone)]
pub(crate) struct NamedSpan {
    pub name: Arc<str>,
    pub span: SourceSpan,
}

impl NamedSpan {
    pub fn new(name: Arc<str>, span: SourceSpan) -> Self {
        Self { name, span }
    }
}
