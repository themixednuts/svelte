use crate::error::CompileError;

pub(crate) fn validate_module_source(source: &str) -> Option<CompileError> {
    crate::api::validation::validate_module_program(source)
}
