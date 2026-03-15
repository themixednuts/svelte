mod component;
mod css;
mod oxc;
mod oxc_query;
mod scan;

use std::sync::Arc;

use crate::ast::{CssAst, modern::CssNode};
use crate::error::CompileError;
use crate::js::JsProgram;
pub use component::modern::{
    find_matching_brace_close, legacy_expression_from_modern_expression, line_column_at_offset,
    parse_modern_expression_from_text, parse_modern_expression_tag,
};
pub use component::{
    AttributeKind, ElementKind, SvelteElementKind, classify_attribute_name, classify_element_name,
    is_component_name, is_custom_element_name, is_valid_component_name, is_valid_element_name,
    is_void_element_name,
};
pub use component::{ParseMode, ParseOptions, parse, parse_modern_root, parse_modern_root_incremental};
pub(crate) use scan::find_valid_legacy_closing_tag_start;
pub use scan::parse_svelte_ignores;

pub(crate) struct ParsedProgramContent {
    pub parsed: Arc<JsProgram>,
}

pub fn parse_css(source: &str) -> Result<CssAst, CompileError> {
    css::parse_css_stylesheet(source)
}

pub fn parse_modern_css_nodes(source: &str, start: usize, end: usize) -> Vec<CssNode> {
    css::parse_modern_css_nodes(source, start, end)
}

pub(crate) fn parse_modern_program_content_with_offsets(
    snippet: &str,
    global_start: usize,
    _start_line: usize,
    _start_column: usize,
    _end_line: usize,
    _end_column: usize,
    is_ts: bool,
) -> Option<ParsedProgramContent> {
    oxc::SvelteOxcParser::new(snippet)
        .with_offsets(oxc::OxcProgramOffsets { global_start })
        .parse_program_for_compile(is_ts)
}

pub(crate) fn parse_modern_expression_with_oxc(
    expression: &str,
    global_start: usize,
    _base_line: usize,
    _base_column: usize,
) -> Option<crate::ast::modern::Expression> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets { global_start })
        .parse_expression_for_template()
}

pub(crate) fn parse_modern_expression_error_with_oxc(
    expression: &str,
    global_start: usize,
    _base_line: usize,
    _base_column: usize,
) -> Option<std::sync::Arc<str>> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets { global_start })
        .parse_expression_error_for_template()
}

pub(crate) fn parse_modern_expression_error_detail_with_oxc(
    expression: &str,
    global_start: usize,
    _base_line: usize,
    _base_column: usize,
) -> Option<(usize, std::sync::Arc<str>)> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets { global_start })
        .parse_expression_error_detail_for_template()
}
