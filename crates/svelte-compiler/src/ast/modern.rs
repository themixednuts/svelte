use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ast::common::Span;
pub use crate::ast::common::{
    AttrError, AttrErrorKind, AttributeValueSyntax, DirectiveValueSyntax, EstreeNode, EstreeValue,
    FragmentType, LiteralValue, Loc, NameLocation, ParseError, Position, RootCommentType,
    ScriptContext, ScriptType, SnippetHeaderError, SnippetHeaderErrorKind,
};

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
    pub content: EstreeNode,
    pub attributes: Box<[Attribute]>,
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
    pub name_loc: NameLocation,
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
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotElement {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteHead {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteBody {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteWindow {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteDocument {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteComponent {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
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
    pub name_loc: NameLocation,
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
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteFragment {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SvelteBoundary {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub attributes: Box<[Attribute]>,
    pub fragment: Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleElement {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
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
    pub name_loc: NameLocation,
    pub value: AttributeValueList,
    #[serde(skip_serializing, default)]
    pub value_syntax: AttributeValueSyntax,
    #[serde(skip_serializing, default)]
    pub error: Option<AttrError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValueList {
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
    pub name_loc: NameLocation,
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
    pub name_loc: NameLocation,
    pub modifiers: Box<[Arc<str>]>,
    pub value: AttributeValueList,
    #[serde(skip_serializing, default)]
    pub value_syntax: AttributeValueSyntax,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDirective {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expression(
    pub EstreeNode,
    #[serde(skip_serializing, default)] pub ExpressionSyntax,
);

impl Expression {
    pub fn parens(&self) -> u16 {
        self.1.parens
    }

    pub fn is_parenthesized(&self) -> bool {
        self.parens() != 0
    }
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

pub trait Element: Span {
    fn name(&self) -> &str;
    fn name_loc(&self) -> &NameLocation;
    fn attributes(&self) -> &[Attribute];
    fn fragment(&self) -> &Fragment;
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
            impl Element for $ty {
                fn name(&self) -> &str { &self.name }
                fn name_loc(&self) -> &NameLocation { &self.name_loc }
                fn attributes(&self) -> &[Attribute] { &self.attributes }
                fn fragment(&self) -> &Fragment { &self.fragment }
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
    fn name_loc(&self) -> &NameLocation {
        &self.name_loc
    }
    fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
    fn self_closing(&self) -> bool {
        self.self_closing
    }
}

impl Element for SvelteComponent {
    fn name(&self) -> &str {
        &self.name
    }
    fn name_loc(&self) -> &NameLocation {
        &self.name_loc
    }
    fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
    fn expression(&self) -> Option<&Expression> {
        self.expression.as_ref()
    }
}

impl Element for SvelteElement {
    fn name(&self) -> &str {
        &self.name
    }
    fn name_loc(&self) -> &NameLocation {
        &self.name_loc
    }
    fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    fn fragment(&self) -> &Fragment {
        &self.fragment
    }
    fn expression(&self) -> Option<&Expression> {
        self.expression.as_ref()
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootComment {
    pub r#type: RootCommentType,
    pub start: usize,
    pub end: usize,
    pub value: Arc<str>,
    pub loc: NameLocation,
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
