use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::SourceLocation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptType {
    Script,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptContext {
    Default,
    Module,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameLocation {
    pub start: SourceLocation,
    pub end: SourceLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DirectiveValueSyntax {
    #[default]
    Implicit,
    Expression,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AttributeValueSyntax {
    #[default]
    Boolean,
    Expression,
    Quoted,
    Unquoted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrErrorKind {
    InvalidName,
    ExpectedEquals,
    ExpectedValue,
    HtmlTag,
    Block(Arc<str>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttrError {
    pub kind: AttrErrorKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnippetHeaderErrorKind {
    ExpectedRightBrace,
    ExpectedRightParen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetHeaderError {
    pub kind: SnippetHeaderErrorKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParseErrorKind {
    BlockInvalidContinuationPlacement,
    ExpectedTokenElse,
    ExpectedTokenAwaitBranch,
    ExpectedTokenCommentClose,
    ExpectedTokenStyleClose,
    ExpectedTokenRightBrace,
    ExpectedWhitespace,
    BlockUnexpectedCharacter,
    UnexpectedReservedWord { word: Arc<str> },
    JsParseError { message: Arc<str> },
    CssExpectedIdentifier,
    UnexpectedEof,
    BlockUnclosed,
    ElementUnclosed { name: Arc<str> },
    ElementInvalidClosingTag { name: Arc<str> },
    ElementInvalidClosingTagAutoclosed { name: Arc<str>, reason: Arc<str> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub start: usize,
    pub end: usize,
}

/// A JavaScript comment extracted from OXC parsing, used for the public AST.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsComment {
    pub kind: JsCommentKind,
    pub value: Arc<str>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsCommentKind {
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootCommentType {
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FragmentType {
    Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LiteralValue {
    String(Arc<str>),
    Number(i64),
    JsonNumber(serde_json::Number),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loc {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<usize>,
}

pub trait Span {
    fn start(&self) -> usize;
    fn end(&self) -> usize;

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.end().saturating_sub(self.start())
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.start() >= self.end()
    }

    #[allow(dead_code)]
    fn contains(&self, offset: usize) -> bool {
        offset >= self.start() && offset <= self.end()
    }
}
