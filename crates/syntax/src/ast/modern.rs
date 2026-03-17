use std::ops::ControlFlow;
use std::sync::Arc;

use oxc_ast::ast::{
    BindingIdentifier, BindingPattern, Expression as OxcExpression, FormalParameter,
    FormalParameterRest, Program as OxcProgram, Statement as OxcStatement,
    VariableDeclaration as OxcVariableDeclaration,
};
use oxc_span::GetSpan;
use serde::{Deserialize, Serialize, ser::SerializeMap};

use crate::ast::common::Span;
pub use crate::ast::common::{
    AttrError, AttrErrorKind, AttributeValueSyntax, DirectiveValueSyntax,
    FragmentType, LiteralValue, Loc, SourceRange, ParseError, Position, RootCommentType,
    ScriptContext, ScriptType, SnippetHeaderError, SnippetHeaderErrorKind,
};
use crate::js::{JsExpression, JsParameters, JsPattern, JsProgram};

fn empty_parsed_js_program() -> Arc<JsProgram> {
    Arc::new(JsProgram::parse("", oxc_span::SourceType::mjs()))
}

/// Serde discriminant for `"type": "Root"` in the JSON AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootType {
    Root,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    pub r#type: FragmentType,
    pub nodes: Box<[Node]>,
}

impl Fragment {
    pub fn empty() -> Self {
        Self {
            r#type: FragmentType::Fragment,
            nodes: Box::new([]),
        }
    }
}

impl Default for Fragment {
    fn default() -> Self {
        Self::empty()
    }
}

pub enum Search<T> {
    Continue,
    Skip,
    Found(T),
}

#[derive(Clone, Copy)]
pub enum Entry<'a> {
    Node(&'a Node),
    IfBlock(&'a IfBlock),
}

impl<'a> Entry<'a> {
    pub fn as_node(self) -> Option<&'a Node> {
        match self {
            Self::Node(node) => Some(node),
            Self::IfBlock(_) => None,
        }
    }

    pub fn as_if_block(self) -> Option<&'a IfBlock> {
        match self {
            Self::Node(Node::IfBlock(block)) | Self::IfBlock(block) => Some(block),
            Self::Node(_) => None,
        }
    }
}

impl Fragment {
    /// Depth-first walk over descendant template nodes and else-if branches.
    pub fn walk<'a, T, S, E, L>(&'a self, state: &mut S, enter: E, leave: L) -> Option<T>
    where
        E: FnMut(Entry<'a>, &mut S) -> Search<T>,
        L: FnMut(Entry<'a>, &mut S),
    {
        fn walk_fragment<'a, T, S, E, L>(
            fragment: &'a Fragment,
            state: &mut S,
            enter: &mut E,
            leave: &mut L,
        ) -> Option<T>
        where
            E: FnMut(Entry<'a>, &mut S) -> Search<T>,
            L: FnMut(Entry<'a>, &mut S),
        {
            for node in fragment.nodes.iter() {
                if let Some(found) = walk_node(node, state, enter, leave) {
                    return Some(found);
                }
            }
            None
        }

        fn walk_entry<'a, T, S, E, L>(
            entry: Entry<'a>,
            state: &mut S,
            enter: &mut E,
            leave: &mut L,
        ) -> Option<T>
        where
            E: FnMut(Entry<'a>, &mut S) -> Search<T>,
            L: FnMut(Entry<'a>, &mut S),
        {
            match enter(entry, state) {
                Search::Found(found) => return Some(found),
                Search::Skip => {
                    leave(entry, state);
                    return None;
                }
                Search::Continue => {}
            }

            let found = match entry {
                Entry::Node(node) => walk_node_children(node, state, enter, leave),
                Entry::IfBlock(block) => walk_if_block_children(block, state, enter, leave),
            };
            if found.is_none() {
                leave(entry, state);
            }
            found
        }

        fn walk_alternate<'a, T, S, E, L>(
            alternate: &'a Alternate,
            state: &mut S,
            enter: &mut E,
            leave: &mut L,
        ) -> Option<T>
        where
            E: FnMut(Entry<'a>, &mut S) -> Search<T>,
            L: FnMut(Entry<'a>, &mut S),
        {
            match alternate {
                Alternate::Fragment(fragment) => walk_fragment(fragment, state, enter, leave),
                Alternate::IfBlock(block) => walk_entry(Entry::IfBlock(block), state, enter, leave),
            }
        }

        fn walk_if_block_children<'a, T, S, E, L>(
            block: &'a IfBlock,
            state: &mut S,
            enter: &mut E,
            leave: &mut L,
        ) -> Option<T>
        where
            E: FnMut(Entry<'a>, &mut S) -> Search<T>,
            L: FnMut(Entry<'a>, &mut S),
        {
            walk_fragment(&block.consequent, state, enter, leave).or_else(|| {
                block
                    .alternate
                    .as_deref()
                    .and_then(|alternate| walk_alternate(alternate, state, enter, leave))
            })
        }

        fn walk_node<'a, T, S, E, L>(
            node: &'a Node,
            state: &mut S,
            enter: &mut E,
            leave: &mut L,
        ) -> Option<T>
        where
            E: FnMut(Entry<'a>, &mut S) -> Search<T>,
            L: FnMut(Entry<'a>, &mut S),
        {
            walk_entry(Entry::Node(node), state, enter, leave)
        }

        fn walk_node_children<'a, T, S, E, L>(
            node: &'a Node,
            state: &mut S,
            enter: &mut E,
            leave: &mut L,
        ) -> Option<T>
        where
            E: FnMut(Entry<'a>, &mut S) -> Search<T>,
            L: FnMut(Entry<'a>, &mut S),
        {
            match node {
                Node::IfBlock(block) => walk_if_block_children(block, state, enter, leave),
                Node::EachBlock(block) => {
                    walk_fragment(&block.body, state, enter, leave).or_else(|| {
                        block
                            .fallback
                            .as_ref()
                            .and_then(|fragment| walk_fragment(fragment, state, enter, leave))
                    })
                }
                Node::KeyBlock(block) => walk_fragment(&block.fragment, state, enter, leave),
                Node::AwaitBlock(block) => {
                    for fragment in [
                        block.pending.as_ref(),
                        block.then.as_ref(),
                        block.catch.as_ref(),
                    ] {
                        if let Some(fragment) = fragment
                            && let Some(found) = walk_fragment(fragment, state, enter, leave)
                        {
                            return Some(found);
                        }
                    }
                    None
                }
                Node::SnippetBlock(block) => walk_fragment(&block.body, state, enter, leave),
                _ => node
                    .as_element()
                    .and_then(|element| walk_fragment(element.fragment(), state, enter, leave)),
            }
        }

        let mut enter = enter;
        let mut leave = leave;
        walk_fragment(self, state, &mut enter, &mut leave)
    }

    pub fn search<'a, T, F>(&'a self, visit: F) -> Option<T>
    where
        F: FnMut(Entry<'a>, &mut ()) -> Search<T>,
    {
        self.walk(&mut (), visit, |_, _| {})
    }

    pub fn find_map<'a, T, F>(&'a self, mut find: F) -> Option<T>
    where
        F: FnMut(Entry<'a>) -> Option<T>,
    {
        self.search(|entry, _| match find(entry) {
            Some(found) => Search::Found(found),
            None => Search::Continue,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Script {
    pub r#type: ScriptType,
    pub start: usize,
    pub end: usize,
    /// Byte range of the script content (between open tag `>` and `</script>`).
    #[serde(skip_deserializing, default)]
    pub content_start: usize,
    #[serde(skip_deserializing, default)]
    pub content_end: usize,
    pub context: ScriptContext,
    #[serde(skip_deserializing, default = "empty_parsed_js_program")]
    pub content: Arc<JsProgram>,
    /// Pre-serialized ESTree JSON for the content Program node.
    #[serde(skip_deserializing, default)]
    pub content_json: Option<Arc<str>>,
    pub attributes: Box<[Attribute]>,
}

impl Serialize for Script {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &self.r#type)?;
        map.serialize_entry("start", &self.start)?;
        map.serialize_entry("end", &self.end)?;
        map.serialize_entry("context", &self.context)?;
        if let Some(ref json) = self.content_json {
            let raw = serde_json::value::RawValue::from_string(json.to_string())
                .map_err(serde::ser::Error::custom)?;
            map.serialize_entry("content", &raw)?;
        }
        map.serialize_entry("attributes", &self.attributes)?;
        map.end()
    }
}

impl Script {
    pub fn parsed_program(&self) -> &JsProgram {
        &self.content
    }

    pub fn oxc_program(&self) -> &OxcProgram<'_> {
        self.parsed_program().program()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EachBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    pub body: Fragment,
    #[serde(skip_serializing, default)]
    pub has_as_clause: bool,
    #[serde(skip_serializing, default)]
    pub invalid_key_without_as: bool,
    pub context: Option<Expression>,
    #[serde(skip_serializing, default)]
    pub context_error: Option<ParseError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<Expression>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Fragment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    pub value: Option<Expression>,
    pub error: Option<Expression>,
    pub pending: Option<Fragment>,
    pub then: Option<Fragment>,
    pub catch: Option<Fragment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    #[serde(rename = "typeParams", skip_serializing_if = "Option::is_none")]
    pub type_params: Option<Arc<str>>,
    pub parameters: Box<[Expression]>,
    pub body: Fragment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_error: Option<SnippetHeaderError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderTag {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlTag {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstTag {
    pub start: usize,
    pub end: usize,
    pub declaration: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugTag {
    pub start: usize,
    pub end: usize,
    pub arguments: Box<[Expression]>,
    pub identifiers: Box<[Identifier]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionTag {
    pub r#type: ExpressionTagType,
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

/// Serde discriminant for `"type": "ExpressionTag"` in the JSON AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpressionTagType {
    ExpressionTag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub start: usize,
    pub end: usize,
    pub data: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegularElement {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    #[serde(skip_serializing, default)]
    pub self_closing: bool,
    #[serde(skip_serializing, default)]
    pub has_end_tag: bool,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotElement {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteHead {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteBody {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteWindow {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteDocument {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteComponent {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteElement {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteSelf {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteFragment {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteBoundary {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleElement {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Attribute {
    Attribute(NamedAttribute),
    SpreadAttribute(SpreadAttribute),
    BindDirective(DirectiveAttribute),
    OnDirective(DirectiveAttribute),
    ClassDirective(DirectiveAttribute),
    LetDirective(DirectiveAttribute),
    StyleDirective(StyleDirective),
    TransitionDirective(TransitionDirective),
    AnimateDirective(DirectiveAttribute),
    UseDirective(DirectiveAttribute),
    AttachTag(AttachTag),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadAttribute {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedAttribute {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub value: AttributeValueKind,
    #[serde(skip_serializing, default)]
    pub value_syntax: AttributeValueSyntax,
    #[serde(skip_serializing, default)]
    pub error: Option<AttrError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValueKind {
    Boolean(bool),
    Values(Box<[AttributeValue]>),
    ExpressionTag(ExpressionTag),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AttributeValue {
    Text(Text),
    ExpressionTag(ExpressionTag),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectiveAttribute {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub expression: Expression,
    pub modifiers: Box<[Arc<str>]>,
    #[serde(skip_serializing, default)]
    pub value_syntax: DirectiveValueSyntax,
    #[serde(skip_serializing, default)]
    pub value_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDirective {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub modifiers: Box<[Arc<str>]>,
    pub value: AttributeValueKind,
    #[serde(skip_serializing, default)]
    pub value_syntax: AttributeValueSyntax,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDirective {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: SourceRange,
    pub expression: Expression,
    pub modifiers: Box<[Arc<str>]>,
    pub intro: bool,
    pub outro: bool,
    #[serde(skip_serializing, default)]
    pub value_syntax: DirectiveValueSyntax,
    #[serde(skip_serializing, default)]
    pub value_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachTag {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IfBlock {
    pub elseif: bool,
    pub start: usize,
    pub end: usize,
    pub test: Expression,
    pub consequent: Fragment,
    pub alternate: Option<Box<Alternate>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Alternate {
    Fragment(Fragment),
    IfBlock(IfBlock),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionSyntax {
    #[serde(skip_serializing, default)]
    pub parens: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsNodeHandle {
    Expression(Arc<JsExpression>),
    SequenceItem {
        root: Arc<JsExpression>,
        index: usize,
    },
    Pattern(Arc<JsPattern>),
    ParameterItem {
        parameters: Arc<JsParameters>,
        index: usize,
    },
    RestParameter(Arc<JsParameters>),
    StatementInProgram {
        program: Arc<JsProgram>,
        index: usize,
    },
}

/// A JS comment extracted from expression source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsComment {
    pub kind: JsCommentKind,
    pub value: Arc<str>,
    /// Absolute source position (start). `None` for synthetic comments (e.g. HTML comments).
    pub start: Option<usize>,
    /// Absolute source position (end). `None` for synthetic comments.
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsCommentKind {
    Line,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Expression {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing, default)]
    pub syntax: ExpressionSyntax,
    #[serde(skip_serializing, skip_deserializing, default)]
    pub node: Option<JsNodeHandle>,
    /// Pre-computed ESTree JSON with loc fields injected from the full source.
    /// Set by `Root::enrich_expressions` after parsing.
    #[serde(skip_serializing, skip_deserializing, default)]
    pub enriched_json: Option<Arc<str>>,
    /// Leading JS comments extracted from the expression source text.
    #[serde(skip_serializing, skip_deserializing, default)]
    pub leading_comments: Vec<JsComment>,
    /// Trailing JS comments extracted from the expression source text.
    #[serde(skip_serializing, skip_deserializing, default)]
    pub trailing_comments: Vec<JsComment>,
}

impl Expression {
    pub fn empty(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: None,
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    /// Return `true` if this expression has no content (zero-length span, no node).
    pub fn is_empty(&self) -> bool {
        self.node.is_none() && self.start == self.end
    }

    pub fn from_expression(parsed: Arc<JsExpression>, start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::Expression(parsed)),
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    pub fn from_sequence_item(
        root: Arc<JsExpression>,
        index: usize,
        start: usize,
        end: usize,
    ) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::SequenceItem { root, index }),
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    pub fn from_pattern(parsed: Arc<JsPattern>, start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::Pattern(parsed)),
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    pub fn from_parameter_item(
        parameters: Arc<JsParameters>,
        index: usize,
        start: usize,
        end: usize,
    ) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::ParameterItem { parameters, index }),
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    pub fn from_rest_parameter(parameters: Arc<JsParameters>, start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::RestParameter(parameters)),
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    pub fn from_statement(
        program: Arc<JsProgram>,
        index: usize,
        start: usize,
        end: usize,
    ) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::StatementInProgram { program, index }),
            enriched_json: None,
            leading_comments: Vec::new(),
            trailing_comments: Vec::new(),
        }
    }

    pub fn parens(&self) -> u16 {
        self.syntax.parens.max(self.outer_parens())
    }

    pub fn is_parenthesized(&self) -> bool {
        self.parens() != 0
    }

    fn outer_parens(&self) -> u16 {
        let mut parens = 0u16;
        let mut current = match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => parsed.expression(),
            Some(JsNodeHandle::SequenceItem { root, .. }) => root.expression(),
            Some(JsNodeHandle::Pattern(_))
            | Some(JsNodeHandle::ParameterItem { .. })
            | Some(JsNodeHandle::RestParameter(_))
            | Some(JsNodeHandle::StatementInProgram { .. })
            | None => return 0,
        };

        while let OxcExpression::ParenthesizedExpression(parenthesized) = current {
            parens = parens.saturating_add(1);
            current = &parenthesized.expression;
        }

        parens
    }

    pub fn parsed(&self) -> Option<&JsExpression> {
        match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => Some(parsed),
            Some(JsNodeHandle::SequenceItem { root, .. }) => Some(root),
            Some(JsNodeHandle::Pattern(_))
            | Some(JsNodeHandle::ParameterItem { .. })
            | Some(JsNodeHandle::RestParameter(_))
            | Some(JsNodeHandle::StatementInProgram { .. })
            | None => None,
        }
    }

    pub fn oxc_expression_raw(&self) -> Option<&OxcExpression<'_>> {
        match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => Some(parsed.expression()),
            Some(JsNodeHandle::SequenceItem { root, index }) => {
                let OxcExpression::SequenceExpression(sequence) = root.expression() else {
                    return None;
                };
                sequence.expressions.get(*index)
            }
            Some(JsNodeHandle::Pattern(_))
            | Some(JsNodeHandle::ParameterItem { .. })
            | Some(JsNodeHandle::RestParameter(_))
            | Some(JsNodeHandle::StatementInProgram { .. })
            | None => None,
        }
    }

    pub fn oxc_expression(&self) -> Option<&OxcExpression<'_>> {
        let mut expression = self.oxc_expression_raw()?;

        while let OxcExpression::ParenthesizedExpression(parenthesized) = expression {
            expression = &parenthesized.expression;
        }

        Some(expression)
    }

    /// Returns `true` if this expression represents a destructured pattern
    /// (ObjectPattern or ArrayPattern).
    pub fn is_destructured_pattern(&self) -> bool {
        self.oxc_pattern().is_some_and(|p| {
            matches!(
                p,
                BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_)
            )
        })
    }

    pub fn oxc_pattern(&self) -> Option<&BindingPattern<'_>> {
        match &self.node {
            Some(JsNodeHandle::Pattern(parsed)) => Some(parsed.pattern()),
            Some(JsNodeHandle::ParameterItem { parameters, index }) => {
                Some(&parameters.parameter(*index)?.pattern)
            }
            Some(JsNodeHandle::RestParameter(parameters)) => {
                Some(&parameters.rest_parameter()?.rest.argument)
            }
            _ => None,
        }
    }

    pub fn oxc_parameter(&self) -> Option<&FormalParameter<'_>> {
        match &self.node {
            Some(JsNodeHandle::ParameterItem { parameters, index }) => parameters.parameter(*index),
            _ => None,
        }
    }

    pub fn oxc_rest_parameter(&self) -> Option<&FormalParameterRest<'_>> {
        match &self.node {
            Some(JsNodeHandle::RestParameter(parameters)) => parameters.rest_parameter(),
            _ => None,
        }
    }

    pub fn oxc_statement(&self) -> Option<&OxcStatement<'_>> {
        match &self.node {
            Some(JsNodeHandle::StatementInProgram { program, index }) => program.statement(*index),
            _ => None,
        }
    }

    pub fn oxc_variable_declaration(&self) -> Option<&OxcVariableDeclaration<'_>> {
        match &self.node {
            Some(JsNodeHandle::StatementInProgram { program, index }) => {
                program.variable_declaration(*index)
            }
            _ => None,
        }
    }

    pub fn is_rest_parameter(&self) -> bool {
        matches!(self.node, Some(JsNodeHandle::RestParameter(_)))
    }

    pub fn source_snippet(&self) -> Option<&str> {
        match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => Some(parsed.source()),
            Some(JsNodeHandle::SequenceItem { root, index }) => {
                let OxcExpression::SequenceExpression(sequence) = root.expression() else {
                    return None;
                };
                let node = sequence.expressions.get(*index)?;
                root.source().get(node.span().start as usize..node.span().end as usize)
            }
            Some(JsNodeHandle::Pattern(parsed)) => Some(parsed.source()),
            Some(JsNodeHandle::ParameterItem { parameters, index }) => {
                let parameter = parameters.parameter(*index)?;
                parameters
                    .source()
                    .get(parameter.span.start as usize - 1..parameter.span.end as usize - 1)
            }
            Some(JsNodeHandle::RestParameter(parameters)) => {
                let parameter = parameters.rest_parameter()?;
                parameters
                    .source()
                    .get(parameter.span.start as usize - 1..parameter.span.end as usize - 1)
            }
            Some(JsNodeHandle::StatementInProgram { program, index }) => program.statement_source(*index),
            None => None,
        }
    }

    pub fn identifier_name(&self) -> Option<Arc<str>> {
        if let Some(identifier) = self
            .oxc_expression()
            .and_then(OxcExpression::get_identifier_reference)
        {
            return Some(Arc::from(identifier.name.as_str()));
        }

        match self.oxc_pattern()? {
            BindingPattern::BindingIdentifier(identifier) => {
                Some(Arc::from(identifier.name.as_str()))
            }
            _ => None,
        }
    }

    pub fn literal_string(&self) -> Option<Arc<str>> {
        match self.oxc_expression()? {
            OxcExpression::StringLiteral(value) => Some(Arc::from(value.value.as_str())),
            _ => None,
        }
    }

    pub fn literal_bool(&self) -> Option<bool> {
        match self.oxc_expression()? {
            OxcExpression::BooleanLiteral(value) => Some(value.value),
            _ => None,
        }
    }

    pub fn binding_identifier(&self) -> Option<&BindingIdentifier<'_>> {
        if let Some(declaration) = self.oxc_variable_declaration() {
            let [declarator] = declaration.declarations.as_slice() else {
                return None;
            };
            return declarator.id.get_binding_identifier();
        }

        match self.oxc_pattern()? {
            BindingPattern::BindingIdentifier(identifier) => Some(identifier),
            _ => None,
        }
    }
}

impl Span for Expression {
    fn start(&self) -> usize {
        self.start
    }

    fn end(&self) -> usize {
        self.end
    }
}

impl Serialize for Expression {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Prefer enriched JSON (with loc fields) if available.
        if let Some(ref enriched) = self.enriched_json {
            let raw = serde_json::value::RawValue::from_string(enriched.to_string())
                .map_err(serde::ser::Error::custom)?;
            return raw.serialize(serializer);
        }
        if let Some(raw_json) = self.to_estree_json() {
            // Embed pre-serialized JSON directly via RawValue — no re-parsing.
            let raw = serde_json::value::RawValue::from_string(raw_json)
                .map_err(serde::ser::Error::custom)?;
            raw.serialize(serializer)
        } else {
            // Fallback: emit empty Identifier when no OXC node is available.
            // Upstream produces {type: "Identifier", name: "", start, end} for
            // broken or empty expressions in loose mode.
            let mut map = serializer.serialize_map(Some(4))?;
            map.serialize_entry("type", "Identifier")?;
            map.serialize_entry("name", "")?;
            map.serialize_entry("start", &self.start)?;
            map.serialize_entry("end", &self.end)?;
            map.end()
        }
    }
}

impl Expression {
    /// Compute and store the enriched ESTree JSON with `loc` fields.
    /// Uses the full source to compute line/column information.
    /// `column_offset` is added to loc columns for nodes not on line 1
    /// (used for destructured patterns to match upstream's wrapping behavior).
    pub fn enrich_with_source(&mut self, full_source: &str) {
        self.enrich_inner(full_source, 0, false);
    }

    pub fn enrich_with_source_and_column_offset(
        &mut self,
        full_source: &str,
        column_offset: usize,
    ) {
        self.enrich_inner(full_source, column_offset, false);
    }

    /// Like `enrich_with_source` but adds `character` (byte offset) to loc objects.
    /// Used for SnippetBlock expression names where upstream includes character.
    pub fn enrich_with_character(&mut self, full_source: &str) {
        self.enrich_inner(full_source, 0, true);
    }

    fn enrich_inner(
        &mut self,
        full_source: &str,
        column_offset: usize,
        with_character: bool,
    ) {
        let Some(raw_json) = self.serialize_oxc_node() else {
            // For empty expressions (no OXC node), we can still produce enriched JSON
            // with loc fields if character mode is requested (loose mode).
            if with_character {
                self.enrich_empty_with_character(full_source);
            }
            return;
        };
        let offset = self.oxc_span_offset();
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw_json) else {
            return;
        };
        // Fix TemplateElement spans for TS-serialized expressions.
        // Only Expression handles use the TS serializer; Pattern/Statement use JS.
        if self.uses_ts_serializer() {
            crate::js::fix_template_element_spans(&mut value);
        }
        if with_character {
            crate::js::adjust_expression_json_with_character(
                &mut value,
                full_source,
                offset,
            );
        } else {
            crate::js::adjust_expression_json_with_column_offset(
                &mut value,
                full_source,
                offset,
                column_offset,
            );
        }
        // Inject leadingComments if present
        if !self.leading_comments.is_empty() {
            if let serde_json::Value::Object(ref mut map) = value {
                map.insert(
                    "leadingComments".to_string(),
                    serde_json::Value::Array(crate::estree::make_comment_json(&self.leading_comments)),
                );
            }
        }
        // Inject trailingComments if present
        if !self.trailing_comments.is_empty() {
            if let serde_json::Value::Object(ref mut map) = value {
                map.insert(
                    "trailingComments".to_string(),
                    serde_json::Value::Array(crate::estree::make_comment_json(&self.trailing_comments)),
                );
            }
        }
        // Attach internal comments (comments within function bodies, etc.)
        let internal_comments = self.extract_internal_comments();
        if !internal_comments.is_empty() {
            crate::js::attach_comments_to_json_tree(&mut value, &internal_comments, full_source);
        }
        if let Ok(enriched) = serde_json::to_string(&value) {
            self.enriched_json = Some(Arc::from(enriched));
        }
    }

    /// Check if this expression uses the TS serializer (for TemplateElement span fixing).
    fn uses_ts_serializer(&self) -> bool {
        matches!(
            &self.node,
            Some(JsNodeHandle::Expression(_))
                | Some(JsNodeHandle::SequenceItem { .. })
                | Some(JsNodeHandle::ParameterItem { .. })
                | Some(JsNodeHandle::RestParameter(_))
                | Some(JsNodeHandle::StatementInProgram { .. })
        )
    }

    /// Generate enriched JSON for an empty expression (no OXC node) with loc+character.
    fn enrich_empty_with_character(&mut self, full_source: &str) {
        let start = self.start;
        let end = self.end;
        let (sl, sc) = crate::line_column_at_offset(full_source, start);
        let (el, ec) = crate::line_column_at_offset(full_source, end);
        let json = format!(
            r#"{{"type":"Identifier","name":"","start":{},"end":{},"loc":{{"start":{{"line":{},"column":{},"character":{}}},"end":{{"line":{},"column":{},"character":{}}}}}}}"#,
            start, end, sl, sc, start, el, ec, end
        );
        self.enriched_json = Some(Arc::from(json));
    }

    /// Serialize this expression to an ESTree JSON string using OXC's serializer,
    /// with span offsets adjusted from OXC-local to Svelte source coordinates.
    /// Single-pass string-level adjustment, no intermediate tree parsing.
    ///
    /// Returns `None` if no OXC node handle is attached.
    pub fn to_estree_json(&self) -> Option<String> {
        let json = self.serialize_oxc_node()?;
        let offset = self.oxc_span_offset();
        if offset == 0 {
            Some(json)
        } else {
            Some(adjust_estree_span_offsets(&json, offset))
        }
    }

    /// Serialize the underlying OXC node to a JSON string via oxc_estree.
    fn serialize_oxc_node(&self) -> Option<String> {
        use oxc_estree::ESTree;

        match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => {
                let mut ser = oxc_estree::CompactTSSerializer::new(false);
                serialize_oxc_expression_unwrapped_ts(parsed.expression(), &mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::SequenceItem { root, index }) => {
                let OxcExpression::SequenceExpression(seq) = root.expression() else {
                    return None;
                };
                let expr = seq.expressions.get(*index)?;
                let mut ser = oxc_estree::CompactTSSerializer::new(false);
                expr.serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::Pattern(parsed)) => {
                let mut ser = oxc_estree::CompactJSSerializer::new(false);
                parsed.pattern().serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::ParameterItem { parameters, index }) => {
                let param = parameters.parameter(*index)?;
                // Use TS serializer so typeAnnotation is included
                let mut ser = oxc_estree::CompactTSSerializer::new(false);
                param.serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::RestParameter(parameters)) => {
                let rest = parameters.rest_parameter()?;
                // Use TS serializer so typeAnnotation is included
                let mut ser = oxc_estree::CompactTSSerializer::new(false);
                rest.serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::StatementInProgram { program, index }) => {
                let stmt = program.statement(*index)?;
                let mut ser = oxc_estree::CompactTSSerializer::new(false);
                stmt.serialize(&mut ser);
                Some(ser.into_string())
            }
            None => None,
        }
    }

    /// Compute the offset to add to OXC span values to get Svelte source positions.
    fn oxc_span_offset(&self) -> i64 {
        match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => {
                self.start as i64 - parsed.expression().span().start as i64
            }
            Some(JsNodeHandle::SequenceItem { root, index }) => {
                if let OxcExpression::SequenceExpression(seq) = root.expression()
                    && let Some(expr) = seq.expressions.get(*index)
                {
                    return self.start as i64 - expr.span().start as i64;
                }
                0
            }
            Some(JsNodeHandle::Pattern(parsed)) => {
                self.start as i64 - parsed.pattern().span().start as i64
            }
            Some(JsNodeHandle::ParameterItem { parameters, index }) => {
                if let Some(param) = parameters.parameter(*index) {
                    return self.start as i64 - param.span.start as i64;
                }
                0
            }
            Some(JsNodeHandle::RestParameter(parameters)) => {
                if let Some(rest) = parameters.rest_parameter() {
                    return self.start as i64 - rest.span.start as i64;
                }
                0
            }
            Some(JsNodeHandle::StatementInProgram { program, index }) => {
                if let Some(stmt) = program.statement(*index) {
                    return self.start as i64 - stmt.span().start as i64;
                }
                0
            }
            None => 0,
        }
    }
}

impl Expression {
    /// Scan the expression's source text and extract all internal JS comments.
    /// Returns (absolute_start, absolute_end, json_value) tuples suitable for
    /// `attach_comments_to_json_tree`.
    pub fn extract_internal_comments(&self) -> Vec<(u32, u32, serde_json::Value)> {
        let source = match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => parsed.source(),
            _ => return Vec::new(),
        };
        let offset = self.oxc_span_offset();
        let mut comments = Vec::new();
        let bytes = source.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            match bytes[i] {
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                    let start = i;
                    i += 2;
                    let value_start = i;
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    let value = &source[value_start..i];
                    let abs_start = (start as i64 + offset) as u32;
                    let abs_end = (i as i64 + offset) as u32;
                    comments.push((
                        abs_start,
                        abs_end,
                        crate::estree::EstreeComment {
                            kind: crate::estree::EstreeCommentKind::Line,
                            value: value.to_string(),
                            start: Some(abs_start as usize),
                            end: Some(abs_end as usize),
                        }
                        .to_json_value(),
                    ));
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                    let start = i;
                    i += 2;
                    let value_start = i;
                    while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    let value_end = i;
                    if i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i = bytes.len();
                    }
                    let value = &source[value_start..value_end];
                    let abs_start = (start as i64 + offset) as u32;
                    let abs_end = (i as i64 + offset) as u32;
                    comments.push((
                        abs_start,
                        abs_end,
                        crate::estree::EstreeComment {
                            kind: crate::estree::EstreeCommentKind::Block,
                            value: value.to_string(),
                            start: Some(abs_start as usize),
                            end: Some(abs_end as usize),
                        }
                        .to_json_value(),
                    ));
                }
                b'"' => {
                    // Skip string literals
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                b'\'' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'\'' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                b'`' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'`' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }

        comments
    }
}

/// Serialize an OXC expression, unwrapping ParenthesizedExpression nodes.
/// Svelte tracks parens via `Expression.syntax.parens`, not in the AST.
/// Uses TS serializer to emit typeAnnotation on params/identifiers.
fn serialize_oxc_expression_unwrapped_ts(
    expr: &OxcExpression<'_>,
    ser: &mut oxc_estree::CompactTSSerializer,
) {
    use oxc_estree::ESTree;
    let mut inner = expr;
    while let OxcExpression::ParenthesizedExpression(paren) = inner {
        inner = &paren.expression;
    }
    inner.serialize(ser);
}


/// Single-pass adjustment of `"start":N` and `"end":N` span values in a JSON string.
///
/// In ESTree JSON from oxc_estree, these keys with numeric values always represent
/// byte-position spans. This is safe because string values containing `"start":` are
/// escaped (`\"start\":`) in JSON, so the pattern cannot appear inside string values.
fn adjust_estree_span_offsets(json: &str, offset: i64) -> String {
    let bytes = json.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len + 64);
    let mut i = 0;

    while i < len {
        let remaining = &bytes[i..];

        let key_len = if remaining.starts_with(b"\"start\":") {
            8 // "start":
        } else if remaining.starts_with(b"\"end\":") {
            6 // "end":
        } else {
            0
        };

        if key_len > 0 {
            // Verify preceding char is a struct delimiter (not inside a string value)
            let before_ok = i == 0 || matches!(bytes[i - 1], b',' | b'{' | b'\n' | b' ');
            let num_start = i + key_len;

            if before_ok && num_start < len && bytes[num_start].is_ascii_digit() {
                let mut num_end = num_start;
                while num_end < len && bytes[num_end].is_ascii_digit() {
                    num_end += 1;
                }

                if let Ok(val) = json[num_start..num_end].parse::<i64>() {
                    let adjusted = (val + offset).max(0) as u64;
                    result.push_str(&json[i..i + key_len]);
                    result.push_str(&adjusted.to_string());
                    i = num_end;
                    continue;
                }
            }
        }

        // Copy single byte (ASCII in JSON keys/structure)
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identifier {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<Loc>,
    pub name: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Literal {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<Loc>,
    pub value: LiteralValue,
    pub raw: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryExpression {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<Loc>,
    pub left: Box<Expression>,
    pub operator: Arc<str>,
    pub right: Box<Expression>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallExpression {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<Loc>,
    pub callee: Box<Expression>,
    pub arguments: Box<[Expression]>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub start: usize,
    pub end: usize,
    pub raw: Arc<str>,
    pub data: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Css {
    pub r#type: CssType,
    pub start: usize,
    pub end: usize,
    pub attributes: Box<[Attribute]>,
    pub children: Box<[CssNode]>,
    pub content: CssContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssType {
    StyleSheet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssContent {
    pub start: usize,
    pub end: usize,
    pub styles: Arc<str>,
    pub comment: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CssNode {
    Rule(CssRule),
    Atrule(CssAtrule),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssRule {
    pub prelude: CssSelectorList,
    pub block: CssBlock,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssAtrule {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub prelude: Arc<str>,
    pub block: Option<CssBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssBlock {
    pub r#type: CssBlockType,
    pub start: usize,
    pub end: usize,
    pub children: Box<[CssBlockChild]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssBlockType {
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CssBlockChild {
    Declaration(CssDeclaration),
    Rule(CssRule),
    Atrule(CssAtrule),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssDeclaration {
    pub start: usize,
    pub end: usize,
    pub property: Arc<str>,
    pub value: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssSelectorList {
    pub r#type: CssSelectorListType,
    pub start: usize,
    pub end: usize,
    pub children: Box<[CssComplexSelector]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssSelectorListType {
    SelectorList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssComplexSelector {
    pub r#type: CssComplexSelectorType,
    pub start: usize,
    pub end: usize,
    pub children: Box<[CssRelativeSelector]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssComplexSelectorType {
    ComplexSelector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssRelativeSelector {
    pub r#type: CssRelativeSelectorType,
    pub combinator: Option<CssCombinator>,
    pub selectors: Box<[CssSimpleSelector]>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssRelativeSelectorType {
    RelativeSelector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssCombinator {
    pub r#type: CssCombinatorType,
    pub name: Arc<str>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssCombinatorType {
    Combinator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CssSimpleSelector {
    TypeSelector(CssNameSelector),
    IdSelector(CssNameSelector),
    ClassSelector(CssNameSelector),
    PseudoElementSelector(CssNameSelector),
    PseudoClassSelector(CssPseudoClassSelector),
    AttributeSelector(CssAttributeSelector),
    Nth(CssValueSelector),
    Percentage(CssValueSelector),
    NestingSelector(CssNameSelector),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssNameSelector {
    pub name: Arc<str>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssValueSelector {
    pub value: Arc<str>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssPseudoClassSelector {
    pub name: Arc<str>,
    pub args: Option<CssSelectorList>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssAttributeSelector {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub matcher: Option<Arc<str>>,
    pub value: Option<Arc<str>>,
    pub flags: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    pub start: usize,
    pub end: usize,
    pub attributes: Box<[Attribute]>,
    #[serde(skip_serializing, skip_deserializing, default)]
    pub fragment: Fragment,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "customElement")]
    pub custom_element: Option<CustomElement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runes: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomElement {
    pub tag: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Node {
    Text(Text),
    IfBlock(IfBlock),
    EachBlock(EachBlock),
    KeyBlock(KeyBlock),
    AwaitBlock(AwaitBlock),
    SnippetBlock(SnippetBlock),
    RenderTag(RenderTag),
    HtmlTag(HtmlTag),
    ConstTag(ConstTag),
    DebugTag(DebugTag),
    ExpressionTag(ExpressionTag),
    Comment(Comment),
    RegularElement(RegularElement),
    Component(Component),
    SlotElement(SlotElement),
    SvelteHead(SvelteHead),
    SvelteBody(SvelteBody),
    SvelteWindow(SvelteWindow),
    SvelteDocument(SvelteDocument),
    SvelteComponent(SvelteComponent),
    SvelteElement(SvelteElement),
    SvelteSelf(SvelteSelf),
    SvelteFragment(SvelteFragment),
    SvelteBoundary(SvelteBoundary),
    TitleElement(TitleElement),
}

pub trait HasFragment {
    fn fragment(&self) -> &Fragment;
}

pub trait Element: Span + HasFragment {
    fn name(&self) -> &str;
    fn name_loc(&self) -> &SourceRange;
    fn attributes(&self) -> &[Attribute];
    fn expression(&self) -> Option<&Expression> {
        None
    }
    fn self_closing(&self) -> bool {
        false
    }
}

macro_rules! impl_element {
    ($($ty:ty),* $(,)?) => {
        $(
            impl HasFragment for $ty {
                fn fragment(&self) -> &Fragment { &self.fragment }
            }

            impl Element for $ty {
                fn name(&self) -> &str { &self.name }
                fn name_loc(&self) -> &SourceRange { &self.name_loc }
                fn attributes(&self) -> &[Attribute] { &self.attributes }
            }
        )*
    };
}

impl_element!(
    Component,
    SlotElement,
    SvelteHead,
    SvelteBody,
    SvelteWindow,
    SvelteDocument,
    SvelteSelf,
    SvelteFragment,
    SvelteBoundary,
    TitleElement,
);

impl Element for RegularElement {
    fn name(&self) -> &str {
        &self.name
    }
    fn name_loc(&self) -> &SourceRange {
        &self.name_loc
    }
    fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    fn self_closing(&self) -> bool {
        self.self_closing
    }
}

impl HasFragment for RegularElement {
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
}

impl HasFragment for SvelteComponent {
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
}

impl Element for SvelteComponent {
    fn name(&self) -> &str {
        &self.name
    }
    fn name_loc(&self) -> &SourceRange {
        &self.name_loc
    }
    fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    fn expression(&self) -> Option<&Expression> {
        self.expression.as_ref()
    }
}

impl HasFragment for SvelteElement {
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
}

impl Element for SvelteElement {
    fn name(&self) -> &str {
        &self.name
    }
    fn name_loc(&self) -> &SourceRange {
        &self.name_loc
    }
    fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    fn expression(&self) -> Option<&Expression> {
        self.expression.as_ref()
    }
}

impl HasFragment for Root {
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
}

impl HasFragment for KeyBlock {
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
}

impl HasFragment for SnippetBlock {
    fn fragment(&self) -> &Fragment {
        &self.body
    }
}

impl Alternate {
    pub fn try_for_each_fragment<B>(
        &self,
        mut visit: impl FnMut(&Fragment) -> ControlFlow<B>,
    ) -> ControlFlow<B> {
        match self {
            Self::Fragment(fragment) => visit(fragment),
            Self::IfBlock(block) => {
                visit(&block.consequent)?;
                if let Some(alternate) = block.alternate.as_deref() {
                    alternate.try_for_each_fragment(visit)
                } else {
                    ControlFlow::Continue(())
                }
            }
        }
    }

    pub fn for_each_fragment(&self, mut visit: impl FnMut(&Fragment)) {
        let _ = self.try_for_each_fragment(|fragment| {
            visit(fragment);
            ControlFlow::<()>::Continue(())
        });
    }
}

impl Node {
    pub fn as_element(&self) -> Option<&dyn Element> {
        match self {
            Node::RegularElement(el) => Some(el),
            Node::Component(el) => Some(el),
            Node::SlotElement(el) => Some(el),
            Node::SvelteHead(el) => Some(el),
            Node::SvelteBody(el) => Some(el),
            Node::SvelteWindow(el) => Some(el),
            Node::SvelteDocument(el) => Some(el),
            Node::SvelteComponent(el) => Some(el),
            Node::SvelteElement(el) => Some(el),
            Node::SvelteSelf(el) => Some(el),
            Node::SvelteFragment(el) => Some(el),
            Node::SvelteBoundary(el) => Some(el),
            Node::TitleElement(el) => Some(el),
            _ => None,
        }
    }

    pub fn try_for_each_child_fragment<B>(
        &self,
        mut visit: impl FnMut(&Fragment) -> ControlFlow<B>,
    ) -> ControlFlow<B> {
        match self {
            Self::IfBlock(block) => {
                visit(&block.consequent)?;
                if let Some(alternate) = block.alternate.as_deref() {
                    alternate.try_for_each_fragment(visit)
                } else {
                    ControlFlow::Continue(())
                }
            }
            Self::EachBlock(block) => {
                visit(&block.body)?;
                if let Some(fallback) = block.fallback.as_ref() {
                    visit(fallback)?;
                }
                ControlFlow::Continue(())
            }
            Self::KeyBlock(block) => visit(block.fragment()),
            Self::AwaitBlock(block) => {
                for fragment in [
                    block.pending.as_ref(),
                    block.then.as_ref(),
                    block.catch.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    visit(fragment)?;
                }
                ControlFlow::Continue(())
            }
            Self::SnippetBlock(block) => visit(block.fragment()),
            _ => self
                .as_element()
                .map_or(ControlFlow::Continue(()), |element| {
                    visit(element.fragment())
                }),
        }
    }

    pub fn for_each_child_fragment(&self, mut visit: impl FnMut(&Fragment)) {
        let _ = self.try_for_each_child_fragment(|fragment| {
            visit(fragment);
            ControlFlow::<()>::Continue(())
        });
    }
}

/// Visitor trait for mutable AST walks.
///
/// Default implementations recurse into children. Override `visit_node` or
/// `visit_attribute` for per-variant customisation, calling `walk_node_children`
/// / `walk_attribute_expressions` for the default recursion.
pub trait NodeVisitorMut {
    /// Called for every `Expression` leaf.
    fn visit_expression(&mut self, expr: &mut Expression);

    /// Called for every `Node`. Default: recurse into children.
    fn visit_node(&mut self, node: &mut Node) {
        self.walk_node_children(node);
    }

    /// Called for every `Fragment`. Default: visit each child node.
    fn visit_fragment(&mut self, fragment: &mut Fragment) {
        for node in fragment.nodes.iter_mut() {
            self.visit_node(node);
        }
    }

    /// Called for every `Attribute`. Default: visit contained expressions.
    fn visit_attribute(&mut self, attr: &mut Attribute) {
        self.walk_attribute_expressions(attr);
    }

    /// Walk all children of a node, dispatching to `visit_expression`,
    /// `visit_fragment`, and `visit_attribute` as appropriate.
    ///
    /// This is the single canonical match over all `Node` variants.
    fn walk_node_children(&mut self, node: &mut Node) {
        match node {
            Node::ExpressionTag(tag) => {
                self.visit_expression(&mut tag.expression);
            }
            Node::IfBlock(block) => {
                self.visit_expression(&mut block.test);
                self.visit_fragment(&mut block.consequent);
                if let Some(ref mut alt) = block.alternate {
                    self.walk_alternate(alt);
                }
            }
            Node::EachBlock(block) => {
                self.visit_expression(&mut block.expression);
                if let Some(ref mut ctx) = block.context {
                    self.visit_expression(ctx);
                }
                if let Some(ref mut key) = block.key {
                    self.visit_expression(key);
                }
                self.visit_fragment(&mut block.body);
                if let Some(ref mut fallback) = block.fallback {
                    self.visit_fragment(fallback);
                }
            }
            Node::KeyBlock(block) => {
                self.visit_expression(&mut block.expression);
                self.visit_fragment(&mut block.fragment);
            }
            Node::AwaitBlock(block) => {
                self.visit_expression(&mut block.expression);
                if let Some(ref mut v) = block.value {
                    self.visit_expression(v);
                }
                if let Some(ref mut e) = block.error {
                    self.visit_expression(e);
                }
                if let Some(ref mut f) = block.pending {
                    self.visit_fragment(f);
                }
                if let Some(ref mut f) = block.then {
                    self.visit_fragment(f);
                }
                if let Some(ref mut f) = block.catch {
                    self.visit_fragment(f);
                }
            }
            Node::SnippetBlock(block) => {
                self.visit_expression(&mut block.expression);
                for param in block.parameters.iter_mut() {
                    self.visit_expression(param);
                }
                self.visit_fragment(&mut block.body);
            }
            Node::RenderTag(tag) => {
                self.visit_expression(&mut tag.expression);
            }
            Node::HtmlTag(tag) => {
                self.visit_expression(&mut tag.expression);
            }
            Node::ConstTag(tag) => {
                self.visit_expression(&mut tag.declaration);
            }
            Node::DebugTag(tag) => {
                for expr in tag.arguments.iter_mut() {
                    self.visit_expression(expr);
                }
            }
            // Element-like nodes: attributes + fragment
            Node::RegularElement(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::Component(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SlotElement(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteHead(el) => {
                // SvelteHead: only fragment, no attribute enrichment
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteBody(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteWindow(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteDocument(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteComponent(el) => {
                if let Some(ref mut expr) = el.expression {
                    self.visit_expression(expr);
                }
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteElement(el) => {
                if let Some(ref mut expr) = el.expression {
                    self.visit_expression(expr);
                }
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteSelf(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteFragment(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::SvelteBoundary(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::TitleElement(el) => {
                for attr in el.attributes.iter_mut() { self.visit_attribute(attr); }
                self.visit_fragment(&mut el.fragment);
            }
            Node::Text(_) | Node::Comment(_) => {}
        }
    }

    /// Walk an `Alternate` branch (else / else-if chain).
    fn walk_alternate(&mut self, alt: &mut Alternate) {
        match alt {
            Alternate::Fragment(f) => self.visit_fragment(f),
            Alternate::IfBlock(block) => {
                self.visit_expression(&mut block.test);
                self.visit_fragment(&mut block.consequent);
                if let Some(ref mut inner) = block.alternate {
                    self.walk_alternate(inner);
                }
            }
        }
    }

    /// Walk all expressions inside an `Attribute`.
    ///
    /// This is the single canonical match over all `Attribute` variants.
    fn walk_attribute_expressions(&mut self, attr: &mut Attribute) {
        match attr {
            Attribute::Attribute(a) => {
                match &mut a.value {
                    AttributeValueKind::ExpressionTag(tag) => {
                        self.visit_expression(&mut tag.expression);
                    }
                    AttributeValueKind::Values(values) => {
                        for val in values.iter_mut() {
                            if let AttributeValue::ExpressionTag(tag) = val {
                                self.visit_expression(&mut tag.expression);
                            }
                        }
                    }
                    AttributeValueKind::Boolean(_) => {}
                }
            }
            Attribute::SpreadAttribute(a) => {
                self.visit_expression(&mut a.expression);
            }
            Attribute::OnDirective(d)
            | Attribute::BindDirective(d)
            | Attribute::ClassDirective(d)
            | Attribute::LetDirective(d)
            | Attribute::AnimateDirective(d)
            | Attribute::UseDirective(d) => {
                self.visit_expression(&mut d.expression);
            }
            Attribute::StyleDirective(d) => {
                match &mut d.value {
                    AttributeValueKind::ExpressionTag(tag) => {
                        self.visit_expression(&mut tag.expression);
                    }
                    AttributeValueKind::Values(values) => {
                        for val in values.iter_mut() {
                            if let AttributeValue::ExpressionTag(tag) = val {
                                self.visit_expression(&mut tag.expression);
                            }
                        }
                    }
                    AttributeValueKind::Boolean(_) => {}
                }
            }
            Attribute::TransitionDirective(d) => {
                self.visit_expression(&mut d.expression);
            }
            Attribute::AttachTag(a) => {
                self.visit_expression(&mut a.expression);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootComment {
    pub r#type: RootCommentType,
    pub start: usize,
    pub end: usize,
    pub value: Arc<str>,
    pub loc: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub css: Option<Css>,
    #[serde(skip_serializing, default)]
    pub styles: Box<[Css]>,
    pub js: Box<[Script]>,
    #[serde(skip_serializing, default)]
    pub scripts: Box<[Script]>,
    pub start: usize,
    pub end: usize,
    pub r#type: RootType,
    pub fragment: Fragment,
    pub options: Option<Options>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<Script>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<Script>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Box<[RootComment]>>,
    #[serde(skip_serializing, default)]
    pub errors: Box<[crate::ast::common::ParseError]>,
}

impl Root {
    /// Walk all expressions in the AST and compute enriched ESTree JSON with `loc` fields.
    /// When `loose` is true, certain expression types get `character` in their loc.
    pub fn enrich_expressions(&mut self, source: &str, loose: bool) {
        enrich_fragment_expressions(&mut self.fragment, source, loose);
        if let Some(ref mut options) = self.options {
            enrich_options_expressions(options, source, loose);
        }
        // Inject HTML comments as leadingComments on Script Program nodes.
        self.inject_html_comments_into_scripts();
    }

    /// Find HTML Comment nodes in fragment that immediately precede a Script tag,
    /// and inject them as `leadingComments` on the Script's Program content_json.
    fn inject_html_comments_into_scripts(&mut self) {
        // Collect HTML comments from the fragment that appear before scripts.
        let mut html_comments_before_instance: Vec<JsComment> = Vec::new();
        let mut html_comments_before_module: Vec<JsComment> = Vec::new();

        let instance_start = self.instance.as_ref().map(|s| s.start);
        let module_start = self.module.as_ref().map(|s| s.start);

        for node in self.fragment.nodes.iter() {
            if let Node::Comment(comment) = node {
                // Check if this comment immediately precedes the instance script
                if let Some(inst_start) = instance_start {
                    if comment.end <= inst_start {
                        html_comments_before_instance.push(JsComment {
                            kind: JsCommentKind::Line,
                            value: comment.data.clone(),
                            start: None,
                            end: None,
                        });
                    }
                }
                // Check if this comment immediately precedes the module script
                if let Some(mod_start) = module_start {
                    if comment.end <= mod_start {
                        html_comments_before_module.push(JsComment {
                            kind: JsCommentKind::Line,
                            value: comment.data.clone(),
                            start: None,
                            end: None,
                        });
                    }
                }
            }
        }

        // Inject into instance content_json
        if !html_comments_before_instance.is_empty() {
            if let Some(ref mut script) = self.instance {
                inject_leading_comments_into_content_json(
                    &mut script.content_json,
                    &html_comments_before_instance,
                );
            }
        }
        if !html_comments_before_module.is_empty() {
            if let Some(ref mut script) = self.module {
                inject_leading_comments_into_content_json(
                    &mut script.content_json,
                    &html_comments_before_module,
                );
            }
        }
    }
}

fn inject_leading_comments_into_content_json(
    content_json: &mut Option<Arc<str>>,
    comments: &[JsComment],
) {
    let Some(json_str) = content_json.as_ref() else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json_str.as_ref()) else {
        return;
    };
    if let serde_json::Value::Object(ref mut map) = value {
        map.insert(
            "leadingComments".to_string(),
            serde_json::Value::Array(crate::estree::make_comment_json(comments)),
        );
    }
    if let Ok(enriched) = serde_json::to_string(&value) {
        *content_json = Some(Arc::from(enriched));
    }
}

fn enrich_expression(expr: &mut Expression, source: &str) {
    if expr.node.is_some() && expr.enriched_json.is_none() {
        expr.enrich_with_source(source);
    }
}

fn enrich_expression_with_character(expr: &mut Expression, source: &str) {
    if expr.enriched_json.is_none() {
        expr.enrich_with_character(source);
    }
}

/// Visitor that enriches all expressions in the AST with ESTree JSON + loc fields.
struct EnrichVisitor<'a> {
    source: &'a str,
    loose: bool,
}

impl NodeVisitorMut for EnrichVisitor<'_> {
    fn visit_expression(&mut self, expr: &mut Expression) {
        enrich_expression(expr, self.source);
    }

    fn visit_node(&mut self, node: &mut Node) {
        // Variants with special enrichment logic that differs from the default
        // `visit_expression` path (character mode, column offsets, etc.).
        match node {
            Node::EachBlock(block) => {
                self.visit_expression(&mut block.expression);
                if let Some(ref mut ctx) = block.context {
                    if ctx.is_destructured_pattern() {
                        // Destructured patterns need +1 column offset (matching upstream wrapping)
                        ctx.enrich_with_source_and_column_offset(self.source, 1);
                    } else if self.loose {
                        enrich_expression_with_character(ctx, self.source);
                    } else {
                        enrich_expression(ctx, self.source);
                    }
                }
                if let Some(ref mut key) = block.key {
                    self.visit_expression(key);
                }
                self.visit_fragment(&mut block.body);
                if let Some(ref mut fallback) = block.fallback {
                    self.visit_fragment(fallback);
                }
            }
            Node::AwaitBlock(block) => {
                self.visit_expression(&mut block.expression);
                // value and error need character mode in loose
                for opt_expr in [&mut block.value, &mut block.error] {
                    if let Some(expr) = opt_expr {
                        if self.loose {
                            enrich_expression_with_character(expr, self.source);
                        } else {
                            enrich_expression(expr, self.source);
                        }
                    }
                }
                if let Some(ref mut f) = block.pending {
                    self.visit_fragment(f);
                }
                if let Some(ref mut f) = block.then {
                    self.visit_fragment(f);
                }
                if let Some(ref mut f) = block.catch {
                    self.visit_fragment(f);
                }
            }
            Node::SnippetBlock(block) => {
                // SnippetBlock expression (name) uses loc with `character` field
                block.expression.enrich_with_character(self.source);
                for param in block.parameters.iter_mut() {
                    self.visit_expression(param);
                }
                self.visit_fragment(&mut block.body);
            }
            // All other variants use the default walk.
            _ => self.walk_node_children(node),
        }
    }

    fn visit_attribute(&mut self, attr: &mut Attribute) {
        // In loose mode, shorthand empty expressions like `<div {}>` get loc with `character`
        if self.loose {
            if let Attribute::Attribute(a) = attr {
                if let AttributeValueKind::ExpressionTag(tag) = &mut a.value {
                    if tag.expression.node.is_none() && a.name.is_empty() {
                        enrich_expression_with_character(&mut tag.expression, self.source);
                        return;
                    }
                }
            }
        }
        self.walk_attribute_expressions(attr);
    }
}

fn enrich_fragment_expressions(fragment: &mut Fragment, source: &str, loose: bool) {
    let mut visitor = EnrichVisitor { source, loose };
    visitor.visit_fragment(fragment);
}

fn enrich_options_expressions(options: &mut Options, source: &str, loose: bool) {
    let mut visitor = EnrichVisitor { source, loose };
    for attr in options.attributes.iter_mut() {
        visitor.visit_attribute(attr);
    }
}

macro_rules! impl_span_for_struct {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Span for $ty {
                fn start(&self) -> usize {
                    self.start
                }

                fn end(&self) -> usize {
                    self.end
                }
            }
        )*
    };
}

impl_span_for_struct!(
    Script,
    EachBlock,
    KeyBlock,
    AwaitBlock,
    SnippetBlock,
    RenderTag,
    HtmlTag,
    ConstTag,
    DebugTag,
    ExpressionTag,
    Comment,
    RegularElement,
    Component,
    SlotElement,
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
    IfBlock,
    Text,
    Css,
    CssContent,
    CssRule,
    CssAtrule,
    CssBlock,
    CssDeclaration,
    CssSelectorList,
    CssComplexSelector,
    CssRelativeSelector,
    CssCombinator,
    CssNameSelector,
    CssValueSelector,
    CssPseudoClassSelector,
    CssAttributeSelector,
    Options,
    RootComment,
    Root
);

impl Span for Node {
    fn start(&self) -> usize {
        match self {
            Node::Text(node) => node.start,
            Node::IfBlock(node) => node.start,
            Node::EachBlock(node) => node.start,
            Node::KeyBlock(node) => node.start,
            Node::AwaitBlock(node) => node.start,
            Node::SnippetBlock(node) => node.start,
            Node::RenderTag(node) => node.start,
            Node::HtmlTag(node) => node.start,
            Node::ConstTag(node) => node.start,
            Node::DebugTag(node) => node.start,
            Node::ExpressionTag(node) => node.start,
            Node::Comment(node) => node.start,
            Node::RegularElement(node) => node.start,
            Node::Component(node) => node.start,
            Node::SlotElement(node) => node.start,
            Node::SvelteHead(node) => node.start,
            Node::SvelteBody(node) => node.start,
            Node::SvelteWindow(node) => node.start,
            Node::SvelteDocument(node) => node.start,
            Node::SvelteComponent(node) => node.start,
            Node::SvelteElement(node) => node.start,
            Node::SvelteSelf(node) => node.start,
            Node::SvelteFragment(node) => node.start,
            Node::SvelteBoundary(node) => node.start,
            Node::TitleElement(node) => node.start,
        }
    }

    fn end(&self) -> usize {
        match self {
            Node::Text(node) => node.end,
            Node::IfBlock(node) => node.end,
            Node::EachBlock(node) => node.end,
            Node::KeyBlock(node) => node.end,
            Node::AwaitBlock(node) => node.end,
            Node::SnippetBlock(node) => node.end,
            Node::RenderTag(node) => node.end,
            Node::HtmlTag(node) => node.end,
            Node::ConstTag(node) => node.end,
            Node::DebugTag(node) => node.end,
            Node::ExpressionTag(node) => node.end,
            Node::Comment(node) => node.end,
            Node::RegularElement(node) => node.end,
            Node::Component(node) => node.end,
            Node::SlotElement(node) => node.end,
            Node::SvelteHead(node) => node.end,
            Node::SvelteBody(node) => node.end,
            Node::SvelteWindow(node) => node.end,
            Node::SvelteDocument(node) => node.end,
            Node::SvelteComponent(node) => node.end,
            Node::SvelteElement(node) => node.end,
            Node::SvelteSelf(node) => node.end,
            Node::SvelteFragment(node) => node.end,
            Node::SvelteBoundary(node) => node.end,
            Node::TitleElement(node) => node.end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_estree_span_offsets_adds_offset() {
        let json = r#"{"type":"Identifier","name":"foo","start":0,"end":3}"#;
        let adjusted = adjust_estree_span_offsets(json, 10);
        assert_eq!(
            adjusted,
            r#"{"type":"Identifier","name":"foo","start":10,"end":13}"#
        );
    }

    #[test]
    fn adjust_estree_span_offsets_handles_nested() {
        let json = r#"{"type":"BinaryExpression","left":{"type":"Identifier","name":"a","start":0,"end":1},"right":{"type":"NumericLiteral","value":1,"raw":"1","start":4,"end":5},"start":0,"end":5}"#;
        let adjusted = adjust_estree_span_offsets(json, 20);
        assert!(adjusted.contains(r#""start":20"#));
        assert!(adjusted.contains(r#""end":21"#));
        assert!(adjusted.contains(r#""start":24"#));
        assert!(adjusted.contains(r#""end":25"#));
    }

    #[test]
    fn adjust_estree_span_offsets_preserves_string_values() {
        // "name":"start" should NOT be adjusted
        let json = r#"{"type":"Identifier","name":"start","start":0,"end":5}"#;
        let adjusted = adjust_estree_span_offsets(json, 10);
        assert_eq!(
            adjusted,
            r#"{"type":"Identifier","name":"start","start":10,"end":15}"#
        );
    }

    #[test]
    fn expression_serializes_oxc_identifier_with_offset() {
        let parsed = crate::js::JsExpression::parse(
            "foo",
            oxc_span::SourceType::mjs(),
        )
        .expect("valid expression");

        let expr = Expression::from_expression(Arc::new(parsed), 42, 45);
        let json = serde_json::to_string(&expr).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        assert_eq!(value["type"], "Identifier");
        assert_eq!(value["name"], "foo");
        assert_eq!(value["start"], 42);
        assert_eq!(value["end"], 45);
    }

    #[test]
    fn expression_serializes_fallback_without_node() {
        let expr = Expression::empty(10, 20);
        let json = serde_json::to_string(&expr).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        assert_eq!(value["type"], "Identifier");
        assert_eq!(value["name"], "");
        assert_eq!(value["start"], 10);
        assert_eq!(value["end"], 20);
    }

    #[test]
    fn ts_serializer_emits_type_annotation_on_arrow_params() {
        let parsed = crate::js::JsExpression::parse(
            "(e: MouseEvent) => e",
            oxc_span::SourceType::ts().with_module(true),
        )
        .expect("valid expression");

        let expr = Expression::from_expression(Arc::new(parsed), 0, 20);
        let raw = expr.serialize_oxc_node().expect("should serialize");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
        assert_eq!(value["type"], "ArrowFunctionExpression");
        let param = &value["params"][0];
        assert!(param.get("typeAnnotation").is_some(), "param should have typeAnnotation");
    }
}
