mod component;
mod css;
mod oxc;
mod scan;

use crate::ast::{CssAst, modern::CssNode};
use crate::error::CompileError;
pub use component::modern::{
    attach_estree_comments_to_tree, attach_leading_comments_to_expression,
    attach_trailing_comments_to_expression, estree_value_to_usize, expression_literal_bool,
    expression_literal_string, find_matching_brace_close, legacy_expression_from_modern_expression,
    line_column_at_offset, modern_empty_identifier_expression, named_children_vec,
    normalize_estree_node, normalize_pattern_template_elements, parse_all_comment_nodes,
    parse_leading_comment_nodes, parse_modern_expression_from_text, parse_modern_expression_tag,
    position_raw_node,
};
pub use component::{
    AttributeKind, ElementKind, SvelteElementKind, classify_attribute_name, classify_element_name,
    is_component_name, is_custom_element_name, is_valid_component_name, is_valid_element_name,
    is_void_element_name,
};
pub use component::{ParseMode, ParseOptions, parse, parse_modern_root};
pub use component::{
    RawField, estree_node_field, estree_node_field_array, estree_node_field_mut,
    estree_node_field_object, estree_node_field_str, estree_node_has_field, estree_node_type,
    expression_identifier_name, modern_node_end, modern_node_span, modern_node_start,
    walk_estree_node, walk_raw_value,
};
pub(crate) use scan::find_valid_legacy_closing_tag_start;
pub use scan::parse_svelte_ignores;

pub fn parse_css(source: &str) -> Result<CssAst, CompileError> {
    css::parse_css_stylesheet(source)
}

pub fn parse_modern_css_nodes(source: &str, start: usize, end: usize) -> Vec<CssNode> {
    css::parse_modern_css_nodes(source, start, end)
}

pub(crate) fn parse_modern_program_content_with_offsets(
    snippet: &str,
    global_start: usize,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
    is_ts: bool,
) -> Option<crate::ast::modern::EstreeNode> {
    oxc::SvelteOxcParser::new(snippet)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line,
            start_column,
            end_line,
            end_column,
        })
        .parse_program_for_compile(is_ts)
}

pub(crate) fn parse_modern_expression_with_oxc(
    expression: &str,
    global_start: usize,
    base_line: usize,
    base_column: usize,
) -> Option<crate::ast::modern::Expression> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line: base_line,
            start_column: base_column,
            end_line: base_line,
            end_column: base_column + expression.len(),
        })
        .parse_expression_for_template()
}

pub(crate) fn parse_modern_expression_error_with_oxc(
    expression: &str,
    global_start: usize,
    base_line: usize,
    base_column: usize,
) -> Option<std::sync::Arc<str>> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line: base_line,
            start_column: base_column,
            end_line: base_line,
            end_column: base_column + expression.len(),
        })
        .parse_expression_error_for_template()
}

pub(crate) fn parse_modern_expression_error_detail_with_oxc(
    expression: &str,
    global_start: usize,
    base_line: usize,
    base_column: usize,
) -> Option<(usize, std::sync::Arc<str>)> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line: base_line,
            start_column: base_column,
            end_line: base_line,
            end_column: base_column + expression.len(),
        })
        .parse_expression_error_detail_for_template()
}
