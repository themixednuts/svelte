use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ast::common::Span;
pub use crate::ast::common::{
    EstreeNode, FragmentType, LiteralValue, Loc as ExpressionLoc, NameLocation,
    Position as ExpressionPoint, RootCommentType, ScriptContext, ScriptType, SnippetHeaderError,
    SnippetHeaderErrorKind,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub r#type: ScriptType,
    pub start: usize,
    pub end: usize,
    pub context: ScriptContext,
    pub content: EstreeNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    pub r#type: FragmentType,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub children: Box<[Node]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramComment {
    pub r#type: RootCommentType,
    pub value: Arc<str>,
    pub start: usize,
    pub end: usize,
    pub loc: ExpressionLoc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<ElementTag>,
    pub attributes: Box<[Attribute]>,
    pub children: Box<[Node]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub attributes: Box<[Attribute]>,
    pub children: Box<[Node]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineComponent {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
    pub attributes: Box<[Attribute]>,
    pub children: Box<[Node]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElementTag {
    String(Arc<str>),
    Expression(Expression),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Attribute {
    Attribute(NamedAttribute),
    Spread(SpreadAttribute),
    Transition(TransitionDirective),
    StyleDirective(StyleDirective),
    Let(DirectiveAttribute),
    Action(DirectiveAttribute),
    Binding(DirectiveAttribute),
    Class(DirectiveAttribute),
    Animation(DirectiveAttribute),
    EventHandler(DirectiveAttribute),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedAttribute {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub value: AttributeValueList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadAttribute {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDirective {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub modifiers: Box<[Arc<str>]>,
    pub value: AttributeValueList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectiveAttribute {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub expression: Option<Expression>,
    pub modifiers: Box<[Arc<str>]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDirective {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub name_loc: NameLocation,
    pub expression: Option<Expression>,
    pub modifiers: Box<[Arc<str>]>,
    pub intro: bool,
    pub outro: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AttributeValue {
    Text(Text),
    MustacheTag(MustacheTag),
    AttributeShorthand(AttributeShorthand),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValueList {
    Boolean(bool),
    Values(Box<[AttributeValue]>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MustacheTag {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawMustacheTag {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugTag {
    pub start: usize,
    pub end: usize,
    pub arguments: Box<[Expression]>,
    pub identifiers: Box<[IdentifierExpression]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeShorthand {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Expression {
    Identifier(IdentifierExpression),
    Literal(LiteralExpression),
    CallExpression(CallExpression),
    BinaryExpression(BinaryExpression),
    ArrowFunctionExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    AssignmentExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    UnaryExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    MemberExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    LogicalExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    ConditionalExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    ArrayPattern {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    ObjectPattern {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    RestElement {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    ArrayExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    ObjectExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    Property {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    FunctionExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    TemplateLiteral {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    TaggedTemplateExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    SequenceExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    UpdateExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    ThisExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
    NewExpression {
        #[serde(flatten)]
        fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifierExpression {
    pub name: Arc<str>,
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc: Option<ExpressionLoc>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiteralExpression {
    pub start: usize,
    pub end: usize,
    pub loc: Option<ExpressionLoc>,
    pub value: LiteralValue,
    pub raw: Arc<str>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallExpression {
    pub start: usize,
    pub end: usize,
    pub loc: Option<ExpressionLoc>,
    pub callee: Box<Expression>,
    pub arguments: Box<[Expression]>,
    pub optional: bool,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryExpression {
    pub start: usize,
    pub end: usize,
    pub loc: Option<ExpressionLoc>,
    pub left: Box<Expression>,
    pub operator: Arc<str>,
    pub right: Box<Expression>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, crate::ast::modern::EstreeValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub start: usize,
    pub end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Arc<str>>,
    pub data: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub start: usize,
    pub end: usize,
    pub data: Arc<str>,
    pub ignores: Box<[Arc<str>]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IfBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    pub children: Box<[Node]>,
    #[serde(rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_block: Option<ElseBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elseif: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EachBlock {
    pub start: usize,
    pub end: usize,
    pub children: Box<[Node]>,
    pub context: Option<Expression>,
    pub expression: Expression,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<Expression>,
    #[serde(rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_block: Option<ElseBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    pub children: Box<[Node]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    pub value: Option<Expression>,
    pub error: Option<Expression>,
    pub pending: PendingBlock,
    pub then: ThenBlock,
    pub catch: CatchBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetBlock {
    pub start: usize,
    pub end: usize,
    pub expression: Expression,
    #[serde(rename = "typeParams", skip_serializing_if = "Option::is_none")]
    pub type_params: Option<Arc<str>>,
    pub parameters: Box<[Expression]>,
    pub children: Box<[Node]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_error: Option<SnippetHeaderError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingBlock {
    pub r#type: PendingBlockType,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub children: Box<[Node]>,
    pub skip: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThenBlock {
    pub r#type: ThenBlockType,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub children: Box<[Node]>,
    pub skip: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatchBlock {
    pub r#type: CatchBlockType,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub children: Box<[Node]>,
    pub skip: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingBlockType {
    PendingBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThenBlockType {
    ThenBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatchBlockType {
    CatchBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub r#type: StyleType,
    pub start: usize,
    pub end: usize,
    pub attributes: Box<[crate::ast::modern::Attribute]>,
    pub children: Box<[StyleNode]>,
    pub content: crate::ast::modern::CssContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StyleType {
    Style,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StyleNode {
    Rule(StyleRule),
    Atrule(StyleAtrule),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleRule {
    pub prelude: StyleSelectorList,
    pub block: crate::ast::modern::CssBlock,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleAtrule {
    pub start: usize,
    pub end: usize,
    pub name: Arc<str>,
    pub prelude: Arc<str>,
    pub block: Option<crate::ast::modern::CssBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSelectorList {
    pub r#type: crate::ast::modern::CssSelectorListType,
    pub start: usize,
    pub end: usize,
    pub children: Box<[StyleSelector]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSelector {
    pub r#type: StyleSelectorType,
    pub start: usize,
    pub end: usize,
    pub children: Box<[crate::ast::modern::CssSimpleSelector]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StyleSelectorType {
    Selector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElseBlock {
    pub r#type: ElseBlockType,
    pub start: usize,
    pub end: usize,
    pub children: Box<[Node]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElseBlockType {
    ElseBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Node {
    Element(Element),
    Head(Head),
    InlineComponent(InlineComponent),
    Text(Text),
    MustacheTag(MustacheTag),
    RawMustacheTag(RawMustacheTag),
    DebugTag(DebugTag),
    Comment(Comment),
    IfBlock(IfBlock),
    EachBlock(EachBlock),
    KeyBlock(KeyBlock),
    AwaitBlock(AwaitBlock),
    SnippetBlock(SnippetBlock),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub html: Fragment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub css: Option<Style>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<Script>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<Script>,
    #[serde(rename = "_comments", skip_serializing_if = "Option::is_none")]
    pub comments: Option<Box<[ProgramComment]>>,
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
    ProgramComment,
    Element,
    Head,
    InlineComponent,
    NamedAttribute,
    SpreadAttribute,
    StyleDirective,
    DirectiveAttribute,
    TransitionDirective,
    MustacheTag,
    RawMustacheTag,
    DebugTag,
    AttributeShorthand,
    IdentifierExpression,
    LiteralExpression,
    CallExpression,
    BinaryExpression,
    Text,
    Comment,
    IfBlock,
    EachBlock,
    KeyBlock,
    AwaitBlock,
    SnippetBlock,
    Style,
    StyleRule,
    StyleAtrule,
    StyleSelectorList,
    StyleSelector,
    ElseBlock
);

impl Span for Node {
    fn start(&self) -> usize {
        match self {
            Node::Element(node) => node.start,
            Node::Head(node) => node.start,
            Node::InlineComponent(node) => node.start,
            Node::Text(node) => node.start,
            Node::MustacheTag(node) => node.start,
            Node::RawMustacheTag(node) => node.start,
            Node::DebugTag(node) => node.start,
            Node::Comment(node) => node.start,
            Node::IfBlock(node) => node.start,
            Node::EachBlock(node) => node.start,
            Node::KeyBlock(node) => node.start,
            Node::AwaitBlock(node) => node.start,
            Node::SnippetBlock(node) => node.start,
        }
    }

    fn end(&self) -> usize {
        match self {
            Node::Element(node) => node.end,
            Node::Head(node) => node.end,
            Node::InlineComponent(node) => node.end,
            Node::Text(node) => node.end,
            Node::MustacheTag(node) => node.end,
            Node::RawMustacheTag(node) => node.end,
            Node::DebugTag(node) => node.end,
            Node::Comment(node) => node.end,
            Node::IfBlock(node) => node.end,
            Node::EachBlock(node) => node.end,
            Node::KeyBlock(node) => node.end,
            Node::AwaitBlock(node) => node.end,
            Node::SnippetBlock(node) => node.end,
        }
    }
}
