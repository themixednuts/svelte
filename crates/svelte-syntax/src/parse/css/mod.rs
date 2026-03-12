mod parser;

use lightningcss::stylesheet::{ParserOptions, StyleSheet};

use crate::ast::modern::CssNode;
use crate::ast::{CssAst, CssRootType};
use crate::error::CompileError;

pub(crate) fn parse_css_stylesheet(source: &str) -> Result<CssAst, CompileError> {
    let css_source = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    StyleSheet::parse(css_source, ParserOptions::default())
        .map_err(|err| CompileError::internal(format!("lightningcss parse failed: {err}")))?;
    let mut parser = parser::CssParser::new(css_source, 0, css_source.len());

    Ok(CssAst {
        r#type: CssRootType::StyleSheetFile,
        children: parser.read_body().into_boxed_slice(),
        start: 0,
        end: css_source.len(),
    })
}

pub(crate) fn parse_modern_css_nodes(source: &str, start: usize, end: usize) -> Vec<CssNode> {
    let mut parser = parser::CssParser::new(source, start, end);
    parser.read_body()
}
