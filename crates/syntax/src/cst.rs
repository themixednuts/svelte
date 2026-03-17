use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use self_cell::self_cell;
use tree_sitter::{InputEdit, Node, Parser, Point, Tree, TreeCursor};

use crate::ast::modern::Expression;
use crate::error::CompileError;
use crate::parse::is_component_name;
use crate::primitives::{BytePos, Span};
use crate::source::SourceText;

/// Languages supported by the CST parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// The Svelte component language.
    Svelte,
}

// ---------------------------------------------------------------------------
// ExpressionCache — pre-parsed OXC expressions indexed by byte offset
// ---------------------------------------------------------------------------

/// Cache of pre-parsed OXC expressions, keyed by node start byte offset.
#[derive(Debug, Default)]
pub struct ExpressionCache {
    expressions: HashMap<usize, Expression>,
}

impl ExpressionCache {
    /// Build the cache by walking the tree and parsing all expression nodes.
    pub fn from_tree(source: &str, tree: &Tree) -> Self {
        let mut cache = Self::default();
        cache.walk_and_parse(source, tree.root_node());
        cache
    }

    /// Look up a pre-parsed expression by its node's start byte offset.
    pub fn get(&self, start_byte: usize) -> Option<&Expression> {
        self.expressions.get(&start_byte)
    }

    /// Number of cached expressions.
    pub fn len(&self) -> usize {
        self.expressions.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.expressions.is_empty()
    }

    fn walk_and_parse(&mut self, source: &str, node: Node<'_>) {
        match node.kind() {
            "expression" | "expression_value" => {
                self.parse_and_insert(source, node);
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk_and_parse(source, child);
        }
    }

    fn parse_and_insert(&mut self, source: &str, node: Node<'_>) {
        if let Some(expr) = parse_expression_from_node(source, node) {
            self.expressions.insert(node.start_byte(), expr);
        }
    }
}

/// Parse a tree-sitter expression/expression_value node into an OXC `Expression`.
fn parse_expression_from_node(source: &str, node: Node<'_>) -> Option<Expression> {
    let raw = node.utf8_text(source.as_bytes()).ok()?;

    // For expression nodes with `{...}` delimiters, extract the inner content
    let (text, start_byte) = if node.kind() == "expression" {
        if let Some(content) = node.child_by_field_name("content") {
            let t = content.utf8_text(source.as_bytes()).ok()?;
            (t, content.start_byte())
        } else if raw.len() >= 2 && raw.starts_with('{') && raw.ends_with('}') {
            (&raw[1..raw.len() - 1], node.start_byte() + 1)
        } else {
            (raw, node.start_byte())
        }
    } else {
        (raw, node.start_byte())
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading = text.find(trimmed).unwrap_or(0);
    let abs = start_byte + leading;
    let (line, column) = crate::parse::line_column_at_offset(source, abs);
    crate::parse::parse_modern_expression_from_text(trimmed, abs, line, column)
}

// ---------------------------------------------------------------------------
// ParsedDocument — self-contained owning document (source + tree + expressions)
// ---------------------------------------------------------------------------

struct ParsedDocumentOwner {
    source: Arc<str>,
    tree: Tree,
    expressions: ExpressionCache,
}

/// The dependent data borrowing from the owner.
struct ParsedDocumentDependent<'a> {
    root: Root<'a>,
}

self_cell! {
    /// A fully parsed, self-contained document.
    ///
    /// Owns the source text, tree-sitter tree, and pre-parsed expression cache.
    /// Provides zero-copy wrapper access through `root()`.
    pub struct ParsedDocument {
        owner: ParsedDocumentOwner,

        #[covariant]
        dependent: ParsedDocumentDependent,
    }
}

impl std::fmt::Debug for ParsedDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedDocument")
            .field("source_len", &self.source().len())
            .field("expressions", &self.expressions().len())
            .finish()
    }
}

// SAFETY: ParsedDocument owns all its data (Arc<str>, Tree, ExpressionCache).
// The !Send/!Sync comes from self_cell's self-referential borrow, but the
// underlying data is fully owned and not shared across threads unsafely.
unsafe impl Send for ParsedDocument {}
unsafe impl Sync for ParsedDocument {}

impl ParsedDocument {
    /// Parse source into a fully self-contained document.
    pub fn parse(source: &str) -> Result<Self, CompileError> {
        let tree = {
            let mut parser = CstParser::new().configure(Language::Svelte)?;
            let st = SourceText::new(crate::primitives::SourceId::new(0), source, None);
            let doc = parser.parse(st)?;
            doc.tree
        };
        let expressions = ExpressionCache::from_tree(source, &tree);
        let source_arc: Arc<str> = Arc::from(source);

        Ok(ParsedDocument::new(
            ParsedDocumentOwner {
                source: source_arc,
                tree,
                expressions,
            },
            |owner| ParsedDocumentDependent {
                root: Root::new(&owner.source, owner.tree.root_node()),
            },
        ))
    }

    /// The source text.
    pub fn source(&self) -> &str {
        &self.borrow_owner().source
    }

    /// The root wrapper node.
    pub fn root(&self) -> &Root<'_> {
        &self.borrow_dependent().root
    }

    /// The pre-parsed expression cache.
    pub fn expressions(&self) -> &ExpressionCache {
        &self.borrow_owner().expressions
    }

    /// The underlying tree-sitter tree.
    pub fn tree(&self) -> &Tree {
        &self.borrow_owner().tree
    }
}

// ---------------------------------------------------------------------------
// Document — legacy borrowed document (kept for incremental parsing support)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Thin wrapper types — zero-copy views over tree-sitter nodes
// ---------------------------------------------------------------------------

/// Extract the text of a tree-sitter node from source.
fn node_text<'src>(source: &'src str, node: Node<'_>) -> &'src str {
    &source[node.start_byte()..node.end_byte()]
}

/// A thin wrapper around a tree-sitter node and source text.
/// All wrapper types share this layout: `(&str, Node)`.
macro_rules! define_wrapper {
    ($($(#[$meta:meta])* $name:ident),* $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Clone, Copy)]
            pub struct $name<'src> {
                source: &'src str,
                node: Node<'src>,
            }

            impl<'src> $name<'src> {
                /// Create from source text and a matching tree-sitter node.
                pub fn new(source: &'src str, node: Node<'src>) -> Self {
                    Self { source, node }
                }

                /// The underlying tree-sitter node.
                pub fn ts_node(&self) -> Node<'src> {
                    self.node
                }

                /// Byte offset of the start of this node.
                pub fn start(&self) -> usize {
                    self.node.start_byte()
                }

                /// Byte offset of the end of this node.
                pub fn end(&self) -> usize {
                    self.node.end_byte()
                }

                /// Span covering this node.
                pub fn span(&self) -> Span {
                    node_span(self.node)
                }

                /// The raw source text of this node.
                pub fn text(&self) -> &'src str {
                    node_text(self.source, self.node)
                }

                /// Whether this node has parse errors.
                pub fn has_error(&self) -> bool {
                    self.node.has_error()
                }
            }

            impl std::fmt::Debug for $name<'_> {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.debug_struct(stringify!($name))
                        .field("kind", &self.node.kind())
                        .field("range", &(self.start()..self.end()))
                        .finish()
                }
            }
        )*
    };
}

define_wrapper!(
    /// Root document node.
    Root,
    /// An HTML/Svelte element.
    Element,
    /// A text node.
    TextNode,
    /// An HTML comment.
    CommentNode,
    /// `{#if}...{:else if}...{:else}...{/if}`
    IfBlock,
    /// `{#each items as item}...{/each}`
    EachBlock,
    /// `{#await promise}...{:then}...{:catch}...{/await}`
    AwaitBlock,
    /// `{#key expression}...{/key}`
    KeyBlock,
    /// `{#snippet name(params)}...{/snippet}`
    SnippetBlock,
    /// `{expression}`
    ExpressionTag,
    /// `{@html expression}`
    HtmlTag,
    /// `{@const assignment}`
    ConstTag,
    /// `{@debug vars}`
    DebugTag,
    /// `{@render snippet()}`
    RenderTag,
    /// `{@attach handler}`
    AttachTag,
    /// An attribute on an element.
    AttributeNode,
    /// A start tag `<name ...>`.
    StartTag,
);

// --- Root accessors ---

impl<'src> Root<'src> {
    /// Iterate over top-level child nodes as `TemplateNode`s.
    pub fn children(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }
}

// --- Element accessors ---

impl<'src> Element<'src> {
    /// The element's tag name (e.g., "div", "Button", "svelte:head").
    pub fn name(&self) -> &'src str {
        // First child is start_tag or self_closing_tag
        let tag = self.node.child(0).expect("element must have a tag");
        if let Some(name_node) = tag.child_by_field_name("name") {
            node_text(self.source, name_node)
        } else {
            // Fallback: find first tag_name child
            let mut cursor = tag.walk();
            for child in tag.children(&mut cursor) {
                if child.kind() == "tag_name" {
                    return node_text(self.source, child);
                }
            }
            ""
        }
    }

    /// Whether this element uses self-closing syntax (`<br />`).
    pub fn is_self_closing(&self) -> bool {
        self.node.child(0)
            .is_some_and(|tag| tag.kind() == "self_closing_tag")
    }

    /// Whether this element has an explicit end tag.
    pub fn has_end_tag(&self) -> bool {
        let mut cursor = self.node.walk();
        self.node.children(&mut cursor).any(|c| c.kind() == "end_tag")
    }

    /// The start tag node.
    pub fn start_tag(&self) -> Option<StartTag<'src>> {
        let first = self.node.child(0)?;
        match first.kind() {
            "start_tag" | "self_closing_tag" => Some(StartTag::new(self.source, first)),
            _ => None,
        }
    }

    /// Iterate over attributes on this element.
    pub fn attributes(&self) -> AttributeIter<'src> {
        let tag = self.node.child(0).expect("element must have a tag");
        AttributeIter {
            source: self.source,
            cursor: tag.walk(),
            started: false,
        }
    }

    /// Iterate over child content nodes (between start and end tags).
    pub fn children(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }

    /// Whether this element's name indicates a component.
    pub fn is_component(&self) -> bool {
        is_component_name(self.name())
    }

    /// Classify this element into a `TemplateNode` variant based on its name.
    pub fn classify(&self) -> TemplateNode<'src> {
        classify_element(self.source, self.node)
    }
}

// --- StartTag accessors ---

impl<'src> StartTag<'src> {
    /// The tag name.
    pub fn name(&self) -> &'src str {
        if let Some(name_node) = self.node.child_by_field_name("name") {
            node_text(self.source, name_node)
        } else {
            ""
        }
    }

    /// Iterate over attributes on this start tag.
    pub fn attributes(&self) -> AttributeIter<'src> {
        AttributeIter {
            source: self.source,
            cursor: self.node.walk(),
            started: false,
        }
    }
}

// --- TextNode accessors ---

impl<'src> TextNode<'src> {
    /// The raw text content.
    pub fn raw(&self) -> &'src str {
        node_text(self.source, self.node)
    }

    /// Decoded text content (HTML entities resolved).
    pub fn data(&self) -> Cow<'src, str> {
        // For now, return raw. Entity decoding can be added later.
        Cow::Borrowed(self.raw())
    }
}

// --- CommentNode accessors ---

impl<'src> CommentNode<'src> {
    /// The comment content (without `<!--` and `-->`).
    pub fn data(&self) -> &'src str {
        let raw = self.raw();
        raw.strip_prefix("<!--")
            .and_then(|s| s.strip_suffix("-->"))
            .unwrap_or(raw)
    }

    fn raw(&self) -> &'src str {
        node_text(self.source, self.node)
    }
}

// --- Block accessors ---

impl<'src> IfBlock<'src> {
    /// The expression node for the test condition.
    pub fn test_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("expression")
    }

    /// The raw test expression text.
    pub fn test_text(&self) -> &'src str {
        self.test_node()
            .map(|n| node_text(self.source, n))
            .unwrap_or("")
    }

    /// The pre-parsed test expression from the cache.
    pub fn test_expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.test_node().and_then(|n| cache.get(n.start_byte()))
    }

    /// Iterate over consequent (then-branch) child nodes.
    pub fn consequent(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }

    /// The else clause, if any. Returns either an else fragment or an else-if block.
    pub fn alternate(&self) -> Option<Alternate<'src>> {
        if self.node.kind() == "else_if_clause" {
            // When wrapping an else_if_clause, the alternate is the next
            // sibling clause from the parent if_block, not a child of this node.
            let mut sibling = self.node.next_named_sibling();
            while let Some(s) = sibling {
                match s.kind() {
                    "else_clause" => return Some(Alternate::Else(ElseClause::new(self.source, s))),
                    "else_if_clause" => return Some(Alternate::ElseIf(IfBlock::new(self.source, s))),
                    _ => {}
                }
                sibling = s.next_named_sibling();
            }
            return None;
        }
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            match child.kind() {
                "else_clause" => return Some(Alternate::Else(ElseClause::new(self.source, child))),
                "else_if_clause" => return Some(Alternate::ElseIf(IfBlock::new(self.source, child))),
                _ => {}
            }
        }
        None
    }
}

impl<'src> EachBlock<'src> {
    /// The iterable expression node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("expression")
    }

    /// The pre-parsed iterable expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }

    /// The binding pattern node.
    pub fn binding_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("binding")
    }

    /// The key expression node, if any.
    pub fn key_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("key")
    }

    /// The index identifier node, if any.
    pub fn index_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("index")
    }

    /// Iterate over body child nodes.
    pub fn body(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }

    /// The else (fallback) clause, if any.
    pub fn fallback(&self) -> Option<ElseClause<'src>> {
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            if child.kind() == "else_clause" {
                return Some(ElseClause::new(self.source, child));
            }
        }
        None
    }
}

impl<'src> AwaitBlock<'src> {
    /// The promise expression node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("expression")
    }

    /// The pre-parsed promise expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }

    /// The pending (loading) body, if any.
    pub fn pending(&self) -> Option<ChildIter<'src>> {
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            if child.kind() == "await_pending" {
                return Some(ChildIter::new(self.source, child));
            }
        }
        None
    }

    /// The then branch children, if any.
    pub fn then_children(&self) -> Option<ChildIter<'src>> {
        // Check for shorthand form: {#await expr then value}...{/await}
        if let Some(shorthand) = self.node.child_by_field_name("shorthand") {
            if node_text(self.source, shorthand) == "then" {
                if let Some(children) = self.node.child_by_field_name("shorthand_children") {
                    return Some(ChildIter::new(self.source, children));
                }
            }
        }
        self.branch_children("then")
    }

    /// The catch branch children, if any.
    pub fn catch_children(&self) -> Option<ChildIter<'src>> {
        // Check for shorthand form: {#await expr catch error}...{/await}
        if let Some(shorthand) = self.node.child_by_field_name("shorthand") {
            if node_text(self.source, shorthand) == "catch" {
                if let Some(children) = self.node.child_by_field_name("shorthand_children") {
                    return Some(ChildIter::new(self.source, children));
                }
            }
        }
        self.branch_children("catch")
    }

    /// The then binding pattern text (e.g., "value" in `{:then value}`).
    pub fn then_binding_text(&self) -> Option<&'src str> {
        // Shorthand form: {#await expr then value}...{/await}
        if let Some(shorthand) = self.node.child_by_field_name("shorthand") {
            if node_text(self.source, shorthand) == "then" {
                return self.node.child_by_field_name("binding")
                    .map(|n| node_text(self.source, n));
            }
        }
        self.branch_binding("then")
    }

    /// The catch binding pattern text (e.g., "error" in `{:catch error}`).
    pub fn catch_binding_text(&self) -> Option<&'src str> {
        // Shorthand form: {#await expr catch error}...{/await}
        if let Some(shorthand) = self.node.child_by_field_name("shorthand") {
            if node_text(self.source, shorthand) == "catch" {
                return self.node.child_by_field_name("binding")
                    .map(|n| node_text(self.source, n));
            }
        }
        self.branch_binding("catch")
    }

    fn branch_binding(&self, kind_name: &str) -> Option<&'src str> {
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            if child.kind() == "await_branch" {
                let mut inner = child.walk();
                for c in child.children(&mut inner) {
                    if c.kind() == "branch_kind" && node_text(self.source, c) == kind_name {
                        return child.child_by_field_name("binding")
                            .or_else(|| child.child_by_field_name("expression"))
                            .map(|n| node_text(self.source, n));
                    }
                }
            }
        }
        None
    }

    fn branch_children(&self, kind_name: &str) -> Option<ChildIter<'src>> {
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            if child.kind() == "await_branch" {
                // Check branch_kind
                let mut inner = child.walk();
                for c in child.children(&mut inner) {
                    if c.kind() == "branch_kind" && node_text(self.source, c) == kind_name {
                        // Find await_branch_children
                        let mut inner2 = child.walk();
                        for c2 in child.children(&mut inner2) {
                            if c2.kind() == "await_branch_children" {
                                return Some(ChildIter::new(self.source, c2));
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

impl<'src> KeyBlock<'src> {
    /// The key expression node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("expression")
    }

    /// The pre-parsed key expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }

    /// Iterate over body child nodes.
    pub fn body(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }
}

impl<'src> SnippetBlock<'src> {
    /// The snippet name.
    pub fn name(&self) -> &'src str {
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            if child.kind() == "snippet_name" {
                return node_text(self.source, child);
            }
        }
        ""
    }

    /// The snippet parameters node, if any.
    pub fn parameters_node(&self) -> Option<Node<'src>> {
        let mut cursor = self.node.walk();
        for child in self.node.children(&mut cursor) {
            if child.kind() == "snippet_parameters" {
                return Some(child);
            }
        }
        None
    }

    /// The raw text of the parameters (e.g., "a, b" from `{#snippet name(a, b)}`).
    pub fn parameters_text(&self) -> Option<&'src str> {
        self.parameters_node().map(|n| node_text(self.source, n))
    }

    /// Iterate over body child nodes.
    pub fn body(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }
}

// --- Tag accessors ---

impl<'src> ExpressionTag<'src> {
    /// The expression content node (js or ts).
    pub fn content_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("content")
    }

    /// The raw expression text.
    pub fn content_text(&self) -> &'src str {
        self.content_node()
            .map(|n| node_text(self.source, n))
            .unwrap_or("")
    }

    /// The pre-parsed expression from the cache (keyed on the expression node itself).
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        cache.get(self.node.start_byte())
    }
}

impl<'src> HtmlTag<'src> {
    /// The expression node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("expression")
            .or_else(|| {
                let mut cursor = self.node.walk();
                self.node.children(&mut cursor)
                    .find(|c| c.kind() == "expression_value")
            })
    }

    /// The pre-parsed expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }
}

impl<'src> ConstTag<'src> {
    /// The expression/declaration node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        let mut cursor = self.node.walk();
        self.node.children(&mut cursor)
            .find(|c| c.kind() == "expression_value")
    }

    /// The pre-parsed expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }
}

impl<'src> DebugTag<'src> {
    /// The expression value node containing debug identifiers.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        let mut cursor = self.node.walk();
        self.node.children(&mut cursor)
            .find(|c| c.kind() == "expression_value")
    }

    /// The pre-parsed expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }
}

impl<'src> RenderTag<'src> {
    /// The expression node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("expression")
            .or_else(|| {
                let mut cursor = self.node.walk();
                self.node.children(&mut cursor)
                    .find(|c| c.kind() == "expression_value")
            })
    }

    /// The pre-parsed expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }
}

impl<'src> AttachTag<'src> {
    /// The expression node.
    pub fn expression_node(&self) -> Option<Node<'src>> {
        let mut cursor = self.node.walk();
        self.node.children(&mut cursor)
            .find(|c| c.kind() == "expression_value" || c.kind() == "expression")
    }

    /// The pre-parsed expression from the cache.
    pub fn expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        self.expression_node().and_then(|n| cache.get(n.start_byte()))
    }
}

// --- AttributeNode accessors ---

impl<'src> AttributeNode<'src> {
    /// The attribute name.
    pub fn name(&self) -> &'src str {
        if let Some(name_node) = self.node.child_by_field_name("name") {
            node_text(self.source, name_node)
        } else {
            // Shorthand attribute — content is the name
            node_text(self.source, self.node)
        }
    }

    /// The attribute value node, if any.
    pub fn value_node(&self) -> Option<Node<'src>> {
        self.node.child_by_field_name("value")
    }

    /// Whether this is a shorthand attribute (`{identifier}`).
    pub fn is_shorthand(&self) -> bool {
        self.node.kind() == "shorthand_attribute"
    }

    /// Whether this is a spread attribute (`{...expr}`).
    pub fn is_spread(&self) -> bool {
        self.is_shorthand() && self.text().starts_with("{...")
    }

    /// Whether this is a directive (e.g., `bind:value`, `on:click`).
    pub fn is_directive(&self) -> bool {
        self.directive_prefix().is_some()
    }

    /// The directive prefix (e.g., "class" for `class:active`, "bind" for `bind:value`).
    pub fn directive_prefix(&self) -> Option<&'src str> {
        let name_node = self.node.child_by_field_name("name")?;
        let mut cursor = name_node.walk();
        for child in name_node.children(&mut cursor) {
            if child.kind() == "attribute_directive" {
                return Some(node_text(self.source, child));
            }
        }
        None
    }

    /// For directives, the identifier after the colon (e.g., "active" in `class:active`).
    pub fn directive_name(&self) -> Option<&'src str> {
        let name_node = self.node.child_by_field_name("name")?;
        let mut cursor = name_node.walk();
        for child in name_node.children(&mut cursor) {
            if child.kind() == "attribute_identifier" {
                return Some(node_text(self.source, child));
            }
        }
        None
    }

    /// Whether this is a `class:name` directive.
    pub fn is_class_directive(&self) -> bool {
        self.directive_prefix() == Some("class")
    }

    /// Whether this is a `bind:name` directive.
    pub fn is_bind_directive(&self) -> bool {
        self.directive_prefix() == Some("bind")
    }

    /// Whether this is a `style:name` directive.
    pub fn is_style_directive(&self) -> bool {
        self.directive_prefix() == Some("style")
    }

    /// Whether this wraps a shorthand_attribute child node (both `{name}` and `{...spread}`).
    pub fn has_shorthand_child(&self) -> bool {
        let mut cursor = self.node.walk();
        self.node.children(&mut cursor).any(|c| c.kind() == "shorthand_attribute")
    }

    /// The static text value of this attribute, if it's a simple quoted or bare value.
    pub fn static_value(&self) -> Option<&'src str> {
        let value = self.value_node()?;
        match value.kind() {
            "quoted_attribute_value" => {
                let mut cursor = value.walk();
                for child in value.children(&mut cursor) {
                    if child.kind() == "attribute_value" {
                        return Some(node_text(self.source, child));
                    }
                }
                None
            }
            "attribute_value" => Some(node_text(self.source, value)),
            _ => None,
        }
    }

    /// The value expression from the cache, if the value is an expression tag.
    pub fn value_expression<'c>(&self, cache: &'c ExpressionCache) -> Option<&'c Expression> {
        let value = self.value_node()?;
        match value.kind() {
            "expression" => cache.get(value.start_byte()),
            "quoted_attribute_value" => {
                let mut cursor = value.walk();
                for child in value.children(&mut cursor) {
                    if child.kind() == "expression" {
                        return cache.get(child.start_byte());
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Whether the value contains any expression (dynamic value).
    pub fn has_expression_value(&self) -> bool {
        let Some(value) = self.value_node() else { return false };
        match value.kind() {
            "expression" => true,
            "quoted_attribute_value" => {
                let mut cursor = value.walk();
                value.children(&mut cursor).any(|c| c.kind() == "expression")
            }
            _ => false,
        }
    }

    /// Whether the value contains mixed text and expressions.
    pub fn has_mixed_value(&self) -> bool {
        let Some(value) = self.value_node() else { return false };
        if value.kind() != "quoted_attribute_value" { return false; }
        let mut has_text = false;
        let mut has_expr = false;
        let mut cursor = value.walk();
        for child in value.children(&mut cursor) {
            match child.kind() {
                "attribute_value" => has_text = true,
                "expression" => has_expr = true,
                _ => {}
            }
        }
        has_text && has_expr
    }

    /// The source text that this attribute references.
    pub fn source_text(&self) -> &'src str {
        self.source
    }

    /// Iterate over value parts as `(is_expression: bool, text: &str, start_byte: usize)`.
    /// For quoted values like `"foo {bar} baz"`, yields text and expression parts.
    /// For expression values like `{expr}`, yields one expression part.
    /// For boolean (no value), yields nothing.
    pub fn value_parts(&self) -> Vec<AttributeValuePart<'src>> {
        let Some(value) = self.value_node() else { return vec![] };
        match value.kind() {
            "expression" => {
                vec![AttributeValuePart::Expression(
                    value.start_byte(),
                    node_text(self.source, value),
                )]
            }
            "quoted_attribute_value" => {
                let mut parts = Vec::new();
                let mut cursor = value.walk();
                for child in value.children(&mut cursor) {
                    match child.kind() {
                        "attribute_value" => {
                            parts.push(AttributeValuePart::Text(
                                node_text(self.source, child),
                            ));
                        }
                        "expression" => {
                            parts.push(AttributeValuePart::Expression(
                                child.start_byte(),
                                node_text(self.source, child),
                            ));
                        }
                        _ => {}
                    }
                }
                parts
            }
            _ => vec![],
        }
    }
}

/// A part of an attribute value (text or expression).
#[derive(Debug, Clone, Copy)]
pub enum AttributeValuePart<'src> {
    /// Static text content.
    Text(&'src str),
    /// An expression `{...}`. The usize is the start byte offset for cache lookup.
    Expression(usize, &'src str),
}

// --- ElseClause (not in define_wrapper since it's structural) ---

define_wrapper!(
    /// An else clause in a block.
    ElseClause,
);

impl<'src> ElseClause<'src> {
    /// Iterate over child nodes of this else clause.
    pub fn children(&self) -> ChildIter<'src> {
        ChildIter::new(self.source, self.node)
    }
}

// --- Alternate enum ---

/// The alternate branch of an if block.
#[derive(Debug, Clone, Copy)]
pub enum Alternate<'src> {
    Else(ElseClause<'src>),
    ElseIf(IfBlock<'src>),
}

// --- TemplateNode enum ---

/// A template node — discriminated by tree-sitter node kind.
#[derive(Debug, Clone, Copy)]
pub enum TemplateNode<'src> {
    Text(TextNode<'src>),
    Comment(CommentNode<'src>),
    ExpressionTag(ExpressionTag<'src>),
    HtmlTag(HtmlTag<'src>),
    ConstTag(ConstTag<'src>),
    DebugTag(DebugTag<'src>),
    RenderTag(RenderTag<'src>),
    AttachTag(AttachTag<'src>),
    IfBlock(IfBlock<'src>),
    EachBlock(EachBlock<'src>),
    AwaitBlock(AwaitBlock<'src>),
    KeyBlock(KeyBlock<'src>),
    SnippetBlock(SnippetBlock<'src>),
    // Element variants — all wrap Element but carry classification
    RegularElement(Element<'src>),
    Component(Element<'src>),
    SlotElement(Element<'src>),
    SvelteHead(Element<'src>),
    SvelteBody(Element<'src>),
    SvelteWindow(Element<'src>),
    SvelteDocument(Element<'src>),
    SvelteComponent(Element<'src>),
    SvelteElement(Element<'src>),
    SvelteSelf(Element<'src>),
    SvelteFragment(Element<'src>),
    SvelteBoundary(Element<'src>),
    TitleElement(Element<'src>),
}

impl<'src> TemplateNode<'src> {
    /// Byte offset of the start of this node.
    pub fn start(&self) -> usize {
        self.ts_node().start_byte()
    }

    /// Byte offset of the end of this node.
    pub fn end(&self) -> usize {
        self.ts_node().end_byte()
    }

    /// The underlying tree-sitter node.
    pub fn ts_node(&self) -> Node<'src> {
        match self {
            Self::Text(n) => n.ts_node(),
            Self::Comment(n) => n.ts_node(),
            Self::ExpressionTag(n) => n.ts_node(),
            Self::HtmlTag(n) => n.ts_node(),
            Self::ConstTag(n) => n.ts_node(),
            Self::DebugTag(n) => n.ts_node(),
            Self::RenderTag(n) => n.ts_node(),
            Self::AttachTag(n) => n.ts_node(),
            Self::IfBlock(n) => n.ts_node(),
            Self::EachBlock(n) => n.ts_node(),
            Self::AwaitBlock(n) => n.ts_node(),
            Self::KeyBlock(n) => n.ts_node(),
            Self::SnippetBlock(n) => n.ts_node(),
            Self::RegularElement(n)
            | Self::Component(n)
            | Self::SlotElement(n)
            | Self::SvelteHead(n)
            | Self::SvelteBody(n)
            | Self::SvelteWindow(n)
            | Self::SvelteDocument(n)
            | Self::SvelteComponent(n)
            | Self::SvelteElement(n)
            | Self::SvelteSelf(n)
            | Self::SvelteFragment(n)
            | Self::SvelteBoundary(n)
            | Self::TitleElement(n) => n.ts_node(),
        }
    }

    /// If this is any element variant, return the inner `Element`.
    pub fn as_element(&self) -> Option<&Element<'src>> {
        match self {
            Self::RegularElement(e)
            | Self::Component(e)
            | Self::SlotElement(e)
            | Self::SvelteHead(e)
            | Self::SvelteBody(e)
            | Self::SvelteWindow(e)
            | Self::SvelteDocument(e)
            | Self::SvelteComponent(e)
            | Self::SvelteElement(e)
            | Self::SvelteSelf(e)
            | Self::SvelteFragment(e)
            | Self::SvelteBoundary(e)
            | Self::TitleElement(e) => Some(e),
            _ => None,
        }
    }

    /// Whether this node is a component-like element (Component, SvelteComponent, etc.).
    pub fn is_component_like(&self) -> bool {
        matches!(
            self,
            Self::Component(_)
                | Self::SvelteComponent(_)
                | Self::SvelteSelf(_)
                | Self::SvelteFragment(_)
                | Self::SvelteBoundary(_)
                | Self::SvelteHead(_)
                | Self::SvelteBody(_)
                | Self::SvelteWindow(_)
                | Self::SvelteDocument(_)
                | Self::TitleElement(_)
        )
    }

    /// Whether this is a SvelteElement (`<svelte:element>`).
    pub fn is_svelte_element(&self) -> bool {
        matches!(self, Self::SvelteElement(_))
    }

    /// Visit all direct child fragments of this node.
    /// Calls `f` with a `ChildIter` for each child fragment (body, fallback, branches, etc.).
    pub fn for_each_child_iter<F>(&self, mut f: F)
    where
        F: FnMut(ChildIter<'src>),
    {
        match self {
            Self::RegularElement(el)
            | Self::Component(el)
            | Self::SlotElement(el)
            | Self::SvelteHead(el)
            | Self::SvelteBody(el)
            | Self::SvelteWindow(el)
            | Self::SvelteDocument(el)
            | Self::SvelteComponent(el)
            | Self::SvelteElement(el)
            | Self::SvelteSelf(el)
            | Self::SvelteFragment(el)
            | Self::SvelteBoundary(el)
            | Self::TitleElement(el) => {
                f(el.children());
            }
            Self::IfBlock(block) => {
                f(block.consequent());
                match block.alternate() {
                    Some(Alternate::Else(clause)) => f(clause.children()),
                    Some(Alternate::ElseIf(nested)) => {
                        TemplateNode::IfBlock(nested).for_each_child_iter(f);
                    }
                    None => {}
                }
            }
            Self::EachBlock(block) => {
                f(block.body());
                if let Some(clause) = block.fallback() {
                    f(clause.children());
                }
            }
            Self::AwaitBlock(block) => {
                if let Some(iter) = block.pending() {
                    f(iter);
                }
                if let Some(iter) = block.then_children() {
                    f(iter);
                }
                if let Some(iter) = block.catch_children() {
                    f(iter);
                }
            }
            Self::KeyBlock(block) => {
                f(block.body());
            }
            Self::SnippetBlock(block) => {
                f(block.body());
            }
            _ => {}
        }
    }

    /// Recursively walk all descendant template nodes depth-first.
    pub fn walk<F>(&self, f: &mut F)
    where
        F: FnMut(TemplateNode<'src>),
    {
        self.for_each_child_iter(|iter| {
            for child in iter {
                f(child);
                child.walk(f);
            }
        });
    }
}

impl<'src> Root<'src> {
    /// Recursively walk all descendant template nodes depth-first.
    pub fn walk<F>(&self, f: &mut F)
    where
        F: FnMut(TemplateNode<'src>),
    {
        for child in self.children() {
            f(child);
            child.walk(f);
        }
    }

    /// Check if any descendant matches a predicate.
    pub fn any<F>(&self, mut f: F) -> bool
    where
        F: FnMut(TemplateNode<'src>) -> bool,
    {
        let mut found = false;
        self.walk(&mut |node| {
            if !found && f(node) {
                found = true;
            }
        });
        found
    }
}

// --- Classify functions ---

/// Classify an `element` tree-sitter node into the appropriate `TemplateNode` variant.
fn classify_element<'src>(source: &'src str, node: Node<'src>) -> TemplateNode<'src> {
    let el = Element::new(source, node);
    let name = el.name();

    match name {
        "slot" => TemplateNode::SlotElement(el),
        "title" => TemplateNode::TitleElement(el),
        _ if name.starts_with("svelte:") => {
            match &name[7..] {
                "head" => TemplateNode::SvelteHead(el),
                "body" => TemplateNode::SvelteBody(el),
                "window" => TemplateNode::SvelteWindow(el),
                "document" => TemplateNode::SvelteDocument(el),
                "component" => TemplateNode::SvelteComponent(el),
                "element" => TemplateNode::SvelteElement(el),
                "self" => TemplateNode::SvelteSelf(el),
                "fragment" => TemplateNode::SvelteFragment(el),
                "boundary" => TemplateNode::SvelteBoundary(el),
                _ => TemplateNode::RegularElement(el),
            }
        }
        _ if is_component_name(name) => TemplateNode::Component(el),
        _ => TemplateNode::RegularElement(el),
    }
}

/// Classify any named tree-sitter node into a `TemplateNode`.
pub fn classify_node<'src>(source: &'src str, node: Node<'src>) -> Option<TemplateNode<'src>> {
    match node.kind() {
        "text" => Some(TemplateNode::Text(TextNode::new(source, node))),
        "comment" => Some(TemplateNode::Comment(CommentNode::new(source, node))),
        "expression" => Some(TemplateNode::ExpressionTag(ExpressionTag::new(source, node))),
        "html_tag" => Some(TemplateNode::HtmlTag(HtmlTag::new(source, node))),
        "const_tag" => Some(TemplateNode::ConstTag(ConstTag::new(source, node))),
        "debug_tag" => Some(TemplateNode::DebugTag(DebugTag::new(source, node))),
        "render_tag" => Some(TemplateNode::RenderTag(RenderTag::new(source, node))),
        "attach_tag" => Some(TemplateNode::AttachTag(AttachTag::new(source, node))),
        "if_block" => Some(TemplateNode::IfBlock(IfBlock::new(source, node))),
        "each_block" => Some(TemplateNode::EachBlock(EachBlock::new(source, node))),
        "await_block" => Some(TemplateNode::AwaitBlock(AwaitBlock::new(source, node))),
        "key_block" => Some(TemplateNode::KeyBlock(KeyBlock::new(source, node))),
        "snippet_block" => Some(TemplateNode::SnippetBlock(SnippetBlock::new(source, node))),
        "element" => Some(classify_element(source, node)),
        _ => None,
    }
}

// --- ChildIter ---

/// Iterator over child nodes that are template content (skipping structural nodes).
pub struct ChildIter<'src> {
    source: &'src str,
    cursor: TreeCursor<'src>,
    started: bool,
}

impl<'src> ChildIter<'src> {
    fn new(source: &'src str, parent: Node<'src>) -> Self {
        Self {
            source,
            cursor: parent.walk(),
            started: false,
        }
    }
}

/// Node kinds that are structural (not template content).
fn is_structural_kind(kind: &str) -> bool {
    matches!(
        kind,
        "block_open"
            | "block_close"
            | "block_end"
            | "block_keyword"
            | "block_sigil"
            | "branch_kind"
            | "start_tag"
            | "end_tag"
            | "self_closing_tag"
            | "else_clause"
            | "else_if_clause"
            | "await_branch"
            | "await_pending"
            | "await_branch_children"
            | "snippet_name"
            | "snippet_parameters"
            | "snippet_type_parameters"
            | "pattern"
            | "expression_value"
            | "shorthand_kind"
            | "raw_text"
    )
}

impl<'src> Iterator for ChildIter<'src> {
    type Item = TemplateNode<'src>;

    fn next(&mut self) -> Option<TemplateNode<'src>> {
        loop {
            let moved = if self.started {
                self.cursor.goto_next_sibling()
            } else {
                self.started = true;
                self.cursor.goto_first_child()
            };

            if !moved {
                return None;
            }

            let node = self.cursor.node();
            if !node.is_named() {
                continue;
            }

            // Skip children that are field-named (structural parts of parent:
            // expression, binding, key, etc.)
            if self.cursor.field_name().is_some() {
                continue;
            }

            let kind = node.kind();
            if is_structural_kind(kind) {
                continue;
            }

            if let Some(template_node) = classify_node(self.source, node) {
                return Some(template_node);
            }
        }
    }
}

// --- AttributeIter ---

/// Iterator over attribute nodes on a start tag.
pub struct AttributeIter<'src> {
    source: &'src str,
    cursor: TreeCursor<'src>,
    started: bool,
}

impl<'src> Iterator for AttributeIter<'src> {
    type Item = AttributeNode<'src>;

    fn next(&mut self) -> Option<AttributeNode<'src>> {
        loop {
            let moved = if self.started {
                self.cursor.goto_next_sibling()
            } else {
                self.started = true;
                self.cursor.goto_first_child()
            };

            if !moved {
                return None;
            }

            let node = self.cursor.node();
            match node.kind() {
                "attribute" | "shorthand_attribute" | "attach_tag" => {
                    return Some(AttributeNode::new(self.source, node));
                }
                _ => continue,
            }
        }
    }
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

    // --- Wrapper type tests ---

    fn parse_source(text: &str) -> Document<'_> {
        let source = SourceText::new(SourceId::new(100), text, None);
        parse_svelte(source).expect("parse")
    }

    #[test]
    fn wrapper_element_name_via_field() {
        let text = r#"<div class="foo">hello</div>"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        assert_eq!(children.len(), 1);
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected RegularElement");
        };
        assert_eq!(el.name(), "div");
        assert!(!el.is_self_closing());
        assert!(el.has_end_tag());
    }

    #[test]
    fn wrapper_self_closing_element() {
        let text = r#"<br />"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        assert_eq!(children.len(), 1);
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected RegularElement");
        };
        assert_eq!(el.name(), "br");
        assert!(el.is_self_closing());
    }

    #[test]
    fn wrapper_component_classification() {
        let text = r#"<Button>Click</Button>"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        assert!(matches!(&children[0], TemplateNode::Component(el) if el.name() == "Button"));
    }

    #[test]
    fn wrapper_svelte_element_classification() {
        let text = r#"<svelte:head><title>Hi</title></svelte:head>"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        assert!(matches!(&children[0], TemplateNode::SvelteHead(_)));
    }

    #[test]
    fn wrapper_if_block() {
        let text = r#"{#if visible}<p>Hello</p>{/if}"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        assert_eq!(children.len(), 1);
        let TemplateNode::IfBlock(block) = &children[0] else {
            panic!("expected IfBlock");
        };
        assert!(block.test_node().is_some());
        let consequent: Vec<_> = block.consequent().collect();
        assert!(!consequent.is_empty());
    }

    #[test]
    fn wrapper_each_block() {
        let text = r#"{#each items as item}<li>{item}</li>{/each}"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        assert_eq!(children.len(), 1);
        let TemplateNode::EachBlock(block) = &children[0] else {
            panic!("expected EachBlock");
        };
        assert!(block.expression_node().is_some());
        let body: Vec<_> = block.body().collect();
        assert!(!body.is_empty());
    }

    #[test]
    fn wrapper_text_node() {
        let text = r#"<div>hello world</div>"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let inner: Vec<_> = el.children().collect();
        assert_eq!(inner.len(), 1);
        let TemplateNode::Text(t) = &inner[0] else {
            panic!("expected text");
        };
        assert_eq!(t.raw(), "hello world");
    }

    #[test]
    fn wrapper_attributes() {
        let text = r#"<div class="foo" id="bar">x</div>"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let attrs: Vec<_> = el.attributes().collect();
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].name(), "class");
        assert_eq!(attrs[1].name(), "id");
    }

    #[test]
    fn wrapper_expression_tag() {
        let text = r#"<p>{count}</p>"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let inner: Vec<_> = el.children().collect();
        assert!(matches!(&inner[0], TemplateNode::ExpressionTag(_)));
    }

    #[test]
    fn wrapper_snippet_block() {
        let text = r#"{#snippet btn(text)}<button>{text}</button>{/snippet}"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::SnippetBlock(block) = &children[0] else {
            panic!("expected SnippetBlock");
        };
        assert_eq!(block.name(), "btn");
        assert!(block.parameters_node().is_some());
    }

    #[test]
    fn wrapper_child_iter_skips_structural_nodes() {
        // Ensure ChildIter doesn't yield start_tag, end_tag, block_open, etc.
        let text = r#"{#if x}<div>A</div>{:else}<span>B</span>{/if}"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::IfBlock(block) = &children[0] else {
            panic!("expected IfBlock");
        };
        // Consequent should have just the div
        let consequent: Vec<_> = block.consequent().collect();
        assert!(consequent.iter().all(|n| matches!(n, TemplateNode::RegularElement(_))));
    }

    // --- ParsedDocument tests ---

    #[test]
    fn attribute_directive_accessors() {
        let text = r#"<div class:active={isActive} bind:value={name} style:color="red" />"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let attrs: Vec<_> = el.attributes().collect();
        assert_eq!(attrs.len(), 3);

        // class:active
        assert!(attrs[0].is_class_directive());
        assert_eq!(attrs[0].directive_prefix(), Some("class"));
        assert_eq!(attrs[0].directive_name(), Some("active"));

        // bind:value
        assert!(attrs[1].is_bind_directive());
        assert_eq!(attrs[1].directive_prefix(), Some("bind"));
        assert_eq!(attrs[1].directive_name(), Some("value"));

        // style:color
        assert!(attrs[2].is_style_directive());
        assert_eq!(attrs[2].directive_prefix(), Some("style"));
        assert_eq!(attrs[2].directive_name(), Some("color"));
        assert_eq!(attrs[2].static_value(), Some("red"));
    }

    #[test]
    fn attribute_static_and_expression_values() {
        let text = r#"<div class="foo" id={myId} />"#;
        let doc = ParsedDocument::parse(text).unwrap();
        let children: Vec<_> = doc.root().children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let attrs: Vec<_> = el.attributes().collect();
        assert_eq!(attrs.len(), 2);

        // class="foo" — static value
        assert_eq!(attrs[0].static_value(), Some("foo"));
        assert!(!attrs[0].has_expression_value());

        // id={myId} — expression value
        assert!(attrs[1].has_expression_value());
        let expr = attrs[1].value_expression(doc.expressions());
        assert!(expr.is_some(), "id expression should be cached");
    }

    #[test]
    fn attribute_spread_detection() {
        let text = r#"<div {...props} {shorthand} />"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let children: Vec<_> = root.children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let attrs: Vec<_> = el.attributes().collect();
        assert_eq!(attrs.len(), 2);

        // {...props} is within an attribute wrapping a shorthand
        assert!(attrs[0].has_shorthand_child());
        // {shorthand} is also within an attribute wrapping a shorthand
        assert!(attrs[1].has_shorthand_child());
    }

    #[test]
    fn walk_visits_all_descendants() {
        let text = r#"<div><p>A</p><span>{x}</span></div>{#if y}<b>B</b>{/if}"#;
        let doc = parse_source(text);
        let root = Root::new(text, doc.root_node());
        let mut count = 0;
        root.walk(&mut |_| count += 1);
        // div, p, Text(A), span, ExpressionTag(x), if_block, b, Text(B)
        assert!(count >= 7, "expected at least 7 descendants, got {count}");
    }

    // --- ParsedDocument tests ---

    #[test]
    fn parsed_document_basic() {
        let doc = ParsedDocument::parse("<div>{count}</div>").unwrap();
        assert_eq!(doc.root().children().count(), 1);
        assert!(!doc.expressions().is_empty());
    }

    #[test]
    fn parsed_document_expression_cache() {
        let doc = ParsedDocument::parse("{#if visible}<p>Hello</p>{/if}").unwrap();
        let children: Vec<_> = doc.root().children().collect();
        let TemplateNode::IfBlock(block) = &children[0] else {
            panic!("expected IfBlock");
        };
        let expr = block.test_expression(doc.expressions());
        assert!(expr.is_some(), "test expression should be cached");
    }

    #[test]
    fn parsed_document_each_expression() {
        let doc = ParsedDocument::parse("{#each items as item}<li>{item}</li>{/each}").unwrap();
        let children: Vec<_> = doc.root().children().collect();
        let TemplateNode::EachBlock(block) = &children[0] else {
            panic!("expected EachBlock");
        };
        let expr = block.expression(doc.expressions());
        assert!(expr.is_some(), "each expression should be cached");
    }

    #[test]
    fn parsed_document_expression_tag() {
        let doc = ParsedDocument::parse("<p>{count + 1}</p>").unwrap();
        let children: Vec<_> = doc.root().children().collect();
        let TemplateNode::RegularElement(el) = &children[0] else {
            panic!("expected element");
        };
        let inner: Vec<_> = el.children().collect();
        let TemplateNode::ExpressionTag(tag) = &inner[0] else {
            panic!("expected ExpressionTag");
        };
        let expr = tag.expression(doc.expressions());
        assert!(expr.is_some(), "expression tag should be cached");
    }
}
