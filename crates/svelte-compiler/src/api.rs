use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use html_escape::decode_html_entities as decode_html_entities_cow;
use lightningcss::stylesheet::ParserOptions as LightningParserOptions;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Point};

use crate::ast::common::Span;
use crate::ast::modern::{EstreeNode, RootCommentType};
use crate::ast::{CssAst, Document};
use crate::error::SourcePosition;
use crate::{CompileError, SourceLocation};

mod elements;
mod legacy;
pub(crate) mod modern;
mod runes_mode;
pub(crate) mod scan;
pub(crate) mod validation;
pub(crate) use elements::*;
pub(crate) use legacy::parse_root as parse_legacy_root_from_cst;
pub(crate) use legacy::{
    find_first_named_child, legacy_expression_from_raw_node, parse_identifier_name,
    parse_modern_attributes, source_location_from_point, text_for_node,
};
pub(crate) use modern::parse_root as parse_root_from_cst;
pub(crate) use modern::{
    attach_estree_comments_to_tree, attach_leading_comments_to_expression,
    attach_trailing_comments_to_expression, estree_value_to_usize, expression_identifier_name,
    find_matching_brace_close, legacy_expression_from_modern_expression, line_column_at_offset,
    modern_empty_identifier_expression, modern_node_end, modern_node_span, modern_node_start,
    named_children_vec, normalize_estree_node, normalize_pattern_template_elements,
    parse_all_comment_nodes, parse_leading_comment_nodes, parse_modern_expression_from_text,
    parse_modern_expression_tag, position_raw_node,
};
pub(crate) use runes_mode::*;
pub(crate) use scan::*;

pub static VERSION: &str = "5.53.9";

#[derive(Clone, Copy)]
pub struct CssHashInput<'a> {
    pub name: &'a str,
    pub filename: &'a str,
    pub css: &'a str,
    pub hash: fn(&str) -> String,
}

impl fmt::Debug for CssHashInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CssHashInput")
            .field("name", &self.name)
            .field("filename", &self.filename)
            .field("css", &self.css)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct WarningFilterCallback(Arc<dyn Fn(&Warning) -> bool + 'static>);

impl WarningFilterCallback {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(&Warning) -> bool + 'static,
    {
        Self(Arc::new(callback))
    }

    pub(crate) fn call(&self, warning: &Warning) -> bool {
        (self.0)(warning)
    }
}

impl fmt::Debug for WarningFilterCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WarningFilterCallback(..)")
    }
}

#[derive(Clone)]
pub struct CssHashGetterCallback(Arc<dyn for<'a> Fn(CssHashInput<'a>) -> Arc<str> + 'static>);

impl CssHashGetterCallback {
    pub fn new<F>(callback: F) -> Self
    where
        F: for<'a> Fn(CssHashInput<'a>) -> Arc<str> + 'static,
    {
        Self(Arc::new(callback))
    }

    pub(crate) fn call(&self, input: CssHashInput<'_>) -> Arc<str> {
        (self.0)(input)
    }
}

impl fmt::Debug for CssHashGetterCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CssHashGetterCallback(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ParseMode {
    #[default]
    Legacy,
    Modern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ParseOptions {
    pub filename: Option<Utf8PathBuf>,
    pub root_dir: Option<Utf8PathBuf>,
    pub modern: Option<bool>,
    pub mode: ParseMode,
    pub loose: bool,
}

impl ParseOptions {
    pub fn effective_mode(&self) -> ParseMode {
        match self.modern {
            Some(true) => ParseMode::Modern,
            Some(false) => ParseMode::Legacy,
            None => self.mode,
        }
    }
}

#[derive(Clone)]
pub struct PrintCommentGetterCallback(Arc<dyn Fn(&EstreeNode) -> Box<[EstreeNode]> + 'static>);

impl PrintCommentGetterCallback {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(&EstreeNode) -> Box<[EstreeNode]> + 'static,
    {
        Self(Arc::new(callback))
    }

    pub(crate) fn call(&self, node: &EstreeNode) -> Box<[EstreeNode]> {
        (self.0)(node)
    }
}

impl fmt::Debug for PrintCommentGetterCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrintCommentGetterCallback(..)")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PrintOptions {
    pub preserve_whitespace: bool,
    #[serde(skip, default)]
    pub get_leading_comments: Option<PrintCommentGetterCallback>,
    #[serde(skip, default)]
    pub get_trailing_comments: Option<PrintCommentGetterCallback>,
}

#[derive(Clone, Copy, Debug)]
pub enum ModernPrintTarget<'a> {
    Root {
        source: &'a str,
        root: &'a crate::ast::modern::Root,
    },
    Fragment {
        source: &'a str,
        fragment: &'a crate::ast::modern::Fragment,
    },
    Node {
        source: &'a str,
        node: &'a crate::ast::modern::Node,
    },
    Script {
        source: &'a str,
        script: &'a crate::ast::modern::Script,
    },
    Css {
        source: &'a str,
        stylesheet: &'a crate::ast::modern::Css,
    },
    CssNode {
        source: &'a str,
        node: &'a crate::ast::modern::CssNode,
    },
    Attribute {
        source: &'a str,
        attribute: &'a crate::ast::modern::Attribute,
    },
    Options {
        source: &'a str,
        options: &'a crate::ast::modern::Options,
    },
    Comment {
        source: &'a str,
        comment: &'a crate::ast::modern::Comment,
    },
}

impl<'a> ModernPrintTarget<'a> {
    pub const fn root(source: &'a str, root: &'a crate::ast::modern::Root) -> Self {
        Self::Root { source, root }
    }

    pub const fn fragment(source: &'a str, fragment: &'a crate::ast::modern::Fragment) -> Self {
        Self::Fragment { source, fragment }
    }

    pub const fn node(source: &'a str, node: &'a crate::ast::modern::Node) -> Self {
        Self::Node { source, node }
    }

    pub const fn script(source: &'a str, script: &'a crate::ast::modern::Script) -> Self {
        Self::Script { source, script }
    }

    pub const fn css(source: &'a str, stylesheet: &'a crate::ast::modern::Css) -> Self {
        Self::Css { source, stylesheet }
    }

    pub const fn css_node(source: &'a str, node: &'a crate::ast::modern::CssNode) -> Self {
        Self::CssNode { source, node }
    }

    pub const fn attribute(source: &'a str, attribute: &'a crate::ast::modern::Attribute) -> Self {
        Self::Attribute { source, attribute }
    }

    pub const fn options(source: &'a str, options: &'a crate::ast::modern::Options) -> Self {
        Self::Options { source, options }
    }

    pub const fn comment(source: &'a str, comment: &'a crate::ast::modern::Comment) -> Self {
        Self::Comment { source, comment }
    }

    pub(crate) const fn source(self) -> &'a str {
        match self {
            Self::Root { source, .. }
            | Self::Fragment { source, .. }
            | Self::Node { source, .. }
            | Self::Script { source, .. }
            | Self::Css { source, .. }
            | Self::CssNode { source, .. }
            | Self::Attribute { source, .. }
            | Self::Options { source, .. }
            | Self::Comment { source, .. } => source,
        }
    }

    pub(crate) fn raw_slice(self) -> Option<&'a str> {
        let source = self.source();
        let (start, end) = match self {
            Self::Root { root, .. } => (root.start, root.end),
            Self::Fragment { fragment, .. } => {
                let first = fragment.nodes.first()?;
                let last = fragment.nodes.last()?;
                (first.start(), last.end())
            }
            Self::Node { node, .. } => (node.start(), node.end()),
            Self::Script { script, .. } => (script.start, script.end),
            Self::Css { stylesheet, .. } => (stylesheet.start, stylesheet.end),
            Self::CssNode { node, .. } => match node {
                crate::ast::modern::CssNode::Rule(rule) => (rule.start, rule.end),
                crate::ast::modern::CssNode::Atrule(atrule) => (atrule.start, atrule.end),
            },
            Self::Attribute { attribute, .. } => match attribute {
                crate::ast::modern::Attribute::Attribute(attribute) => {
                    (attribute.start, attribute.end)
                }
                crate::ast::modern::Attribute::SpreadAttribute(attribute) => {
                    (attribute.start, attribute.end)
                }
                crate::ast::modern::Attribute::BindDirective(attribute)
                | crate::ast::modern::Attribute::OnDirective(attribute)
                | crate::ast::modern::Attribute::ClassDirective(attribute)
                | crate::ast::modern::Attribute::LetDirective(attribute)
                | crate::ast::modern::Attribute::AnimateDirective(attribute)
                | crate::ast::modern::Attribute::UseDirective(attribute) => {
                    (attribute.start, attribute.end)
                }
                crate::ast::modern::Attribute::StyleDirective(attribute) => {
                    (attribute.start, attribute.end)
                }
                crate::ast::modern::Attribute::TransitionDirective(attribute) => {
                    (attribute.start, attribute.end)
                }
                crate::ast::modern::Attribute::AttachTag(attribute) => {
                    (attribute.start, attribute.end)
                }
            },
            Self::Options { options, .. } => (options.start, options.end),
            Self::Comment { comment, .. } => (comment.start, comment.end),
        };

        source.get(start..end)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Namespace {
    #[default]
    Html,
    Svg,
    Mathml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CssOutputMode {
    Injected,
    #[default]
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompatibilityComponentApi {
    #[serde(rename = "4")]
    V4,
    #[default]
    #[serde(rename = "5")]
    V5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompatibilityOptions {
    pub component_api: Option<CompatibilityComponentApi>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GenerateTarget {
    None,
    #[default]
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FragmentStrategy {
    #[default]
    Html,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ErrorMode {
    #[default]
    Error,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExperimentalOptions {
    pub r#async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SourceMap {
    pub version: u32,
    pub file: Option<Arc<str>>,
    pub source_root: Option<Arc<str>>,
    pub sources: Box<[Arc<str>]>,
    pub sources_content: Option<Box<[Option<Arc<str>>]>>,
    pub names: Box<[Arc<str>]>,
    pub mappings: Arc<str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompileOptions {
    pub name: Option<Arc<str>>,
    pub filename: Option<Utf8PathBuf>,
    pub root_dir: Option<Utf8PathBuf>,
    pub generate: GenerateTarget,
    pub fragments: FragmentStrategy,
    pub dev: bool,
    pub hmr: bool,
    pub custom_element: bool,
    pub accessors: bool,
    pub namespace: Namespace,
    pub immutable: bool,
    pub css: CssOutputMode,
    pub warning_filter_ignore_codes: Box<[Arc<str>]>,
    #[serde(skip, default)]
    pub warning_filter: Option<WarningFilterCallback>,
    pub runes: Option<bool>,
    pub error_mode: ErrorMode,
    pub sourcemap: Option<SourceMap>,
    pub output_filename: Option<Utf8PathBuf>,
    pub css_output_filename: Option<Utf8PathBuf>,
    pub css_hash: Option<Arc<str>>,
    #[serde(skip, default)]
    pub css_hash_getter: Option<CssHashGetterCallback>,
    pub preserve_comments: bool,
    pub preserve_whitespace: bool,
    pub disclose_version: bool,
    pub compatibility: Option<CompatibilityOptions>,
    pub modern_ast: bool,
    pub experimental: ExperimentalOptions,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            name: None,
            filename: None,
            root_dir: None,
            generate: GenerateTarget::default(),
            fragments: FragmentStrategy::default(),
            dev: false,
            hmr: false,
            custom_element: false,
            accessors: false,
            namespace: Namespace::default(),
            immutable: false,
            css: CssOutputMode::default(),
            warning_filter_ignore_codes: Box::default(),
            warning_filter: None,
            runes: None,
            error_mode: ErrorMode::default(),
            sourcemap: None,
            output_filename: None,
            css_output_filename: None,
            css_hash: None,
            css_hash_getter: None,
            preserve_comments: false,
            preserve_whitespace: false,
            disclose_version: true,
            compatibility: None,
            modern_ast: false,
            experimental: ExperimentalOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Warning {
    pub code: Arc<str>,
    pub message: Arc<str>,
    pub filename: Option<Utf8PathBuf>,
    pub start: Option<SourceLocation>,
    pub end: Option<SourceLocation>,
    pub frame: Option<Arc<str>>,
    pub position: Option<[usize; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OutputArtifact {
    pub code: Arc<str>,
    pub map: Option<SourceMap>,
    pub has_global: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompileMetadata {
    pub runes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompileResult {
    pub js: OutputArtifact,
    pub css: Option<OutputArtifact>,
    pub warnings: Box<[Warning]>,
    pub metadata: CompileMetadata,
    pub ast: Option<Document>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PrintedOutput {
    pub code: Arc<str>,
    pub map: SourceMap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreprocessAttributeValue {
    String(Arc<str>),
    Bool(bool),
}

impl Default for PreprocessAttributeValue {
    fn default() -> Self {
        Self::Bool(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PreprocessAttribute {
    pub name: Arc<str>,
    pub value: PreprocessAttributeValue,
}

pub type PreprocessAttributes = BTreeMap<Arc<str>, PreprocessAttributeValue>;

#[derive(Debug, Clone, Copy)]
pub struct PreprocessMarkup<'a> {
    pub content: &'a str,
    pub filename: Option<&'a Utf8Path>,
}

#[derive(Debug, Clone, Copy)]
pub struct PreprocessTag<'a> {
    pub content: &'a str,
    pub attributes: &'a PreprocessAttributes,
    pub filename: Option<&'a Utf8Path>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PreprocessOutput {
    pub code: Arc<str>,
    pub dependencies: Box<[Utf8PathBuf]>,
    pub map: Option<SourceMap>,
    pub attributes: Option<Box<[PreprocessAttribute]>>,
}

pub type MarkupPreprocessor = Arc<
    dyn for<'a> Fn(PreprocessMarkup<'a>) -> Result<Option<PreprocessOutput>, CompileError>
        + Send
        + Sync
        + 'static,
>;

pub type TagPreprocessor = Arc<
    dyn for<'a> Fn(PreprocessTag<'a>) -> Result<Option<PreprocessOutput>, CompileError>
        + Send
        + Sync
        + 'static,
>;

pub type AsyncMarkupPreprocessor = Arc<
    dyn for<'a> Fn(
            PreprocessMarkup<'a>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Option<PreprocessOutput>, CompileError>> + Send + 'a>,
        > + Send
        + Sync
        + 'static,
>;

pub type AsyncTagPreprocessor = Arc<
    dyn for<'a> Fn(
            PreprocessTag<'a>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Option<PreprocessOutput>, CompileError>> + Send + 'a>,
        > + Send
        + Sync
        + 'static,
>;

#[derive(Clone, Default)]
pub struct PreprocessorGroup {
    pub name: Option<Arc<str>>,
    pub markup: Option<MarkupPreprocessor>,
    pub script: Option<TagPreprocessor>,
    pub style: Option<TagPreprocessor>,
    pub markup_async: Option<AsyncMarkupPreprocessor>,
    pub script_async: Option<AsyncTagPreprocessor>,
    pub style_async: Option<AsyncTagPreprocessor>,
}

impl fmt::Debug for PreprocessorGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreprocessorGroup")
            .field("name", &self.name)
            .field("markup", &self.markup.is_some())
            .field("script", &self.script.is_some())
            .field("style", &self.style.is_some())
            .field("markup_async", &self.markup_async.is_some())
            .field("script_async", &self.script_async.is_some())
            .field("style_async", &self.style_async.is_some())
            .finish()
    }
}

impl PreprocessorGroup {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PreprocessOptions {
    pub filename: Option<Utf8PathBuf>,
    #[serde(skip, default)]
    pub groups: Box<[PreprocessorGroup]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PreprocessResult {
    pub code: Arc<str>,
    pub dependencies: Box<[Utf8PathBuf]>,
    pub map: Option<SourceMap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MigrateOptions {
    pub filename: Option<Utf8PathBuf>,
    pub use_ts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MigrateResult {
    pub code: Arc<str>,
}

#[derive(Debug, Default)]
pub struct Compiler;

impl Compiler {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&self, source: &str, options: ParseOptions) -> Result<Document, CompileError> {
        crate::compiler::phases::parse::parse_component(source, options)
    }

    pub fn print(
        &self,
        ast: &Document,
        options: PrintOptions,
    ) -> Result<PrintedOutput, CompileError> {
        crate::compiler::phases::transform::print_component(ast, options)
    }

    pub fn print_modern(
        &self,
        ast: ModernPrintTarget<'_>,
        options: PrintOptions,
    ) -> Result<PrintedOutput, CompileError> {
        crate::compiler::phases::transform::print_modern_target(ast, options)
    }

    pub fn compile(
        &self,
        source: &str,
        options: CompileOptions,
    ) -> Result<CompileResult, CompileError> {
        crate::compiler::phases::transform::compile_component(source, options)
    }

    pub fn compile_module(
        &self,
        source: &str,
        options: CompileOptions,
    ) -> Result<CompileResult, CompileError> {
        crate::compiler::phases::transform::compile_module(source, options)
    }

    pub fn parse_css(&self, source: &str) -> Result<CssAst, CompileError> {
        crate::compiler::phases::parse::parse_css(source)
    }

    pub fn preprocess(
        &self,
        source: &str,
        options: PreprocessOptions,
    ) -> Result<PreprocessResult, CompileError> {
        crate::compiler::preprocess(source, options)
    }

    pub fn migrate(
        &self,
        source: &str,
        options: MigrateOptions,
    ) -> Result<MigrateResult, CompileError> {
        crate::compiler::migrate(source, options)
    }
}

#[cfg(test)]
mod debug_tests;
