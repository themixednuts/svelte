use camino::Utf8Path;

use crate::primitives::{SourceId, Span};

#[derive(Debug, Clone, Copy)]
pub struct SourceText<'src> {
    pub id: SourceId,
    pub text: &'src str,
    pub filename: Option<&'src Utf8Path>,
}

impl<'src> SourceText<'src> {
    pub fn new(id: SourceId, text: &'src str, filename: Option<&'src Utf8Path>) -> Self {
        Self { id, text, filename }
    }

    pub fn len(self) -> usize {
        self.text.len()
    }

    pub fn is_empty(self) -> bool {
        self.text.is_empty()
    }

    pub fn span_all(self) -> Span {
        Span::from_offsets(0, self.text.len()).unwrap_or(Span::EMPTY)
    }

    pub fn slice(self, span: Span) -> Option<&'src str> {
        self.text.get(span.start.as_usize()..span.end.as_usize())
    }
}
