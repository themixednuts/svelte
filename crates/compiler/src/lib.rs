//! Svelte compiler APIs for parsing, printing, preprocessing, migration, and
//! JavaScript/CSS code generation.
//!
//! # Examples
//!
//! Compile a component:
//!
//! ```
//! use svelte_compiler::{CompileOptions, compile};
//!
//! let result = compile(
//!     "<script>let name = 'world';</script><h1>Hello {name}</h1>",
//!     CompileOptions::default(),
//! )?;
//!
//! assert!(result.js.code.contains("Hello"));
//! # Ok::<(), svelte_compiler::CompileError>(())
//! ```
//!
//! Parse and print a modern AST:
//!
//! ```
//! use svelte_compiler::{ModernPrintTarget, ParseMode, ParseOptions, PrintOptions, parse, print_modern};
//!
//! let document = parse(
//!     "<button class='primary'>save</button>",
//!     ParseOptions {
//!         mode: ParseMode::Modern,
//!         ..ParseOptions::default()
//!     },
//! )?;
//!
//! let root = match &document.root {
//!     svelte_compiler::ast::Root::Modern(root) => root,
//!     svelte_compiler::ast::Root::Legacy(_) => unreachable!("requested a modern AST"),
//! };
//! let printed = print_modern(ModernPrintTarget::root(document.source(), root), PrintOptions::default())?;
//! assert!(printed.code.contains("<button"));
//! # Ok::<(), svelte_compiler::CompileError>(())
//! ```
mod api;
pub mod ast;
mod compiler;
pub mod cst;
mod error;
mod js;
mod names;
mod primitives;
mod printing;
mod source;

pub use api::{
    AsyncMarkupPreprocessor, AsyncTagPreprocessor, CompatibilityComponentApi, CompatibilityOptions,
    CompileMetadata, CompileOptions, CompileResult, Compiler, CssHashGetterCallback, CssHashInput,
    CssOutputMode, ErrorMode, ExperimentalOptions, FragmentStrategy, GenerateTarget,
    MarkupPreprocessor, MigrateOptions, MigrateResult, ModernPrintTarget, Namespace,
    OutputArtifact, ParseMode, ParseOptions, PreprocessAttribute, PreprocessAttributeValue,
    PreprocessAttributes, PreprocessMarkup, PreprocessOptions, PreprocessOutput, PreprocessResult,
    PreprocessTag, PreprocessorGroup, PrintOptions, PrintedOutput, SourceMap, TagPreprocessor,
    VERSION, Warning, WarningFilterCallback,
};
pub use cst::parse_svelte;
pub use error::{CompileError, CompilerDiagnosticKind, SourceLocation, SourcePosition};
pub use primitives::{BytePos, SourceId, Span};
pub use source::SourceText;

/// Parse a component into the public Svelte AST.
pub fn parse(source: &str, options: ParseOptions) -> Result<ast::Document, CompileError> {
    compiler::phases::parse::parse_component(source, options)
}

/// Print a parsed document back to Svelte source.
pub fn print(ast: &ast::Document, options: PrintOptions) -> Result<PrintedOutput, CompileError> {
    compiler::phases::transform::print_component(ast, options)
}

/// Print a node from the modern AST back to Svelte source.
pub fn print_modern(
    ast: ModernPrintTarget<'_>,
    options: PrintOptions,
) -> Result<PrintedOutput, CompileError> {
    compiler::phases::transform::print_modern_target(ast, options)
}

/// Compile a `.svelte` component into JavaScript and optional CSS artifacts.
pub fn compile(source: &str, options: CompileOptions) -> Result<CompileResult, CompileError> {
    compiler::phases::transform::compile_component(source, options)
}

/// Compile a JavaScript or TypeScript module that uses runes.
pub fn compile_module(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    compiler::phases::transform::compile_module(source, options)
}

/// Parse a stylesheet into the public CSS AST.
pub fn parse_css(source: &str) -> Result<ast::CssAst, CompileError> {
    compiler::phases::parse::parse_css(source)
}

/// Run one or more preprocessors over a component source string.
pub fn preprocess(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    compiler::phases::preprocess::preprocess(source, options)
}

/// Run one or more preprocessors over a component source string asynchronously.
pub async fn preprocess_async(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    compiler::phases::preprocess::preprocess_async(source, options).await
}

/// Attempt a best-effort migration of a component to modern Svelte syntax.
pub fn migrate(source: &str, options: MigrateOptions) -> Result<MigrateResult, CompileError> {
    compiler::phases::migrate::migrate(source, options)
}

/// Compatibility stub for the old JavaScript `walk` export.
pub fn walk() -> ! {
    panic!(
        "'svelte/compiler' no longer exports a `walk` utility — please import it directly from `estree-walker` instead"
    )
}
