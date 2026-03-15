use crate::ast::modern::Root;
use crate::error::CompileError;

pub(crate) fn validate(source: &str, root: &Root) -> Option<CompileError> {
    crate::api::validation::validate_component_imports(source, root)
}
