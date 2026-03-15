use crate::api::CompileOptions;
use crate::ast::modern::Root;
use crate::error::CompileError;

pub(crate) fn validate(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Option<CompileError> {
    crate::api::validation::validate_component_runes(source, options, root)
}
