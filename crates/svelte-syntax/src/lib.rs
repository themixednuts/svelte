//! Syntax-only Svelte parsing utilities.
//!
//! This crate exposes the Svelte AST, CST, and parser entrypoints without the
//! compiler pipeline.
//!
//! # Examples
//!
//! Parse a component into the public AST:
//!
//! ```
//! use svelte_syntax::{parse, ParseMode, ParseOptions};
//!
//! let document = parse(
//!     "<script>let count = 0;</script><button>{count}</button>",
//!     ParseOptions {
//!         mode: ParseMode::Modern,
//!         ..ParseOptions::default()
//!     },
//! )?;
//!
//! assert!(matches!(document.root, svelte_syntax::ast::Root::Modern(_)));
//! # Ok::<(), svelte_syntax::CompileError>(())
//! ```
//!
//! Parse raw source into a tree-sitter CST:
//!
//! ```
//! use svelte_syntax::{SourceId, SourceText, parse_svelte};
//!
//! let source = SourceText::new(SourceId::new(0), "<div class='greeting'>hi</div>", None);
//! let cst = parse_svelte(source)?;
//!
//! assert_eq!(cst.root_kind(), "document");
//! # Ok::<(), svelte_syntax::CompileError>(())
//! ```
pub mod ast;
pub mod cst;
mod error;
mod parse;
mod primitives;
mod source;

/// Parse Svelte source into a tree-sitter CST.
pub use cst::parse_svelte;
pub use error::{CompileError, CompilerDiagnosticKind, SourceLocation, SourcePosition};
/// Parse Svelte source into the public AST.
pub use parse::{
    AttributeKind, ElementKind, ParseMode, ParseOptions, RawField, SvelteElementKind,
    attach_estree_comments_to_tree, attach_leading_comments_to_expression,
    attach_trailing_comments_to_expression, classify_attribute_name, classify_element_name,
    estree_node_field, estree_node_field_array, estree_node_field_mut, estree_node_field_object,
    estree_node_field_str, estree_node_has_field, estree_node_type, estree_value_to_usize,
    expression_identifier_name, expression_literal_bool, expression_literal_string,
    find_matching_brace_close, is_component_name, is_custom_element_name, is_valid_component_name,
    is_valid_element_name, is_void_element_name, legacy_expression_from_modern_expression,
    line_column_at_offset, modern_empty_identifier_expression, modern_node_end, modern_node_span,
    modern_node_start, named_children_vec, normalize_estree_node,
    normalize_pattern_template_elements, parse, parse_all_comment_nodes, parse_css,
    parse_leading_comment_nodes, parse_modern_css_nodes, parse_modern_expression_from_text,
    parse_modern_expression_tag, parse_modern_root, parse_svelte_ignores, position_raw_node,
    walk_estree_node, walk_raw_value,
};
pub use primitives::{BytePos, SourceId, Span};
/// Source text plus filename and offset helpers used by the parser.
pub use source::SourceText;
