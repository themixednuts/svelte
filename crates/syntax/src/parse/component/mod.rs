use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use camino::Utf8PathBuf;
use html_escape::decode_html_entities as decode_html_entities_cow;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Point};

use crate::ast::Document;
use crate::{CompileError, SourceId, SourceLocation, SourceText};

mod elements;
mod legacy;
pub(crate) mod modern;

pub use elements::{
    AttributeKind, ElementKind, SvelteElementKind, classify_attribute_name, classify_element_name,
    is_component_name, is_custom_element_name, is_valid_component_name, is_valid_element_name,
    is_void_element_name,
};
pub(crate) use legacy::parse_root as parse_legacy_root_from_cst;
pub(crate) use legacy::{
    find_first_named_child, parse_identifier_name, parse_modern_attributes,
    source_location_from_point, text_for_node,
};
pub(crate) use modern::parse_root as parse_root_from_cst;
pub(crate) use modern::parse_root_incremental as parse_root_incremental_from_cst;
pub use modern::{
    expression_identifier_name, modern_node_end, modern_node_span, modern_node_start,
};
pub(crate) use modern::{
    attach_leading_comments_to_expression, attach_trailing_comments_to_expression,
    find_matching_brace_close, line_column_at_offset, modern_empty_identifier_expression,
    named_children_vec, parse_modern_expression_from_text, parse_modern_expression_tag,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
/// Selects which public AST shape the parser should return.
pub enum ParseMode {
    #[default]
    Legacy,
    Modern,
}

impl ParseMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Modern => "modern",
        }
    }
}

impl fmt::Display for ParseMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ParseMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "legacy" => Ok(Self::Legacy),
            "modern" => Ok(Self::Modern),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
/// Options for parsing Svelte source into the public AST.
pub struct ParseOptions {
    /// Optional source filename used in diagnostics.
    pub filename: Option<Utf8PathBuf>,
    /// Optional project root used by path-sensitive tooling.
    pub root_dir: Option<Utf8PathBuf>,
    /// Compatibility flag matching Svelte's JavaScript API.
    pub modern: Option<bool>,
    /// Preferred AST shape when `modern` is not set.
    pub mode: ParseMode,
    /// Return a best-effort AST for malformed input when possible.
    pub loose: bool,
}

impl ParseOptions {
    #[must_use]
    pub fn effective_mode(&self) -> ParseMode {
        match self.modern {
            Some(true) => ParseMode::Modern,
            Some(false) => ParseMode::Legacy,
            None => self.mode,
        }
    }
}

struct SvelteParserCore<'src> {
    source: &'src str,
    source_filename: Option<Utf8PathBuf>,
    options: ParseOptions,
}

impl<'src> SvelteParserCore<'src> {
    fn new(source: &'src str, options: ParseOptions) -> Self {
        Self {
            source,
            source_filename: options.filename.clone(),
            options,
        }
    }

    fn parse_root(&self, root: Node<'_>) -> crate::ast::Root {
        match self.options.effective_mode() {
            ParseMode::Legacy => crate::ast::Root::Legacy(parse_legacy_root_from_cst(
                self.source,
                root,
                self.options.loose,
            )),
            ParseMode::Modern => {
                crate::ast::Root::Modern(parse_root_from_cst(self.source, root, self.options.loose))
            }
        }
    }

    fn parse(self) -> Result<Document, CompileError> {
        let source_text = SourceText::new(
            SourceId::new(0),
            self.source,
            self.source_filename.as_deref(),
        );
        let cst = crate::cst::parse_svelte(source_text)?;
        Ok(Document {
            root: self.parse_root(cst.root_node()),
            source: Arc::from(self.source),
        })
    }
}

/// Parse a Svelte component into the public AST.
///
/// This matches the shape of Svelte's `parse(...)` API and can return either
/// the legacy or modern AST.
pub fn parse(source: &str, options: ParseOptions) -> Result<Document, CompileError> {
    SvelteParserCore::new(source, options).parse()
}

/// Parse a Svelte component directly into the modern AST root.
pub fn parse_modern_root(source: &str) -> Result<crate::ast::modern::Root, CompileError> {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = crate::cst::parse_svelte(source_text)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parse_root_from_cst(source_text.text, cst.root_node(), false)
    }))
    .map_err(|_| CompileError::internal("failed to parse component root from cst"))
}

/// Parse a Svelte component into the modern AST root incrementally, reusing
/// unchanged subtrees from a previous parse. Requires a previous CST document
/// and the CST edit that was applied so tree-sitter can compute changed ranges.
///
/// Falls back to a full parse if the CST reports an error root.
pub fn parse_modern_root_incremental(
    source: &str,
    old_source: &str,
    old_root: &crate::ast::modern::Root,
    old_cst: &crate::cst::Document<'_>,
    edit: crate::cst::CstEdit,
) -> Result<crate::ast::modern::Root, CompileError> {
    use crate::cst;

    let source_text = SourceText::new(SourceId::new(0), source, None);

    // Build an edited copy of the old CST for two purposes:
    // 1. tree-sitter incremental parsing (`parser.parse(new_src, Some(&edited_old))`)
    // 2. computing changed_ranges (`edited_old.changed_ranges(&new_tree)`)
    let mut edited_old_cst = old_cst.clone_for_incremental();
    edited_old_cst.apply_edit(edit);

    let new_cst = cst::parse_svelte_with_old_tree(source_text, &edited_old_cst)?;
    let changed_ranges = new_cst.changed_ranges(&edited_old_cst);

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parse_root_incremental_from_cst(
            source_text.text,
            new_cst.root_node(),
            false,
            old_root,
            old_source,
            &changed_ranges,
        )
    }))
    .map_err(|_| CompileError::internal("failed to incrementally parse component root from cst"))
}
