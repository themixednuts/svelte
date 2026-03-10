use crate::ast::modern::Root;

/// Script/style ranges are CST-derived; no source-string scanning fallback.
#[derive(Clone, Default)]
pub(crate) struct ScriptStyleRegions {
    pub non_module_script_content_ranges: Vec<(usize, usize)>,
    /// (block_start, block_end, content_start, content_end)
    pub style_block_ranges: Vec<(usize, usize, usize, usize)>,
}

impl ScriptStyleRegions {
    pub(crate) fn from_root(root: &Root) -> Self {
        let mut non_module_script_content_ranges = Vec::new();
        if let Some(ref script) = root.instance {
            non_module_script_content_ranges.push((script.content_start, script.content_end));
        }
        let style_block_ranges = root
            .css
            .as_ref()
            .map(|css| (css.start, css.end, css.content.start, css.content.end))
            .into_iter()
            .collect();
        Self {
            non_module_script_content_ranges,
            style_block_ranges,
        }
    }
}

pub(crate) fn script_style_regions(root: &Root) -> ScriptStyleRegions {
    ScriptStyleRegions::from_root(root)
}

pub(crate) fn non_module_script_content_ranges(root: &Root) -> Vec<(usize, usize)> {
    script_style_regions(root).non_module_script_content_ranges
}

pub(crate) fn style_block_ranges(root: &Root) -> Vec<(usize, usize, usize, usize)> {
    script_style_regions(root).style_block_ranges
}
