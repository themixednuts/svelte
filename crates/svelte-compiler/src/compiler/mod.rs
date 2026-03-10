pub(crate) mod phases;

use crate::api::{
    CompileOptions, CompileResult, ModernPrintTarget, ParseOptions, PrintOptions, PrintedOutput,
};
use crate::ast::{CssAst, Document};
use crate::error::CompileError;
use crate::{MigrateOptions, MigrateResult, PreprocessOptions, PreprocessResult};

pub(crate) fn parse(source: &str, options: ParseOptions) -> Result<Document, CompileError> {
    phases::parse::parse_component(source, options)
}

pub(crate) fn print(ast: &Document, options: PrintOptions) -> Result<PrintedOutput, CompileError> {
    phases::transform::print_component(ast, options)
}

pub(crate) fn print_modern(
    ast: ModernPrintTarget<'_>,
    options: PrintOptions,
) -> Result<PrintedOutput, CompileError> {
    phases::transform::print_modern_target(ast, options)
}

pub(crate) fn compile(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    phases::transform::compile_component(source, options)
}

pub(crate) fn compile_module(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    phases::transform::compile_module(source, options)
}

pub(crate) fn parse_css(source: &str) -> Result<CssAst, CompileError> {
    phases::parse::parse_css(source)
}

pub(crate) fn preprocess(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    phases::preprocess::preprocess(source, options)
}

pub(crate) fn migrate(
    source: &str,
    options: MigrateOptions,
) -> Result<MigrateResult, CompileError> {
    phases::migrate::migrate(source, options)
}
