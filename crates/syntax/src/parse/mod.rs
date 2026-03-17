mod component;
mod css;
mod oxc;
mod oxc_query;
mod scan;

use std::sync::Arc;
use std::cell::Cell;

use crate::ast::{CssAst, modern::CssNode};
use crate::error::CompileError;
use crate::js::JsProgram;

// Thread-local parse timing counters for profiling.
thread_local! {
    static OXC_EXPR_TIME_US: Cell<u64> = const { Cell::new(0) };
    static OXC_EXPR_COUNT: Cell<u64> = const { Cell::new(0) };
    static OXC_PROGRAM_TIME_US: Cell<u64> = const { Cell::new(0) };
    static OXC_PROGRAM_COUNT: Cell<u64> = const { Cell::new(0) };
}

/// Reset all thread-local parse timing counters.
pub fn reset_parse_counters() {
    OXC_EXPR_TIME_US.set(0);
    OXC_EXPR_COUNT.set(0);
    OXC_PROGRAM_TIME_US.set(0);
    OXC_PROGRAM_COUNT.set(0);
}

/// Snapshot of thread-local parse timing counters.
#[derive(Debug, Clone)]
pub struct ParseCounters {
    pub oxc_expr_us: u64,
    pub oxc_expr_count: u64,
    pub oxc_program_us: u64,
    pub oxc_program_count: u64,
}

/// Read current thread-local parse timing counters.
pub fn read_parse_counters() -> ParseCounters {
    ParseCounters {
        oxc_expr_us: OXC_EXPR_TIME_US.get(),
        oxc_expr_count: OXC_EXPR_COUNT.get(),
        oxc_program_us: OXC_PROGRAM_TIME_US.get(),
        oxc_program_count: OXC_PROGRAM_COUNT.get(),
    }
}
pub use component::modern::{
    find_matching_brace_close, legacy_expression_from_modern_expression, line_column_at_offset,
    parse_modern_expression_from_text, parse_modern_expression_tag,
};
pub use component::{
    AttributeKind, ElementKind, SvelteElementKind, classify_attribute_name, classify_element_name,
    is_component_name, is_custom_element_name, is_valid_component_name, is_valid_element_name,
    is_void_element_name,
};
pub use component::{
    ParseMode, ParseOptions, ParseTimings, legacy_root_from_modern, parse,
    parse_legacy_root_from_cst, parse_modern_root, parse_modern_root_incremental,
    parse_modern_root_timed,
};
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
    let t = std::time::Instant::now();
    let result = oxc::SvelteOxcParser::new(snippet)
        .with_offsets(oxc::OxcProgramOffsets { global_start })
        .parse_program_for_compile(is_ts);
    let elapsed = t.elapsed().as_micros() as u64;
    OXC_PROGRAM_TIME_US.set(OXC_PROGRAM_TIME_US.get() + elapsed);
    OXC_PROGRAM_COUNT.set(OXC_PROGRAM_COUNT.get() + 1);
    result
}

pub(crate) fn parse_modern_expression_with_oxc(
    expression: &str,
    global_start: usize,
    _base_line: usize,
    _base_column: usize,
) -> Option<crate::ast::modern::Expression> {
    let t = std::time::Instant::now();
    let result = oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets { global_start })
        .parse_expression_for_template();
    let elapsed = t.elapsed().as_micros() as u64;
    OXC_EXPR_TIME_US.set(OXC_EXPR_TIME_US.get() + elapsed);
    OXC_EXPR_COUNT.set(OXC_EXPR_COUNT.get() + 1);
    result
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
