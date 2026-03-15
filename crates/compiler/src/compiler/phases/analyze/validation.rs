use crate::api::CompileOptions;
use crate::ast::modern::Root;
use crate::compiler::phases::parse::ParsedModuleProgram;
use crate::error::CompileError;

mod component;
mod module;

pub(crate) fn validate_component_source(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Option<CompileError> {
    component::validate_component_source(source, options, root)
}

pub(crate) fn validate_module_source(parsed: &ParsedModuleProgram<'_>) -> Option<CompileError> {
    module::validate_module_source(parsed)
}
