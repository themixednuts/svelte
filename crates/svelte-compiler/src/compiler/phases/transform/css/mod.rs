mod rewrite;
mod scoping;
mod usage;

use crate::api::SourceMap;

#[derive(Debug, Clone)]
pub(crate) struct TextReplacement {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct GeneratedCssOutput {
    pub(crate) code: String,
    pub(crate) map: Option<SourceMap>,
}

pub(crate) use rewrite::generate_component_css_output;
