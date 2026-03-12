mod api;
pub mod ast;
mod compiler;
pub mod cst;
mod error;
mod estree;
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
    PreprocessTag, PreprocessorGroup, PrintCommentGetterCallback, PrintOptions, PrintedOutput,
    SourceMap, TagPreprocessor, VERSION, Warning, WarningFilterCallback,
};
pub use cst::parse_svelte;
pub use error::{CompileError, CompilerDiagnosticKind, SourceLocation, SourcePosition};
pub use primitives::{BytePos, SourceId, Span};
pub use source::SourceText;

pub fn parse(source: &str, options: ParseOptions) -> Result<ast::Document, CompileError> {
    compiler::phases::parse::parse_component(source, options)
}

pub fn print(ast: &ast::Document, options: PrintOptions) -> Result<PrintedOutput, CompileError> {
    compiler::phases::transform::print_component(ast, options)
}

pub fn print_modern(
    ast: ModernPrintTarget<'_>,
    options: PrintOptions,
) -> Result<PrintedOutput, CompileError> {
    compiler::phases::transform::print_modern_target(ast, options)
}

pub fn compile(source: &str, options: CompileOptions) -> Result<CompileResult, CompileError> {
    compiler::phases::transform::compile_component(source, options)
}

pub fn compile_module(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    compiler::phases::transform::compile_module(source, options)
}

pub fn parse_css(source: &str) -> Result<ast::CssAst, CompileError> {
    compiler::phases::parse::parse_css(source)
}

pub fn preprocess(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    compiler::phases::preprocess::preprocess(source, options)
}

pub fn migrate(source: &str, options: MigrateOptions) -> Result<MigrateResult, CompileError> {
    compiler::phases::migrate::migrate(source, options)
}

pub fn walk() -> ! {
    panic!(
        "'svelte/compiler' no longer exports a `walk` utility — please import it directly from `estree-walker` instead"
    )
}
