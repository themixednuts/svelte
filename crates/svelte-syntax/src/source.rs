use camino::Utf8Path;

use crate::error::SourceLocation;
use crate::primitives::{SourceId, Span};

#[derive(Debug, Clone, Copy)]
/// Borrowed source text together with an identifier and optional filename.
pub struct SourceText<'src> {
    pub id: SourceId,
    pub text: &'src str,
    pub filename: Option<&'src Utf8Path>,
}

impl<'src> SourceText<'src> {
    /// Create a source view over a Svelte or CSS input string.
    pub fn new(id: SourceId, text: &'src str, filename: Option<&'src Utf8Path>) -> Self {
        Self { id, text, filename }
    }

    /// Return the source length in bytes.
    pub fn len(self) -> usize {
        self.text.len()
    }

    /// Return `true` when the source contains no bytes.
    pub fn is_empty(self) -> bool {
        self.text.is_empty()
    }

    /// Return a span that covers the full source.
    pub fn span_all(self) -> Span {
        Span::from_offsets(0, self.text.len()).unwrap_or(Span::EMPTY)
    }

    /// Borrow the substring covered by `span`.
    pub fn slice(self, span: Span) -> Option<&'src str> {
        self.text.get(span.start.as_usize()..span.end.as_usize())
    }

    /// Convert a byte offset into a UTF-16 code-unit offset.
    ///
    /// Carriage returns are ignored so CRLF input reports the same coordinates
    /// as Svelte's JavaScript compiler.
    pub fn utf16_offset(self, offset: usize) -> usize {
        let bounded = offset.min(self.text.len());
        self.text[..bounded]
            .chars()
            .filter(|&ch| ch != '\r')
            .map(char::len_utf16)
            .sum()
    }

    /// Convert a byte offset into a one-based line number and zero-based UTF-16 column.
    pub fn line_column_at_offset(self, offset: usize) -> (usize, usize) {
        let mut line = 1usize;
        let mut column = 0usize;
        let limit = offset.min(self.text.len());
        for ch in self.text[..limit].chars() {
            match ch {
                '\n' => {
                    line += 1;
                    column = 0;
                }
                '\r' => {}
                _ => {
                    column += ch.len_utf16();
                }
            }
        }
        (line, column)
    }

    /// Build a full source location for a byte offset.
    pub fn location_at_offset(self, offset: usize) -> SourceLocation {
        let (line, column) = self.line_column_at_offset(offset);
        SourceLocation {
            line,
            column,
            character: self.utf16_offset(offset),
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::{SourceId, SourceText};

    #[test]
    fn source_text_reports_utf16_locations() {
        let source = SourceText::new(
            SourceId::new(1),
            "a\n😀b",
            Some(Utf8Path::new("input.svelte")),
        );

        assert_eq!(source.utf16_offset(0), 0);
        assert_eq!(source.utf16_offset(2), 2);
        assert_eq!(source.utf16_offset("a\n😀".len()), 4);

        let location = source.location_at_offset("a\n😀".len());
        assert_eq!(location.line, 2);
        assert_eq!(location.column, 2);
        assert_eq!(location.character, 4);
    }

    #[test]
    fn source_text_normalizes_crlf_offsets() {
        let source = SourceText::new(
            SourceId::new(2),
            "a\r\nb\r\n😀c",
            Some(Utf8Path::new("input.svelte")),
        );

        let offset = "a\r\nb\r\n😀".len();
        assert_eq!(source.utf16_offset(offset), 6);

        let location = source.location_at_offset(offset);
        assert_eq!(location.line, 3);
        assert_eq!(location.column, 2);
        assert_eq!(location.character, 6);
    }
}
