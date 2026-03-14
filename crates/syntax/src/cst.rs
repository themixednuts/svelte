use tree_sitter::{InputEdit, Node, Parser, Point, Tree};

use crate::error::CompileError;
use crate::primitives::{BytePos, Span};
use crate::source::SourceText;

/// Languages supported by the CST parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// The Svelte component language.
    Svelte,
}

/// A parsed tree-sitter document holding the source text, language, and
/// concrete syntax tree.
///
/// Use [`parse_svelte`] to create a `Document` from source text, or
/// [`CstParser`] for more control over parser reuse.
#[derive(Debug)]
pub struct Document<'src> {
    /// The language this document was parsed as.
    pub language: Language,
    /// The original source text.
    pub source: SourceText<'src>,
    /// The tree-sitter syntax tree.
    pub tree: Tree,
}

impl<'src> Document<'src> {
    /// Return the root tree-sitter node.
    pub fn root_node(&self) -> Node<'_> {
        self.tree.root_node()
    }

    /// Return the root node kind.
    pub fn root_kind(&self) -> &str {
        self.root_node().kind()
    }

    /// Return `true` if the CST contains parse errors.
    pub fn has_error(&self) -> bool {
        self.root_node().has_error()
    }

    /// Return the root node span in byte offsets.
    pub fn root_span(&self) -> Span {
        node_span(self.root_node())
    }

    /// Apply an edit to the stored tree so it can be reused for incremental reparsing.
    pub fn apply_edit(&mut self, edit: CstEdit) {
        self.tree.edit(&edit.into_input_edit());
    }

    /// Clone the tree for incremental parsing. The source text reference is
    /// preserved but the tree is cloned so `apply_edit` can be called on the
    /// copy without mutating the original.
    pub fn clone_for_incremental(&self) -> Document<'src> {
        Document {
            language: self.language,
            source: self.source,
            tree: self.tree.clone(),
        }
    }

    /// Return byte ranges that differ structurally between this document and a
    /// previously parsed document. Wraps [`Tree::changed_ranges`].
    pub fn changed_ranges(&self, old: &Document<'_>) -> Vec<std::ops::Range<usize>> {
        old.tree
            .changed_ranges(&self.tree)
            .map(|r| r.start_byte..r.end_byte)
            .collect()
    }
}

/// A row/column position in source text, used by [`CstEdit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CstPoint {
    /// Zero-based line number.
    pub row: usize,
    /// Zero-based byte column within the line.
    pub column: usize,
}

/// Describes a text edit for incremental reparsing.
///
/// Records the byte range that was replaced and the resulting positions after
/// the edit. Use the convenience constructors [`CstEdit::replace`],
/// [`CstEdit::insert`], and [`CstEdit::delete`] to build edits from the old
/// source text.
///
/// # Example
///
/// ```
/// use svelte_syntax::CstEdit;
///
/// let old = "<div>Hello</div>";
/// let edit = CstEdit::replace(old, 5, 10, "World");
///
/// assert_eq!(edit.start_byte, 5);
/// assert_eq!(edit.new_end_byte, 10); // 5 + "World".len()
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstEdit {
    /// Byte offset where the edit begins.
    pub start_byte: usize,
    /// Byte offset where the old text ended (before the edit).
    pub old_end_byte: usize,
    /// Byte offset where the new text ends (after the edit).
    pub new_end_byte: usize,
    /// Row/column position where the edit begins.
    pub start_position: CstPoint,
    /// Row/column position where the old text ended.
    pub old_end_position: CstPoint,
    /// Row/column position where the new text ends.
    pub new_end_position: CstPoint,
}

impl CstEdit {
    /// Create an edit that replaces `old_source[start_byte..old_end_byte]` with
    /// `new_text`. Positions are computed automatically from the old source.
    pub fn replace(
        old_source: &str,
        start_byte: usize,
        old_end_byte: usize,
        new_text: &str,
    ) -> Self {
        let start_position = byte_point_at_offset(old_source, start_byte);
        let old_end_position = byte_point_at_offset(old_source, old_end_byte);
        let new_end_byte = start_byte.saturating_add(new_text.len());
        let new_end_position = advance_point(start_position, new_text);

        Self {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position,
        }
    }

    /// Create an edit that inserts `new_text` at `start_byte` without removing
    /// any existing text.
    pub fn insert(old_source: &str, start_byte: usize, new_text: &str) -> Self {
        Self::replace(old_source, start_byte, start_byte, new_text)
    }

    /// Create an edit that deletes `old_source[start_byte..old_end_byte]`.
    pub fn delete(old_source: &str, start_byte: usize, old_end_byte: usize) -> Self {
        Self::replace(old_source, start_byte, old_end_byte, "")
    }

    fn into_input_edit(self) -> InputEdit {
        InputEdit {
            start_byte: self.start_byte,
            old_end_byte: self.old_end_byte,
            new_end_byte: self.new_end_byte,
            start_position: self.start_position.into_point(),
            old_end_position: self.old_end_position.into_point(),
            new_end_position: self.new_end_position.into_point(),
        }
    }
}

impl CstPoint {
    fn into_point(self) -> Point {
        Point {
            row: self.row,
            column: self.column,
        }
    }
}

/// Typestate marker for a parser before a language has been selected.
pub struct Unconfigured;
/// Typestate marker for a parser after a language has been selected.
pub struct Configured {
    language: Language,
}

/// Tree-sitter-backed CST parser with typestate for language selection.
///
/// Create a parser with [`CstParser::new`], configure it with
/// [`CstParser::configure`], then call [`parse`](CstParser::parse) or
/// [`parse_incremental`](CstParser::parse_incremental).
///
/// For a simpler one-shot API, use the free function [`parse_svelte`].
///
/// # Example
///
/// ```
/// use svelte_syntax::cst::{CstParser, Language};
/// use svelte_syntax::{SourceId, SourceText};
///
/// let mut parser = CstParser::new().configure(Language::Svelte)?;
/// let source = SourceText::new(SourceId::new(0), "<p>hi</p>", None);
/// let doc = parser.parse(source)?;
///
/// assert_eq!(doc.root_kind(), "document");
/// # Ok::<(), svelte_syntax::CompileError>(())
/// ```
pub struct CstParser<State> {
    parser: Parser,
    state: State,
}

impl CstParser<Unconfigured> {
    /// Create a parser with no configured language.
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            state: Unconfigured,
        }
    }

    /// Configure the parser for a supported language.
    pub fn configure(mut self, language: Language) -> Result<CstParser<Configured>, CompileError> {
        let ts_lang = match language {
            Language::Svelte => tree_sitter_svelte::language(),
        };

        self.parser
            .set_language(&ts_lang)
            .map_err(|_| CompileError::internal("failed to configure tree-sitter language"))?;

        Ok(CstParser {
            parser: self.parser,
            state: Configured { language },
        })
    }
}

impl Default for CstParser<Unconfigured> {
    fn default() -> Self {
        Self::new()
    }
}

impl CstParser<Configured> {
    /// Parse source text into a CST document.
    pub fn parse<'src>(
        &mut self,
        source: SourceText<'src>,
    ) -> Result<Document<'src>, CompileError> {
        let tree = self
            .parser
            .parse(source.text, None)
            .ok_or_else(|| CompileError::internal("tree-sitter parser returned no syntax tree"))?;

        Ok(Document {
            language: self.state.language,
            source,
            tree,
        })
    }

    /// Parse source text using a previous tree plus edit information for incremental reparsing.
    pub fn parse_incremental<'src>(
        &mut self,
        source: SourceText<'src>,
        previous: &Document<'_>,
        edit: CstEdit,
    ) -> Result<Document<'src>, CompileError> {
        let mut previous_tree = previous.tree.clone();
        previous_tree.edit(&edit.into_input_edit());

        let tree = self
            .parser
            .parse(source.text, Some(&previous_tree))
            .ok_or_else(|| CompileError::internal("tree-sitter parser returned no syntax tree"))?;

        Ok(Document {
            language: self.state.language,
            source,
            tree,
        })
    }
}

/// Parse Svelte source into a tree-sitter CST document.
///
/// This is the simplest way to obtain a concrete syntax tree. For parser
/// reuse across multiple files, use [`CstParser`] directly.
///
/// # Example
///
/// ```
/// use svelte_syntax::{SourceId, SourceText, parse_svelte};
///
/// let source = SourceText::new(SourceId::new(0), "<div>hello</div>", None);
/// let cst = parse_svelte(source)?;
///
/// assert_eq!(cst.root_kind(), "document");
/// # Ok::<(), svelte_syntax::CompileError>(())
/// ```
pub fn parse_svelte<'src>(source: SourceText<'src>) -> Result<Document<'src>, CompileError> {
    let mut parser = CstParser::new().configure(Language::Svelte)?;
    parser.parse(source)
}

/// Parse Svelte source using an already-edited old tree for incremental reparsing.
/// Unlike `parse_svelte_incremental`, this expects the caller to have already
/// called `apply_edit` on the old document.
pub fn parse_svelte_with_old_tree<'src>(
    source: SourceText<'src>,
    edited_old: &Document<'_>,
) -> Result<Document<'src>, CompileError> {
    let ts_lang = match edited_old.language {
        Language::Svelte => tree_sitter_svelte::language(),
    };
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .map_err(|_| CompileError::internal("failed to configure tree-sitter language"))?;
    let tree = parser
        .parse(source.text, Some(&edited_old.tree))
        .ok_or_else(|| CompileError::internal("tree-sitter parser returned no syntax tree"))?;
    Ok(Document {
        language: edited_old.language,
        source,
        tree,
    })
}

/// Parse Svelte source into a tree-sitter CST using a previous CST and edit for incremental reparsing.
pub fn parse_svelte_incremental<'src>(
    source: SourceText<'src>,
    previous: &Document<'_>,
    edit: CstEdit,
) -> Result<Document<'src>, CompileError> {
    let mut parser = CstParser::new().configure(Language::Svelte)?;
    parser.parse_incremental(source, previous, edit)
}

fn node_span(node: Node<'_>) -> Span {
    let start = byte_pos_saturating(node.start_byte());
    let end = byte_pos_saturating(node.end_byte());
    Span::new(start, end)
}

fn byte_pos_saturating(offset: usize) -> BytePos {
    u32::try_from(offset)
        .map(BytePos::from)
        .unwrap_or_else(|_| BytePos::from(u32::MAX))
}

fn byte_point_at_offset(source: &str, offset: usize) -> CstPoint {
    let bounded = offset.min(source.len());
    let mut row = 0usize;
    let mut column = 0usize;

    for byte in source.as_bytes().iter().take(bounded) {
        if *byte == b'\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }

    CstPoint { row, column }
}

fn advance_point(start: CstPoint, inserted_text: &str) -> CstPoint {
    let mut point = start;

    for byte in inserted_text.as_bytes() {
        if *byte == b'\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += 1;
        }
    }

    point
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::SourceId;

    #[test]
    fn parses_svelte_cst_document() {
        let source = SourceText::new(SourceId::new(1), "<div>Hello</div>", None);
        let cst = parse_svelte(source).expect("expected tree-sitter CST parse to succeed");

        assert!(!cst.root_kind().is_empty());
        assert!(cst.root_span().end.as_usize() >= cst.source.len());
    }

    #[test]
    fn cst_contains_attribute_nodes() {
        let source = SourceText::new(SourceId::new(2), "<div class='foo'></div>", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("(attribute"));
        assert!(sexp.contains("(attribute_name"));
    }

    #[test]
    fn cst_style_directive_shape() {
        let source = SourceText::new(SourceId::new(3), "<div style:color={myColor}></div>", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("attribute_directive"));
        assert!(sexp.contains("attribute_identifier"));
    }

    #[test]
    fn cst_if_block_shape() {
        let source = SourceText::new(SourceId::new(4), "{#if foo}bar{/if}", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("if_block"));
        assert!(sexp.contains("block_end"));
    }

    #[test]
    fn cst_breaks_unterminated_tags_before_block_branches() {
        let source = SourceText::new(
            SourceId::new(5),
            "{#if true}\n\t<input>\n{:else}\n{/if}\n\n{#await true}\n\t<input>\n{:then f}\n{/await}",
            None,
        );
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.matches("(else_clause").count() + sexp.matches("(await_branch").count() >= 2);
    }

    #[test]
    fn cst_directive_and_debug_tag_shapes() {
        let source = SourceText::new(
            SourceId::new(6),
            "<div let:x style:color={c} transition:fade={t} animate:flip={a} use:act={u}></div>{@debug x, y}",
            None,
        );
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("attribute_name"));
        assert!(sexp.contains("debug_tag"));
        assert!(sexp.contains("expression_value"));
    }

    #[test]
    fn cst_malformed_snippet_headers_report_error_shape() {
        let source = SourceText::new(SourceId::new(7), "{#snippet children()hi{/snippet}", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();
        assert!(
            cst.has_error(),
            "expected malformed snippet header CST error"
        );
        assert!(sexp.contains("(snippet_name"));

        let source = SourceText::new(SourceId::new(8), "{#snippet children(hi{/snippet}", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();
        assert!(sexp.contains("(snippet_name"));
        assert!(sexp.contains("(snippet_parameters"));
    }

    #[test]
    fn incremental_parse_matches_fresh_parse_after_insert() {
        let before_text = "<div>Hello</div>";
        let after_text = "<div>Hello {name}</div>";
        let before = SourceText::new(SourceId::new(9), before_text, None);
        let after = SourceText::new(SourceId::new(10), after_text, None);

        let mut parser = CstParser::new()
            .configure(Language::Svelte)
            .expect("parser");
        let previous = parser.parse(before).expect("initial parse");
        let edit = CstEdit::insert(before_text, "<div>Hello".len(), " {name}");

        let incremental = parser
            .parse_incremental(after, &previous, edit)
            .expect("incremental parse");
        let fresh = parse_svelte(after).expect("fresh parse");

        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
    }

    #[test]
    fn document_apply_edit_keeps_tree_reusable() {
        let before_text = "<div>Hello</div>";
        let after_text = "<div>Hi</div>";
        let before = SourceText::new(SourceId::new(11), before_text, None);
        let after = SourceText::new(SourceId::new(12), after_text, None);

        let mut parser = CstParser::new()
            .configure(Language::Svelte)
            .expect("parser");
        let mut previous = parser.parse(before).expect("initial parse");
        let edit = CstEdit::replace(before_text, "<div>".len(), "<div>Hello".len(), "Hi");
        previous.apply_edit(edit.clone());

        let incremental = parser
            .parse_incremental(after, &previous, edit)
            .expect("incremental parse");
        let fresh = parse_svelte(after).expect("fresh parse");

        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
    }
}
