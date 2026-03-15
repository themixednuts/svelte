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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub r#type: ScriptType,
    pub start: usize,
    pub end: usize,
    /// Byte range of the script content (between open tag `>` and `</script>`).
    #[serde(skip_serializing, default)]
    pub content_start: usize,
    #[serde(skip_serializing, default)]
    pub content_end: usize,
    pub context: ScriptContext,
    #[serde(
        skip_serializing,
        skip_deserializing,
        default = "empty_parsed_js_program"
    )]
    pub content: Arc<JsProgram>,
    pub attributes: Box<[Attribute]>,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Expression {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing, default)]
    pub syntax: ExpressionSyntax,
    #[serde(skip_serializing, skip_deserializing, default)]
    pub node: Option<JsNodeHandle>,
}

impl Expression {
    pub fn empty(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: None,
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
        }
    }

    pub fn from_pattern(parsed: Arc<JsPattern>, start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::Pattern(parsed)),
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
        }
    }

    pub fn from_rest_parameter(parameters: Arc<JsParameters>, start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            syntax: Default::default(),
            node: Some(JsNodeHandle::RestParameter(parameters)),
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
        if let Some(raw_json) = self.to_estree_json() {
            // Embed pre-serialized JSON directly via RawValue — no re-parsing.
            let raw = serde_json::value::RawValue::from_string(raw_json)
                .map_err(serde::ser::Error::custom)?;
            raw.serialize(serializer)
        } else {
            // Fallback: just emit start/end when no OXC node is available
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("start", &self.start)?;
            map.serialize_entry("end", &self.end)?;
            map.end()
        }
    }
}

impl Expression {
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
        use oxc_estree::{CompactJSSerializer, ESTree};

        match &self.node {
            Some(JsNodeHandle::Expression(parsed)) => {
                let mut ser = CompactJSSerializer::new(false);
                serialize_oxc_expression_unwrapped(parsed.expression(), &mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::SequenceItem { root, index }) => {
                let OxcExpression::SequenceExpression(seq) = root.expression() else {
                    return None;
                };
                let expr = seq.expressions.get(*index)?;
                let mut ser = CompactJSSerializer::new(false);
                expr.serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::Pattern(parsed)) => {
                let mut ser = CompactJSSerializer::new(false);
                parsed.pattern().serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::ParameterItem { parameters, index }) => {
                let param = parameters.parameter(*index)?;
                let mut ser = CompactJSSerializer::new(false);
                param.serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::RestParameter(parameters)) => {
                let rest = parameters.rest_parameter()?;
                let mut ser = CompactJSSerializer::new(false);
                rest.serialize(&mut ser);
                Some(ser.into_string())
            }
            Some(JsNodeHandle::StatementInProgram { program, index }) => {
                let stmt = program.statement(*index)?;
                let mut ser = CompactJSSerializer::new(false);
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

/// Serialize an OXC expression, unwrapping ParenthesizedExpression nodes.
/// Svelte tracks parens via `Expression.syntax.parens`, not in the AST.
fn serialize_oxc_expression_unwrapped(
    expr: &OxcExpression<'_>,
    ser: &mut oxc_estree::CompactJSSerializer,
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

        assert_eq!(value["start"], 10);
        assert_eq!(value["end"], 20);
        assert!(value.get("type").is_none());
    }
}
