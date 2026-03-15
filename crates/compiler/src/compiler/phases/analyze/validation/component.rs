use crate::api::CompileOptions;
use crate::ast::modern::Root;
use crate::error::CompileError;

mod css;
mod imports;
mod runes;
mod snippet;
mod template;

pub(crate) fn validate_component_source(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Option<CompileError> {
    if let Some(error) = template::validate(source, options, root) {
        return Some(error);
    }
    if let Some(error) = css::validate(source, root) {
        return Some(error);
    }
    if let Some(error) = imports::validate(source, root) {
        return Some(error);
    }
    if let Some(error) = snippet::validate(source, root) {
        return Some(error);
    }
    runes::validate(source, options, root)
}
