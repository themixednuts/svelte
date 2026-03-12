use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::Arc;

use camino::Utf8PathBuf;
use html_escape::decode_html_entities as decode_html_entities_cow;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Point};

use crate::ast::Document;
use crate::ast::modern::RootCommentType;
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
    find_first_named_child, legacy_expression_from_raw_node, parse_identifier_name,
    parse_modern_attributes, source_location_from_point, text_for_node,
};
pub(crate) use modern::parse_root as parse_root_from_cst;
pub use modern::{
    RawField, estree_node_field, estree_node_field_array, estree_node_field_mut,
    estree_node_field_object, estree_node_field_str, estree_node_has_field, estree_node_type,
    expression_identifier_name, modern_node_end, modern_node_span, modern_node_start,
    walk_estree_node, walk_raw_value,
};
pub(crate) use modern::{
    attach_leading_comments_to_expression, attach_trailing_comments_to_expression,
    estree_value_to_usize, find_matching_brace_close, legacy_expression_from_modern_expression,
    line_column_at_offset, modern_empty_identifier_expression, named_children_vec,
    normalize_pattern_template_elements, parse_all_comment_nodes,
    parse_modern_expression_from_text, parse_modern_expression_tag,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
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
pub struct ParseOptions {
    pub filename: Option<Utf8PathBuf>,
    pub root_dir: Option<Utf8PathBuf>,
    pub modern: Option<bool>,
    pub mode: ParseMode,
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

pub fn parse(source: &str, options: ParseOptions) -> Result<Document, CompileError> {
    SvelteParserCore::new(source, options).parse()
}

pub fn parse_modern_root(source: &str) -> Result<crate::ast::modern::Root, CompileError> {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = crate::cst::parse_svelte(source_text)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parse_root_from_cst(source_text.text, cst.root_node(), false)
    }))
    .map_err(|_| CompileError::internal("failed to parse component root from cst"))
}
