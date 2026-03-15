use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::ast::common::Span;
use crate::ast::{CssAst, Document};
use crate::error::SourcePosition;
use crate::{CompileError, LineColumn};
use camino::Utf8Path;
use camino::Utf8PathBuf;
use lightningcss::stylesheet::ParserOptions as LightningParserOptions;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod runes_mode;
pub(crate) mod scan;
pub(crate) mod validation;
pub(crate) use runes_mode::*;
pub(crate) use scan::*;
pub(crate) use svelte_syntax::{
    ElementKind, SvelteElementKind, classify_element_name, is_custom_element_name,
    is_valid_component_name, is_valid_element_name, is_void_element_name,
};

/// Current Svelte compiler version string.
pub static VERSION: &str = "5.53.9";

macro_rules! impl_enum_text_traits {
    ($ty:ty { $($text:literal => $variant:path),+ $(,)? }) => {
        impl $ty {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $($variant => $text),+
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $ty {
            type Err = ();

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($text => Ok($variant),)+
                    _ => Err(()),
                }
            }
        }
    };
}

#[derive(Clone, Copy)]
/// Input passed to a custom CSS hash callback.
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
/// Callback used to filter warnings during compilation.
pub struct WarningFilterCallback(Arc<dyn Fn(&Warning) -> bool + 'static>);

impl WarningFilterCallback {
    #[must_use]
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
/// Callback used to override Svelte's generated CSS hash.
pub struct CssHashGetterCallback(Arc<dyn for<'a> Fn(CssHashInput<'a>) -> Arc<str> + 'static>);

impl CssHashGetterCallback {
    #[must_use]
    pub fn new<F, S>(callback: F) -> Self
    where
        F: for<'a> Fn(CssHashInput<'a>) -> S + 'static,
        S: Into<Arc<str>>,
    {
        Self(Arc::new(move |input| callback(input).into()))
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
/// Selects which public AST representation `parse` returns.
pub enum ParseMode {
    #[default]
    Legacy,
    Modern,
}

impl_enum_text_traits!(ParseMode {
    "legacy" => ParseMode::Legacy,
    "modern" => ParseMode::Modern,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
/// Options for parsing a component without compiling it.
pub struct ParseOptions {
    /// Optional source filename used in diagnostics.
    pub filename: Option<Utf8PathBuf>,
    /// Optional project root used by path-sensitive tooling.
    pub root_dir: Option<Utf8PathBuf>,
    /// Compatibility flag matching Svelte's JavaScript API.
    pub modern: Option<bool>,
    /// Preferred AST shape when `modern` is not set.
    pub mode: ParseMode,
    /// Return a best-effort AST for malformed input when possible.
    pub loose: bool,
}

impl ParseOptions {
    #[must_use]
    pub fn effective_mode(&self) -> ParseMode {
        match self.modern {
            Some(true) => ParseMode::Modern,
            Some(false) => ParseMode::Legacy,
            None => self.mode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
/// Options for converting a parsed AST back into Svelte source.
pub struct PrintOptions {
    /// Keep whitespace-only text nodes instead of collapsing them where possible.
    pub preserve_whitespace: bool,
}

#[derive(Clone, Copy, Debug)]
/// Selects which modern AST node should be printed.
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

impl_enum_text_traits!(Namespace {
    "html" => Namespace::Html,
    "svg" => Namespace::Svg,
    "mathml" => Namespace::Mathml,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CssOutputMode {
    Injected,
    #[default]
    External,
}

impl_enum_text_traits!(CssOutputMode {
    "injected" => CssOutputMode::Injected,
    "external" => CssOutputMode::External,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompatibilityComponentApi {
    V4,
    #[default]
    V5,
}

impl_enum_text_traits!(CompatibilityComponentApi {
    "4" => CompatibilityComponentApi::V4,
    "5" => CompatibilityComponentApi::V5,
});

impl Serialize for CompatibilityComponentApi {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(match self {
            Self::V4 => 4,
            Self::V5 => 5,
        })
    }
}

impl<'de> Deserialize<'de> for CompatibilityComponentApi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CompatibilityComponentApiRepr {
            Number(u8),
            String(Arc<str>),
        }

        match CompatibilityComponentApiRepr::deserialize(deserializer)? {
            CompatibilityComponentApiRepr::Number(4) => Ok(Self::V4),
            CompatibilityComponentApiRepr::Number(5) => Ok(Self::V5),
            CompatibilityComponentApiRepr::Number(other) => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(u64::from(other)),
                &"4 or 5",
            )),
            CompatibilityComponentApiRepr::String(value) => value.parse().map_err(|_| {
                serde::de::Error::invalid_value(
                    serde::de::Unexpected::Str(value.as_ref()),
                    &"\"4\" or \"5\"",
                )
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompatibilityOptions {
    #[serde(alias = "componentApi")]
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

impl_enum_text_traits!(GenerateTarget {
    "none" => GenerateTarget::None,
    "client" => GenerateTarget::Client,
    "server" => GenerateTarget::Server,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FragmentStrategy {
    #[default]
    Html,
    Tree,
}

impl_enum_text_traits!(FragmentStrategy {
    "html" => FragmentStrategy::Html,
    "tree" => FragmentStrategy::Tree,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ErrorMode {
    #[default]
    Error,
    Warn,
}

impl_enum_text_traits!(ErrorMode {
    "error" => ErrorMode::Error,
    "warn" => ErrorMode::Warn,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
/// Experimental compiler switches.
pub struct ExperimentalOptions {
    pub r#async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Sourcemap configuration accepted by the compiler.
pub struct SourceMap {
    pub version: u32,
    pub file: Option<Arc<str>>,
    #[serde(alias = "sourceRoot")]
    pub source_root: Option<Arc<str>>,
    pub sources: Box<[Arc<str>]>,
    #[serde(alias = "sourcesContent")]
    pub sources_content: Option<Box<[Option<Arc<str>>]>>,
    pub names: Box<[Arc<str>]>,
    pub mappings: Arc<str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
/// Options for compiling a `.svelte` component or rune-enabled module.
pub struct CompileOptions {
    pub name: Option<Arc<str>>,
    pub filename: Option<Utf8PathBuf>,
    #[serde(alias = "rootDir")]
    pub root_dir: Option<Utf8PathBuf>,
    pub generate: GenerateTarget,
    pub fragments: FragmentStrategy,
    pub dev: bool,
    pub hmr: bool,
    #[serde(alias = "customElement")]
    pub custom_element: bool,
    pub accessors: bool,
    pub namespace: Namespace,
    pub immutable: bool,
    pub css: CssOutputMode,
    #[serde(alias = "warningFilterIgnoreCodes")]
    pub warning_filter_ignore_codes: Box<[Arc<str>]>,
    #[serde(skip, default)]
    pub warning_filter: Option<WarningFilterCallback>,
    pub runes: Option<bool>,
    #[serde(alias = "errorMode")]
    pub error_mode: ErrorMode,
    pub sourcemap: Option<SourceMap>,
    #[serde(alias = "outputFilename")]
    pub output_filename: Option<Utf8PathBuf>,
    #[serde(alias = "cssOutputFilename")]
    pub css_output_filename: Option<Utf8PathBuf>,
    #[serde(alias = "cssHash")]
    pub css_hash: Option<Arc<str>>,
    #[serde(skip, default)]
    pub css_hash_getter: Option<CssHashGetterCallback>,
    #[serde(alias = "preserveComments")]
    pub preserve_comments: bool,
    #[serde(alias = "preserveWhitespace")]
    pub preserve_whitespace: bool,
    #[serde(alias = "discloseVersion")]
    pub disclose_version: bool,
    pub compatibility: Option<CompatibilityOptions>,
    #[serde(alias = "modernAst")]
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
/// Warning emitted during analysis or code generation.
pub struct Warning {
    pub code: Arc<str>,
    pub message: Arc<str>,
    pub filename: Option<Utf8PathBuf>,
    pub start: Option<LineColumn>,
    pub end: Option<LineColumn>,
    pub frame: Option<Arc<str>>,
    pub position: Option<[usize; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Generated output artifact such as JavaScript or CSS.
pub struct OutputArtifact {
    pub code: Arc<str>,
    pub map: Option<SourceMap>,
    pub has_global: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Extra metadata produced during compilation.
pub struct CompileMetadata {
    pub runes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Result of compiling a component or module.
pub struct CompileResult {
    pub js: OutputArtifact,
    pub css: Option<OutputArtifact>,
    pub warnings: Box<[Warning]>,
    pub metadata: CompileMetadata,
    pub ast: Option<Document>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Result of printing a Svelte AST node.
pub struct PrintedOutput {
    pub code: Arc<str>,
    pub map: SourceMap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// Attribute values passed to preprocessors.
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
/// One parsed attribute passed to a tag preprocessor.
pub struct PreprocessAttribute {
    pub name: Arc<str>,
    pub value: PreprocessAttributeValue,
}

pub type PreprocessAttributes = BTreeMap<Arc<str>, PreprocessAttributeValue>;

#[derive(Debug, Clone, Copy)]
/// Input passed to a markup preprocessor.
pub struct PreprocessMarkup<'a> {
    pub content: &'a str,
    pub filename: Option<&'a Utf8Path>,
}

#[derive(Debug, Clone, Copy)]
/// Input passed to a script or style preprocessor.
pub struct PreprocessTag<'a> {
    pub content: &'a str,
    pub attributes: &'a PreprocessAttributes,
    pub markup: &'a str,
    pub filename: Option<&'a Utf8Path>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Output returned by one preprocessor step.
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
/// Collection of preprocessors applied in source order.
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
/// Options for running preprocessors over component source code.
pub struct PreprocessOptions {
    pub filename: Option<Utf8PathBuf>,
    #[serde(skip, default)]
    pub groups: Box<[PreprocessorGroup]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Final result of a preprocessing run.
pub struct PreprocessResult {
    pub code: Arc<str>,
    pub dependencies: Box<[Utf8PathBuf]>,
    pub map: Option<SourceMap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
/// Options for best-effort code migration.
pub struct MigrateOptions {
    pub filename: Option<Utf8PathBuf>,
    pub use_ts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Result of a migration run.
pub struct MigrateResult {
    pub code: Arc<str>,
}

#[derive(Debug, Default)]
/// Convenience object that mirrors the free compiler functions.
pub struct Compiler;

impl Compiler {
    #[must_use]
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
        crate::compiler::phases::preprocess::preprocess(source, options)
    }

    pub async fn preprocess_async(
        &self,
        source: &str,
        options: PreprocessOptions,
    ) -> Result<PreprocessResult, CompileError> {
        crate::compiler::phases::preprocess::preprocess_async(source, options).await
    }

    pub fn migrate(
        &self,
        source: &str,
        options: MigrateOptions,
    ) -> Result<MigrateResult, CompileError> {
        crate::compiler::phases::migrate::migrate(source, options)
    }
}
