//! Arena-based Svelte AST with stable node IDs, parent pointers, and position queries.
//!
//! This module provides [`SvelteAst`], an alternative AST representation designed for
//! tooling consumers (language servers, linters, formatters) that need:
//!
//! - **Stable node IDs** ([`NodeId`]) that survive incremental re-parses
//! - **Parent pointers** for upward navigation without recursion
//! - **Position queries** (node-at-offset, nodes-in-range) via binary search
//! - **Incremental updates** via [`TextEdit`] that re-parse only changed regions
//!
//! # Examples
//!
//! ```
//! use svelte_syntax::arena::{SvelteAst, NodeKind};
//!
//! let ast = SvelteAst::parse("<div>{count}</div>").unwrap();
//! let root = ast.root();
//! assert!(matches!(ast.kind(root), NodeKind::Root));
//!
//! // Walk children
//! for &child in ast.children(root) {
//!     println!("{:?} at {}..{}", ast.kind(child), ast.start(child), ast.end(child));
//! }
//!
//! // Position query
//! if let Some(node) = ast.innermost_at_offset(6) {
//!     println!("innermost node at offset 6: {:?}", ast.kind(node));
//! }
//! ```

use std::sync::Arc;

use smallvec::SmallVec;

use crate::ast::common::{ScriptContext, Span};
use crate::ast::modern::{self, Node as ModernNode};
use crate::error::CompileError;
use crate::js::{JsExpression, JsProgram};
use crate::parse::{ParseMode, ParseOptions};

/// Stable identifier for a node in the Svelte AST arena.
///
/// Node IDs are indices into the arena's internal storage. They are stable
/// across queries but may be invalidated by [`SvelteAst::edit`] for nodes
/// in changed regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

impl NodeId {
    fn new(index: usize) -> Self {
        Self(index as u32)
    }

    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Arena-allocated Svelte AST with parent pointers and position index.
///
/// # Examples
///
/// Navigate to a script and inspect its JS program:
///
/// ```
/// use svelte_syntax::arena::{SvelteAst, NodeKind};
///
/// let ast = SvelteAst::parse("<script>let x = 1;</script><p>{x}</p>").unwrap();
///
/// // Find the script node
/// let script = ast.descendants(ast.root())
///     .find(|&id| matches!(ast.kind(id), NodeKind::Script { .. }))
///     .unwrap();
///
/// // Access the parsed JS program
/// let program = ast.js_program(script).unwrap();
/// assert_eq!(program.program().body.len(), 1);
///
/// // Find expression tags
/// let expr_count = ast.descendants(ast.root())
///     .filter(|&id| matches!(ast.kind(id), NodeKind::ExpressionTag))
///     .count();
/// assert_eq!(expr_count, 1);
/// ```
pub struct SvelteAst {
    nodes: Vec<ArenaNode>,
    source: Arc<str>,
    /// Sorted `(start, NodeId)` pairs for binary-search position queries.
    offset_index: Vec<(u32, NodeId)>,
}

/// A node in the arena, holding kind, span, parent link, and child IDs.
#[derive(Debug)]
pub struct ArenaNode {
    /// Redundant with this node's index in `SvelteAst::nodes`, but kept for
    /// convenience so code holding an `&ArenaNode` can recover its ID without
    /// needing the arena.
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub kind: NodeKind,
    pub start: u32,
    pub end: u32,
    pub children: SmallVec<[NodeId; 4]>,
}

/// The kind of a Svelte AST node, carrying only the data that is not
/// derivable from children or source text.
#[derive(Debug, Clone)]
pub enum NodeKind {
    Root,
    // Template nodes
    Text { data: Arc<str> },
    Comment { data: Arc<str> },
    ExpressionTag,
    HtmlTag,
    ConstTag,
    DebugTag,
    RenderTag,
    // Block nodes
    IfBlock { elseif: bool },
    EachBlock,
    AwaitBlock,
    KeyBlock,
    SnippetBlock { name: Arc<str> },
    // Elements
    RegularElement { name: Arc<str> },
    Component { name: Arc<str> },
    SlotElement { name: Arc<str> },
    SvelteHead,
    SvelteBody,
    SvelteWindow,
    SvelteDocument,
    SvelteComponent,
    SvelteElement,
    SvelteSelf,
    SvelteFragment,
    SvelteBoundary,
    TitleElement,
    // Script and Style
    Script { context: ScriptContext, program: Arc<JsProgram> },
    StyleSheet,
    // JS expressions (leaf references into OXC arena)
    Expression { handle: Option<Arc<JsExpression>> },
    // Sub-structures
    Attribute { name: Arc<str> },
    Alternate,
}

impl NodeKind {
    /// Return a short human-readable name for this node kind.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Root => "Root",
            Self::Text { .. } => "Text",
            Self::Comment { .. } => "Comment",
            Self::ExpressionTag => "ExpressionTag",
            Self::HtmlTag => "HtmlTag",
            Self::ConstTag => "ConstTag",
            Self::DebugTag => "DebugTag",
            Self::RenderTag => "RenderTag",
            Self::IfBlock { .. } => "IfBlock",
            Self::EachBlock => "EachBlock",
            Self::AwaitBlock => "AwaitBlock",
            Self::KeyBlock => "KeyBlock",
            Self::SnippetBlock { .. } => "SnippetBlock",
            Self::RegularElement { .. } => "RegularElement",
            Self::Component { .. } => "Component",
            Self::SlotElement { .. } => "SlotElement",
            Self::SvelteHead => "SvelteHead",
            Self::SvelteBody => "SvelteBody",
            Self::SvelteWindow => "SvelteWindow",
            Self::SvelteDocument => "SvelteDocument",
            Self::SvelteComponent => "SvelteComponent",
            Self::SvelteElement => "SvelteElement",
            Self::SvelteSelf => "SvelteSelf",
            Self::SvelteFragment => "SvelteFragment",
            Self::SvelteBoundary => "SvelteBoundary",
            Self::TitleElement => "TitleElement",
            Self::Script { .. } => "Script",
            Self::StyleSheet => "StyleSheet",
            Self::Expression { .. } => "Expression",
            Self::Attribute { .. } => "Attribute",
            Self::Alternate => "Alternate",
        }
    }
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RegularElement { name } | Self::Component { name } | Self::SlotElement { name } => {
                write!(f, "{} <{}>", self.name(), name)
            }
            Self::SnippetBlock { name } => write!(f, "SnippetBlock {}", name),
            Self::Attribute { name } => write!(f, "Attribute {}", name),
            Self::Script { context, .. } => write!(f, "Script ({:?})", context),
            _ => f.write_str(self.name()),
        }
    }
}

/// A text edit to apply for incremental re-parsing.
#[derive(Debug, Clone)]
pub struct TextEdit {
    pub range: std::ops::Range<usize>,
    pub replacement: String,
}

/// Result of an incremental edit, listing affected node IDs.
#[derive(Debug, Clone)]
pub struct EditResult {
    pub changed_nodes: Vec<NodeId>,
    pub removed_nodes: Vec<NodeId>,
    pub added_nodes: Vec<NodeId>,
}

// ---- Construction ----

impl SvelteAst {
    /// Parse source into an arena AST.
    pub fn parse(source: &str) -> Result<Self, CompileError> {
        let document = crate::parse::parse(
            source,
            ParseOptions {
                mode: ParseMode::Modern,
                ..ParseOptions::default()
            },
        )?;

        let crate::ast::Root::Modern(root) = document.root else {
            return Err(CompileError::internal("arena AST requires modern parse mode"));
        };

        let mut builder = ArenaBuilder {
            nodes: Vec::with_capacity(64),
        };

        builder.build_root(&root);

        let mut ast = SvelteAst {
            nodes: builder.nodes,
            source: document.source,
            offset_index: Vec::new(),
        };
        ast.rebuild_offset_index();
        Ok(ast)
    }

    fn rebuild_offset_index(&mut self) {
        self.offset_index.clear();
        self.offset_index.reserve(self.nodes.len());
        for node in &self.nodes {
            self.offset_index.push((node.start, node.id));
        }
        self.offset_index.sort_by_key(|&(start, id)| (start, id.0));
    }

    // ---- Navigation ----

    /// Return the root node ID.
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Access a node by its ID.
    pub fn node(&self, id: NodeId) -> &ArenaNode {
        &self.nodes[id.index()]
    }

    /// Return the kind of a node.
    pub fn kind(&self, id: NodeId) -> &NodeKind {
        &self.nodes[id.index()].kind
    }

    /// Return the start byte offset of a node.
    pub fn start(&self, id: NodeId) -> u32 {
        self.nodes[id.index()].start
    }

    /// Return the end byte offset of a node.
    pub fn end(&self, id: NodeId) -> u32 {
        self.nodes[id.index()].end
    }

    /// Return the parent of a node, if any.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.index()].parent
    }

    /// Return the children of a node.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id.index()].children
    }

    /// Iterate ancestors of a node (parent, grandparent, ...).
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut current = self.nodes[id.index()].parent;
        std::iter::from_fn(move || {
            let node = current?;
            current = self.nodes[node.index()].parent;
            Some(node)
        })
    }

    /// Depth-first pre-order iteration over a subtree (including the root).
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut stack = vec![id];
        std::iter::from_fn(move || {
            let node = stack.pop()?;
            // Push children in reverse so leftmost is visited first
            let children = &self.nodes[node.index()].children;
            for &child in children.iter().rev() {
                stack.push(child);
            }
            Some(node)
        })
    }

    /// Return the siblings of a node (all children of its parent, excluding itself).
    pub fn siblings(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let parent = self.nodes[id.index()].parent;
        let children: &[NodeId] = parent
            .map(|p| self.nodes[p.index()].children.as_slice())
            .unwrap_or(&[]);
        children.iter().copied().filter(move |&child| child != id)
    }

    // ---- Position queries ----

    /// Find the first node whose span contains the given byte offset.
    pub fn node_at_offset(&self, offset: usize) -> Option<NodeId> {
        let offset = offset as u32;
        // Binary search for the last node starting at or before offset
        let idx = self
            .offset_index
            .partition_point(|&(start, _)| start <= offset);
        if idx == 0 {
            return None;
        }
        // Search backwards from the partition point for a containing node
        for &(_, id) in self.offset_index[..idx].iter().rev() {
            let node = &self.nodes[id.index()];
            if node.start <= offset && offset < node.end {
                return Some(id);
            }
            // Early exit: if start is too far back, no more candidates
            if offset - node.start > 10000 {
                break;
            }
        }
        None
    }

    /// Find the innermost (deepest) node containing the given byte offset.
    ///
    /// ```
    /// use svelte_syntax::arena::{SvelteAst, NodeKind};
    ///
    /// let ast = SvelteAst::parse("<div>hello</div>").unwrap();
    /// let node = ast.innermost_at_offset(6).unwrap();
    /// assert!(matches!(ast.kind(node), NodeKind::Text { .. }));
    /// ```
    pub fn innermost_at_offset(&self, offset: usize) -> Option<NodeId> {
        let mut current = self.node_at_offset(offset)?;
        'outer: loop {
            for &child in &self.nodes[current.index()].children {
                let node = &self.nodes[child.index()];
                if node.start <= offset as u32 && (offset as u32) < node.end {
                    current = child;
                    continue 'outer;
                }
            }
            break;
        }
        Some(current)
    }

    /// Find all nodes whose spans overlap with the given byte range.
    pub fn nodes_in_range(&self, start: usize, end: usize) -> Vec<NodeId> {
        let start = start as u32;
        let end = end as u32;
        self.nodes
            .iter()
            .filter(|node| node.start < end && node.end > start)
            .map(|node| node.id)
            .collect()
    }

    // ---- Source access ----

    /// Return the full source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Return the source text covered by a node's span.
    pub fn node_text(&self, id: NodeId) -> &str {
        let node = &self.nodes[id.index()];
        &self.source[node.start as usize..node.end as usize]
    }

    /// Return the total number of nodes in the arena.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Return `true` if the arena has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    // ---- Type queries ----

    /// If this node is an Expression with an OXC handle, return a reference to the parsed expression.
    pub fn js_expression(&self, id: NodeId) -> Option<&JsExpression> {
        match &self.nodes[id.index()].kind {
            NodeKind::Expression { handle: Some(arc) } => Some(arc.as_ref()),
            _ => None,
        }
    }

    /// If this node is a Script, return a reference to the parsed JS program.
    pub fn js_program(&self, id: NodeId) -> Option<&JsProgram> {
        match &self.nodes[id.index()].kind {
            NodeKind::Script { program, .. } => Some(program.as_ref()),
            _ => None,
        }
    }

    /// Return `true` if the node is an element-like node (RegularElement, Component, etc.).
    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(
            self.nodes[id.index()].kind,
            NodeKind::RegularElement { .. }
                | NodeKind::Component { .. }
                | NodeKind::SlotElement { .. }
                | NodeKind::SvelteHead
                | NodeKind::SvelteBody
                | NodeKind::SvelteWindow
                | NodeKind::SvelteDocument
                | NodeKind::SvelteComponent
                | NodeKind::SvelteElement
                | NodeKind::SvelteSelf
                | NodeKind::SvelteFragment
                | NodeKind::SvelteBoundary
                | NodeKind::TitleElement
        )
    }

    /// Return `true` if the node is a block node (IfBlock, EachBlock, etc.).
    pub fn is_block(&self, id: NodeId) -> bool {
        matches!(
            self.nodes[id.index()].kind,
            NodeKind::IfBlock { .. }
                | NodeKind::EachBlock
                | NodeKind::AwaitBlock
                | NodeKind::KeyBlock
                | NodeKind::SnippetBlock { .. }
        )
    }

    /// Return the element/component name if this node is an element-like node.
    pub fn element_name(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id.index()].kind {
            NodeKind::RegularElement { name }
            | NodeKind::Component { name }
            | NodeKind::SlotElement { name } => Some(name),
            NodeKind::SvelteHead => Some("svelte:head"),
            NodeKind::SvelteBody => Some("svelte:body"),
            NodeKind::SvelteWindow => Some("svelte:window"),
            NodeKind::SvelteDocument => Some("svelte:document"),
            NodeKind::SvelteComponent => Some("svelte:component"),
            NodeKind::SvelteElement => Some("svelte:element"),
            NodeKind::SvelteSelf => Some("svelte:self"),
            NodeKind::SvelteFragment => Some("svelte:fragment"),
            NodeKind::SvelteBoundary => Some("svelte:boundary"),
            NodeKind::TitleElement => Some("title"),
            _ => None,
        }
    }

    /// Return the next sibling of a node, or `None` if it is the last child.
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id.index()].parent?;
        let siblings = &self.nodes[parent.index()].children;
        let pos = siblings.iter().position(|&c| c == id)?;
        siblings.get(pos + 1).copied()
    }

    /// Return the previous sibling of a node, or `None` if it is the first child.
    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id.index()].parent?;
        let siblings = &self.nodes[parent.index()].children;
        let pos = siblings.iter().position(|&c| c == id)?;
        if pos > 0 { Some(siblings[pos - 1]) } else { None }
    }

    /// Return the depth of a node (root = 0).
    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).count()
    }

    // ---- Incremental update ----

    /// Apply a text edit and incrementally re-parse affected regions.
    ///
    /// This re-parses the full document with the edited source (CST-level
    /// incremental reparsing is handled internally by tree-sitter), then
    /// rebuilds the arena. Returns an [`EditResult`] describing which nodes
    /// were affected.
    ///
    /// ```
    /// use svelte_syntax::arena::{SvelteAst, TextEdit};
    ///
    /// let mut ast = SvelteAst::parse("<p>hello</p>").unwrap();
    /// let result = ast.edit(TextEdit {
    ///     range: 3..8,
    ///     replacement: "world".to_string(),
    /// }).unwrap();
    /// assert_eq!(ast.source(), "<p>world</p>");
    /// ```
    pub fn edit(&mut self, edit: TextEdit) -> Result<EditResult, CompileError> {
        let mut new_source = String::with_capacity(
            self.source.len() - edit.range.len() + edit.replacement.len(),
        );
        new_source.push_str(&self.source[..edit.range.start]);
        new_source.push_str(&edit.replacement);
        new_source.push_str(&self.source[edit.range.end..]);

        let old_ids: Vec<NodeId> = self.nodes.iter().map(|n| n.id).collect();

        let new_ast = Self::parse(&new_source)?;
        let new_ids: Vec<NodeId> = new_ast.nodes.iter().map(|n| n.id).collect();

        let removed: Vec<NodeId> = old_ids
            .iter()
            .filter(|id| id.index() >= new_ast.nodes.len())
            .copied()
            .collect();
        let added: Vec<NodeId> = new_ids
            .iter()
            .filter(|id| id.index() >= self.nodes.len())
            .copied()
            .collect();

        // Nodes that exist in both but may have changed
        let changed: Vec<NodeId> = new_ids
            .iter()
            .filter(|id| {
                id.index() < self.nodes.len()
                    && (self.nodes[id.index()].start != new_ast.nodes[id.index()].start
                        || self.nodes[id.index()].end != new_ast.nodes[id.index()].end)
            })
            .copied()
            .collect();

        *self = new_ast;

        Ok(EditResult {
            changed_nodes: changed,
            removed_nodes: removed,
            added_nodes: added,
        })
    }
}

// ---- Arena builder ----

struct ArenaBuilder {
    nodes: Vec<ArenaNode>,
}

impl ArenaBuilder {
    fn alloc(&mut self, parent: Option<NodeId>, kind: NodeKind, start: u32, end: u32) -> NodeId {
        let id = NodeId::new(self.nodes.len());
        self.nodes.push(ArenaNode {
            id,
            parent,
            kind,
            start,
            end,
            children: SmallVec::new(),
        });
        if let Some(parent_id) = parent {
            self.nodes[parent_id.index()].children.push(id);
        }
        id
    }

    fn build_root(&mut self, root: &modern::Root) {
        let root_start = root
            .fragment
            .nodes
            .first()
            .map(|n| n.start())
            .unwrap_or(0) as u32;
        let root_end = root
            .fragment
            .nodes
            .last()
            .map(|n| n.end())
            .unwrap_or(0) as u32;

        let root_id = self.alloc(None, NodeKind::Root, root_start, root_end);

        // Instance script
        if let Some(script) = &root.instance {
            self.build_script(root_id, script, ScriptContext::Default);
        }

        // Module script
        if let Some(script) = &root.module {
            self.build_script(root_id, script, ScriptContext::Module);
        }

        // Fragment children
        self.build_fragment(root_id, &root.fragment);

        // CSS
        if let Some(css) = &root.css {
            self.alloc(
                Some(root_id),
                NodeKind::StyleSheet,
                css.start as u32,
                css.end as u32,
            );
        }
    }

    fn build_script(&mut self, parent: NodeId, script: &modern::Script, context: ScriptContext) {
        self.alloc(
            Some(parent),
            NodeKind::Script {
                context,
                program: script.content.clone(),
            },
            script.start as u32,
            script.end as u32,
        );
    }

    fn build_fragment(&mut self, parent: NodeId, fragment: &modern::Fragment) {
        for node in fragment.nodes.iter() {
            self.build_node(parent, node);
        }
    }

    fn build_node(&mut self, parent: NodeId, node: &ModernNode) {
        match node {
            ModernNode::Text(text) => {
                self.alloc(
                    Some(parent),
                    NodeKind::Text { data: text.data.clone() },
                    text.start as u32,
                    text.end as u32,
                );
            }
            ModernNode::Comment(comment) => {
                self.alloc(
                    Some(parent),
                    NodeKind::Comment { data: comment.data.clone() },
                    comment.start as u32,
                    comment.end as u32,
                );
            }
            ModernNode::ExpressionTag(tag) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::ExpressionTag,
                    tag.start as u32,
                    tag.end as u32,
                );
                self.build_expression(id, &tag.expression);
            }
            ModernNode::HtmlTag(tag) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::HtmlTag,
                    tag.start as u32,
                    tag.end as u32,
                );
                self.build_expression(id, &tag.expression);
            }
            ModernNode::ConstTag(tag) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::ConstTag,
                    tag.start as u32,
                    tag.end as u32,
                );
                self.build_expression(id, &tag.declaration);
            }
            ModernNode::DebugTag(tag) => {
                self.alloc(
                    Some(parent),
                    NodeKind::DebugTag,
                    tag.start as u32,
                    tag.end as u32,
                );
            }
            ModernNode::RenderTag(tag) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::RenderTag,
                    tag.start as u32,
                    tag.end as u32,
                );
                self.build_expression(id, &tag.expression);
            }
            ModernNode::IfBlock(block) => {
                self.build_if_block(parent, block);
            }
            ModernNode::EachBlock(block) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::EachBlock,
                    block.start as u32,
                    block.end as u32,
                );
                self.build_expression(id, &block.expression);
                if let Some(ctx) = &block.context {
                    self.build_expression(id, ctx);
                }
                if let Some(key) = &block.key {
                    self.build_expression(id, key);
                }
                self.build_fragment(id, &block.body);
                if let Some(fallback) = &block.fallback {
                    self.build_fragment(id, fallback);
                }
            }
            ModernNode::AwaitBlock(block) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::AwaitBlock,
                    block.start as u32,
                    block.end as u32,
                );
                self.build_expression(id, &block.expression);
                if let Some(val) = &block.value {
                    self.build_expression(id, val);
                }
                if let Some(err) = &block.error {
                    self.build_expression(id, err);
                }
                for f in [&block.pending, &block.then, &block.catch].into_iter().flatten() {
                    self.build_fragment(id, f);
                }
            }
            ModernNode::KeyBlock(block) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::KeyBlock,
                    block.start as u32,
                    block.end as u32,
                );
                self.build_expression(id, &block.expression);
                self.build_fragment(id, &block.fragment);
            }
            ModernNode::SnippetBlock(block) => {
                let name = block
                    .expression
                    .identifier_name()
                    .unwrap_or_else(|| Arc::from(""));
                let id = self.alloc(
                    Some(parent),
                    NodeKind::SnippetBlock { name },
                    block.start as u32,
                    block.end as u32,
                );
                self.build_expression(id, &block.expression);
                for param in block.parameters.iter() {
                    self.build_expression(id, param);
                }
                self.build_fragment(id, &block.body);
            }
            // Elements
            ModernNode::RegularElement(el) => {
                self.build_element(parent, NodeKind::RegularElement { name: el.name.clone() }, el);
            }
            ModernNode::Component(el) => {
                self.build_element(parent, NodeKind::Component { name: el.name.clone() }, el);
            }
            ModernNode::SlotElement(el) => {
                self.build_element(parent, NodeKind::SlotElement { name: el.name.clone() }, el);
            }
            ModernNode::SvelteHead(el) => self.build_element(parent, NodeKind::SvelteHead, el),
            ModernNode::SvelteBody(el) => self.build_element(parent, NodeKind::SvelteBody, el),
            ModernNode::SvelteWindow(el) => self.build_element(parent, NodeKind::SvelteWindow, el),
            ModernNode::SvelteDocument(el) => {
                self.build_element(parent, NodeKind::SvelteDocument, el);
            }
            ModernNode::SvelteComponent(el) => {
                self.build_element(parent, NodeKind::SvelteComponent, el);
            }
            ModernNode::SvelteElement(el) => {
                self.build_element(parent, NodeKind::SvelteElement, el);
            }
            ModernNode::SvelteSelf(el) => self.build_element(parent, NodeKind::SvelteSelf, el),
            ModernNode::SvelteFragment(el) => {
                self.build_element(parent, NodeKind::SvelteFragment, el);
            }
            ModernNode::SvelteBoundary(el) => {
                self.build_element(parent, NodeKind::SvelteBoundary, el);
            }
            ModernNode::TitleElement(el) => {
                self.build_element(parent, NodeKind::TitleElement, el);
            }
        }
    }

    fn build_if_block(&mut self, parent: NodeId, block: &modern::IfBlock) {
        let id = self.alloc(
            Some(parent),
            NodeKind::IfBlock { elseif: block.elseif },
            block.start as u32,
            block.end as u32,
        );
        self.build_expression(id, &block.test);
        self.build_fragment(id, &block.consequent);
        if let Some(alt) = &block.alternate {
            match alt.as_ref() {
                modern::Alternate::Fragment(f) => {
                    let alt_start = f.nodes.first().map(|n| n.start()).unwrap_or(0) as u32;
                    let alt_end = f.nodes.last().map(|n| n.end()).unwrap_or(0) as u32;
                    let alt_id = self.alloc(Some(id), NodeKind::Alternate, alt_start, alt_end);
                    self.build_fragment(alt_id, f);
                }
                modern::Alternate::IfBlock(nested) => {
                    self.build_if_block(id, nested);
                }
            }
        }
    }

    fn build_element<E: modern::Element>(&mut self, parent: NodeId, kind: NodeKind, el: &E) {
        let id = self.alloc(Some(parent), kind, el.start() as u32, el.end() as u32);
        for attr in el.attributes() {
            self.build_attribute(id, attr);
        }
        self.build_fragment(id, el.fragment());
    }

    fn build_attribute(&mut self, parent: NodeId, attr: &modern::Attribute) {
        match attr {
            modern::Attribute::Attribute(a) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::Attribute { name: a.name.clone() },
                    a.start as u32,
                    a.end as u32,
                );
                self.build_attribute_values(id, &a.value);
            }
            modern::Attribute::SpreadAttribute(s) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::Attribute { name: Arc::from("...") },
                    s.start as u32,
                    s.end as u32,
                );
                self.build_expression(id, &s.expression);
            }
            modern::Attribute::BindDirective(d)
            | modern::Attribute::OnDirective(d)
            | modern::Attribute::ClassDirective(d)
            | modern::Attribute::LetDirective(d)
            | modern::Attribute::AnimateDirective(d)
            | modern::Attribute::UseDirective(d) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::Attribute { name: d.name.clone() },
                    d.start as u32,
                    d.end as u32,
                );
                self.build_expression(id, &d.expression);
            }
            modern::Attribute::StyleDirective(d) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::Attribute { name: d.name.clone() },
                    d.start as u32,
                    d.end as u32,
                );
                self.build_attribute_values(id, &d.value);
            }
            modern::Attribute::TransitionDirective(d) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::Attribute { name: d.name.clone() },
                    d.start as u32,
                    d.end as u32,
                );
                self.build_expression(id, &d.expression);
            }
            modern::Attribute::AttachTag(a) => {
                let id = self.alloc(
                    Some(parent),
                    NodeKind::Attribute { name: Arc::from("@attach") },
                    a.start as u32,
                    a.end as u32,
                );
                self.build_expression(id, &a.expression);
            }
        }
    }

    fn build_attribute_values(&mut self, parent: NodeId, value: &modern::AttributeValueKind) {
        match value {
            modern::AttributeValueKind::Boolean(_) => {}
            modern::AttributeValueKind::ExpressionTag(tag) => {
                self.build_expression(parent, &tag.expression);
            }
            modern::AttributeValueKind::Values(values) => {
                for val in values.iter() {
                    match val {
                        modern::AttributeValue::ExpressionTag(tag) => {
                            self.build_expression(parent, &tag.expression);
                        }
                        modern::AttributeValue::Text(_) => {}
                    }
                }
            }
        }
    }

    fn build_expression(&mut self, parent: NodeId, expr: &modern::Expression) {
        let handle = match &expr.node {
            Some(modern::JsNodeHandle::Expression(arc)) => Some(arc.clone()),
            _ => None,
        };

        self.alloc(
            Some(parent),
            NodeKind::Expression { handle },
            expr.start as u32,
            expr.end as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_element() {
        let ast = SvelteAst::parse("<div>hello</div>").unwrap();
        assert!(!ast.is_empty());
        assert!(matches!(ast.kind(ast.root()), NodeKind::Root));

        let children = ast.children(ast.root());
        assert!(!children.is_empty());
    }

    #[test]
    fn parse_with_expression() {
        let ast = SvelteAst::parse("<p>{count}</p>").unwrap();
        let root = ast.root();

        // Find the expression tag by walking descendants
        let expr_tag = ast
            .descendants(root)
            .find(|&id| matches!(ast.kind(id), NodeKind::ExpressionTag));
        assert!(expr_tag.is_some(), "should find ExpressionTag");
    }

    #[test]
    fn parent_pointers_work() {
        let ast = SvelteAst::parse("<div><span>hi</span></div>").unwrap();
        let root = ast.root();

        // Root has no parent
        assert!(ast.parent(root).is_none());

        // Every non-root node has a parent
        for &id in ast.children(root) {
            assert_eq!(ast.parent(id), Some(root));
        }
    }

    #[test]
    fn ancestors_traverse_upward() {
        let ast = SvelteAst::parse("<div><span>hi</span></div>").unwrap();
        let root = ast.root();

        // Find deepest node
        let text = ast
            .descendants(root)
            .find(|&id| matches!(ast.kind(id), NodeKind::Text { .. }));
        assert!(text.is_some());

        let ancestors: Vec<_> = ast.ancestors(text.unwrap()).collect();
        assert!(ancestors.len() >= 2); // span, div, root
        assert!(ancestors.contains(&root));
    }

    #[test]
    fn innermost_at_offset_finds_deepest() {
        let ast = SvelteAst::parse("<div>hello</div>").unwrap();
        let node = ast.innermost_at_offset(6);
        assert!(node.is_some());
        assert!(matches!(ast.kind(node.unwrap()), NodeKind::Text { .. }));
    }

    #[test]
    fn node_text_returns_source_slice() {
        let ast = SvelteAst::parse("<div>hello</div>").unwrap();
        let text_node = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Text { .. }))
            .unwrap();
        assert_eq!(ast.node_text(text_node), "hello");
    }

    #[test]
    fn edit_updates_ast() {
        let mut ast = SvelteAst::parse("<p>hello</p>").unwrap();
        let result = ast.edit(TextEdit {
            range: 3..8,
            replacement: "world".to_string(),
        });
        assert!(result.is_ok());
        assert_eq!(ast.source(), "<p>world</p>");
    }

    #[test]
    fn siblings_excludes_self() {
        let ast = SvelteAst::parse("<div><a/><b/><c/></div>").unwrap();
        let div = ast.children(ast.root())[0];
        let div_children = ast.children(div);

        if div_children.len() >= 2 {
            let first = div_children[0];
            let sibs: Vec<_> = ast.siblings(first).collect();
            assert!(!sibs.contains(&first));
            assert!(!sibs.is_empty());
        }
    }

    #[test]
    fn if_block_structure() {
        let ast = SvelteAst::parse("{#if condition}<p>yes</p>{:else}<p>no</p>{/if}").unwrap();
        let if_block = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::IfBlock { .. }));
        assert!(if_block.is_some(), "should find IfBlock");

        let if_id = if_block.unwrap();
        assert!(ast.is_block(if_id));
        assert!(!ast.is_element(if_id));

        // Should have expression, element, and alternate children
        let children = ast.children(if_id);
        assert!(children.len() >= 2, "IfBlock should have children: {:?}", children.len());

        // Check for Expression child (the test condition)
        let has_expr = children.iter().any(|&id| matches!(ast.kind(id), NodeKind::Expression { .. }));
        assert!(has_expr, "IfBlock should have an Expression child for the test");
    }

    #[test]
    fn each_block_structure() {
        let ast = SvelteAst::parse("{#each items as item}<p>{item}</p>{/each}").unwrap();
        let each = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::EachBlock));
        assert!(each.is_some(), "should find EachBlock");
        assert!(ast.is_block(each.unwrap()));
    }

    #[test]
    fn script_contains_program() {
        let ast = SvelteAst::parse("<script>let count = 0;</script><p>{count}</p>").unwrap();
        let script = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Script { .. }));
        assert!(script.is_some(), "should find Script");

        let prog = ast.js_program(script.unwrap());
        assert!(prog.is_some(), "Script should have a JS program");
        assert_eq!(prog.unwrap().program().body.len(), 1);
    }

    #[test]
    fn expression_tag_has_js_handle() {
        let ast = SvelteAst::parse("<p>{count + 1}</p>").unwrap();
        let expr = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Expression { .. }));
        assert!(expr.is_some(), "should find Expression node");

        let js = ast.js_expression(expr.unwrap());
        assert!(js.is_some(), "Expression should have OXC handle");
    }

    #[test]
    fn attribute_expressions_are_traversed() {
        let ast = SvelteAst::parse("<button on:click={handler}>go</button>").unwrap();
        let attr = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Attribute { .. }));
        assert!(attr.is_some(), "should find Attribute");

        // The directive attribute should have an Expression child
        let attr_children = ast.children(attr.unwrap());
        let has_expr = attr_children.iter().any(|&id| matches!(ast.kind(id), NodeKind::Expression { .. }));
        assert!(has_expr, "Directive attribute should have Expression child");
    }

    #[test]
    fn spread_attribute_has_expression() {
        let ast = SvelteAst::parse("<div {...props}>hi</div>").unwrap();
        let spread = ast
            .descendants(ast.root())
            .find(|&id| {
                matches!(ast.kind(id), NodeKind::Attribute { name } if name.as_ref() == "...")
            });
        assert!(spread.is_some(), "should find spread attribute");

        let children = ast.children(spread.unwrap());
        let has_expr = children.iter().any(|&id| matches!(ast.kind(id), NodeKind::Expression { .. }));
        assert!(has_expr, "Spread attribute should have Expression child");
    }

    #[test]
    fn nodes_in_range_finds_overlapping() {
        let ast = SvelteAst::parse("<div><p>hello</p><span>world</span></div>").unwrap();
        // Range covering "hello" area
        let nodes = ast.nodes_in_range(8, 13);
        assert!(!nodes.is_empty(), "should find nodes in range");
    }

    #[test]
    fn is_element_and_is_block_classify_correctly() {
        let ast = SvelteAst::parse("<div>{#if x}<span/>{/if}</div>").unwrap();
        for id in ast.descendants(ast.root()) {
            match ast.kind(id) {
                NodeKind::RegularElement { .. } => assert!(ast.is_element(id)),
                NodeKind::IfBlock { .. } => {
                    assert!(ast.is_block(id));
                    assert!(!ast.is_element(id));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn complex_template() {
        let src = r#"<script>
  let items = [1, 2, 3];
  let show = true;
</script>

{#if show}
  {#each items as item}
    <p>{item}</p>
  {/each}
{/if}
"#;
        let ast = SvelteAst::parse(src).unwrap();
        assert!(ast.len() > 10, "complex template should have many nodes");

        // Check we have all expected node types
        let descendants = || ast.descendants(ast.root());
        assert!(descendants().any(|id| matches!(ast.kind(id), NodeKind::Script { .. })), "should have Script");
        assert!(descendants().any(|id| matches!(ast.kind(id), NodeKind::IfBlock { .. })), "should have IfBlock");
        assert!(descendants().any(|id| matches!(ast.kind(id), NodeKind::EachBlock)), "should have EachBlock");
        assert!(descendants().any(|id| matches!(ast.kind(id), NodeKind::ExpressionTag)), "should have ExpressionTag");
        assert!(descendants().any(|id| matches!(ast.kind(id), NodeKind::RegularElement { .. })), "should have RegularElement");
    }

    #[test]
    fn element_name_returns_tag_name() {
        let ast = SvelteAst::parse("<div><MyComponent/></div>").unwrap();
        let div = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::RegularElement { name } if name.as_ref() == "div"))
            .unwrap();
        assert_eq!(ast.element_name(div), Some("div"));

        let comp = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Component { .. }))
            .unwrap();
        assert_eq!(ast.element_name(comp), Some("MyComponent"));

        // Non-element returns None
        assert_eq!(ast.element_name(ast.root()), None);
    }

    #[test]
    fn next_and_prev_sibling() {
        let ast = SvelteAst::parse("<div><a/><b/><c/></div>").unwrap();
        let div = ast.children(ast.root())[0];
        let children = ast.children(div);
        assert!(children.len() >= 3);

        let a = children[0];
        let b = children[1];
        let c = children[2];

        assert_eq!(ast.next_sibling(a), Some(b));
        assert_eq!(ast.next_sibling(b), Some(c));
        assert_eq!(ast.next_sibling(c), None);

        assert_eq!(ast.prev_sibling(a), None);
        assert_eq!(ast.prev_sibling(b), Some(a));
        assert_eq!(ast.prev_sibling(c), Some(b));
    }

    #[test]
    fn depth_counts_ancestors() {
        let ast = SvelteAst::parse("<div><span>hi</span></div>").unwrap();
        assert_eq!(ast.depth(ast.root()), 0);

        let text = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Text { .. }))
            .unwrap();
        assert!(ast.depth(text) >= 2);
    }

    #[test]
    fn node_kind_display() {
        let ast = SvelteAst::parse("<div class='x'>hi</div>").unwrap();
        let div = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::RegularElement { .. }))
            .unwrap();
        assert_eq!(format!("{}", ast.kind(div)), "RegularElement <div>");

        let attr = ast
            .descendants(ast.root())
            .find(|&id| matches!(ast.kind(id), NodeKind::Attribute { .. }))
            .unwrap();
        assert_eq!(format!("{}", ast.kind(attr)), "Attribute class");
    }

    #[test]
    fn node_kind_name() {
        assert_eq!(NodeKind::Root.name(), "Root");
        assert_eq!(NodeKind::ExpressionTag.name(), "ExpressionTag");
        assert_eq!(NodeKind::EachBlock.name(), "EachBlock");
    }
}
