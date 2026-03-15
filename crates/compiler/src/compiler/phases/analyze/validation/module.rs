use crate::{compiler::phases::parse::ParsedModuleProgram, error::CompileError};

pub(crate) fn validate_module_source(parsed: &ParsedModuleProgram<'_>) -> Option<CompileError> {
    crate::api::validation::validate_module_program(parsed)
}
