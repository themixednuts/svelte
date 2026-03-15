//! Parse Svelte components into typed AST and CST representations.
//!
//! This crate provides the syntax layer for working with `.svelte` files in
//! Rust. It parses components into inspectable tree structures without
//! compiling them into JavaScript or CSS.
//!
//! # Parsing a component
//!
//! Use [`parse`] or [`parse_modern_root`] to obtain a typed AST:
//!
//! ```
//! use svelte_syntax::{parse, ParseMode, ParseOptions};
//!
//! let doc = parse(
//!     "<script>let count = 0;</script><button>{count}</button>",
//!     ParseOptions {
//!         mode: ParseMode::Modern,
//!         ..ParseOptions::default()
//!     },
//! )?;
//!
//! let root = match doc.root {
//!     svelte_syntax::ast::Root::Modern(root) => root,
//!     _ => unreachable!(),
//! };
//!
//! // Access the instance script and template fragment.
//! assert!(root.instance.is_some());
//! assert!(!root.fragment.nodes.is_empty());
//! # Ok::<(), svelte_syntax::CompileError>(())
//! ```
//!
//! # Parsing into a CST
//!
//! Use [`parse_svelte`] for a tree-sitter concrete syntax tree:
//!
//! ```
//! use svelte_syntax::{SourceId, SourceText, parse_svelte};
//!
//! let source = SourceText::new(SourceId::new(0), "<div>hello</div>", None);
//! let cst = parse_svelte(source)?;
//!
//! assert_eq!(cst.root_kind(), "document");
//! assert!(!cst.has_error());
//! # Ok::<(), svelte_syntax::CompileError>(())
//! ```
//!
//! # Incremental reparsing
//!
//! Both the CST and AST support incremental reparsing. Provide the previous
//! parse result and a [`CstEdit`] describing the change, and unchanged
//! subtrees are reused via `Arc` sharing:
//!
//! ```
//! use svelte_syntax::{
//!     SourceId, SourceText, CstEdit,
//!     parse_svelte, parse_modern_root, parse_modern_root_incremental,
//! };
//!
//! let before = "<script>let x = 1;</script><div>Hello</div>";
//! let after  = "<script>let x = 1;</script><div>World</div>";
//!
//! let old_root = parse_modern_root(before)?;
//! let old_cst  = parse_svelte(SourceText::new(SourceId::new(0), before, None))?;
//!
//! let edit = CstEdit::replace(before, 37, 42, "World");
//! let new_root = parse_modern_root_incremental(after, before, &old_root, &old_cst, edit)?;
//!
//! // The script was not in the changed range, so it is Arc-reused.
//! assert!(std::sync::Arc::ptr_eq(
//!     &old_root.instance.as_ref().unwrap().content,
//!     &new_root.instance.as_ref().unwrap().content,
//! ));
//! # Ok::<(), svelte_syntax::CompileError>(())
//! ```
pub mod arena;
pub mod ast;
pub mod compat;
pub mod cst;
mod error;
pub mod js;
mod parse;
mod primitives;
mod source;

// --- CST parsing ---

pub use cst::{CstEdit, CstParser, Document, Language, parse_svelte, parse_svelte_incremental};

// --- Errors ---

pub use error::{CompileError, CompilerDiagnosticKind, LineColumn, SourcePosition};

// --- JavaScript handles ---

pub use js::{JsExpression, JsProgram};

// --- AST parsing and element/attribute classification ---

pub use parse::{
    AttributeKind, ElementKind, ParseMode, ParseOptions, SvelteElementKind,
    classify_attribute_name, classify_element_name, find_matching_brace_close, is_component_name,
    is_custom_element_name, is_valid_component_name, is_valid_element_name, is_void_element_name,
    line_column_at_offset, parse, parse_css, parse_modern_css_nodes,
    parse_modern_expression_from_text, parse_modern_expression_tag, parse_modern_root,
    parse_modern_root_incremental, parse_svelte_ignores,
};

// --- Primitives ---

pub use primitives::{BytePos, SourceId, Span};
pub use source::SourceText;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ast::modern::Node;
    use crate::cst::{CstEdit, parse_svelte};
    use crate::parse_modern_root;
    use crate::primitives::SourceId;
    use crate::source::SourceText;

    #[test]
    fn modern_root_scripts_and_template_expressions_keep_oxc_handles() {
        let root = parse_modern_root("<script>let count = 0;</script><button>{count + 1}</button>")
            .expect("modern root should parse");

        let instance = root.instance.as_ref().expect("instance script");
        assert_eq!(instance.oxc_program().body.len(), 1);

        let Node::RegularElement(element) = &root.fragment.nodes[0] else {
            panic!("expected regular element");
        };
        let Node::ExpressionTag(tag) = &element.fragment.nodes[0] else {
            panic!("expected expression tag");
        };
        assert!(tag.expression.parsed().is_some());
        assert!(tag.expression.oxc_expression().is_some());
    }

    #[test]
    fn incremental_parse_reuses_unchanged_script() {
        let before = "<script>let count = 0;</script>\n<div>Hello</div>";
        let after = "<script>let count = 0;</script>\n<div>World</div>";
        let edit_start = "<script>let count = 0;</script>\n<div>".len();
        let edit_old_end = "<script>let count = 0;</script>\n<div>Hello".len();

        let old_root = parse_modern_root(before).expect("initial parse");

        let before_src = SourceText::new(SourceId::new(1), before, None);
        let old_cst = parse_svelte(before_src).expect("initial CST parse");

        let edit = CstEdit::replace(before, edit_start, edit_old_end, "World");

        let new_root = crate::parse_modern_root_incremental(after, before, &old_root, &old_cst, edit)
            .expect("incremental parse");

        // Script was not in any changed range, so it should be Arc-identical.
        let old_script = old_root.instance.as_ref().expect("old instance");
        let new_script = new_root.instance.as_ref().expect("new instance");
        assert!(
            Arc::ptr_eq(&old_script.content, &new_script.content),
            "unchanged script should be Arc-reused (same pointer)",
        );

        // The template fragment should have reparsed.
        // There may be whitespace text nodes before the element.
        let el_node = new_root.fragment.nodes.iter().find(|n| matches!(n, Node::RegularElement(_)))
            .expect("expected regular element in new root fragment");
        let Node::RegularElement(new_el) = el_node else { unreachable!() };
        let Node::Text(text) = &new_el.fragment.nodes[0] else {
            panic!("expected text node in element fragment");
        };
        assert_eq!(text.data.as_ref(), "World");
    }

    /// Verify that tree-sitter's `changed_ranges` reports *structural* changes only.
    /// A content-only edit (same tree shape) yields empty changed ranges.
    #[test]
    fn verify_tree_sitter_changed_ranges_are_structural_only() {
        // Both produce identical CST structure (element > start_tag > tag_name, text, end_tag > tag_name)
        let before = "<div>A</div>";
        let after = "<div>X</div>";

        let before_src = SourceText::new(SourceId::new(50), before, None);
        let old_cst = parse_svelte(before_src).expect("cst");
        let mut edited = old_cst.clone_for_incremental();
        let edit = CstEdit::replace(before, 5, 6, "X");
        edited.apply_edit(edit);

        let after_src = SourceText::new(SourceId::new(51), after, None);
        let new_cst = crate::cst::parse_svelte_with_old_tree(after_src, &edited).expect("cst");
        let ranges = new_cst.changed_ranges(&edited);

        // Tree-sitter does NOT report content-only changes as changed ranges.
        // Both trees have the same shape: (document (element (start_tag (tag_name)) (text) (end_tag (tag_name))))
        assert!(
            ranges.is_empty(),
            "tree-sitter changed_ranges should be empty for same-structure edit, got: {ranges:?}"
        );

        // Now test a structural change: adding a new element
        let before2 = "<div>A</div>";
        let after2 = "<div>A</div><span>B</span>";
        let before_src2 = SourceText::new(SourceId::new(52), before2, None);
        let old_cst2 = parse_svelte(before_src2).expect("cst");
        let mut edited2 = old_cst2.clone_for_incremental();
        let edit2 = CstEdit::insert(before2, before2.len(), "<span>B</span>");
        edited2.apply_edit(edit2);

        let after_src2 = SourceText::new(SourceId::new(53), after2, None);
        let new_cst2 = crate::cst::parse_svelte_with_old_tree(after_src2, &edited2).expect("cst");
        let ranges2 = new_cst2.changed_ranges(&edited2);

        // Structural change: new element added. Should have changed ranges.
        assert!(
            !ranges2.is_empty(),
            "tree-sitter changed_ranges should be non-empty for structural edit"
        );
    }

    #[test]
    fn incremental_parse_reuses_unchanged_sibling_element() {
        let before = "<div>A</div><span>B</span>";
        let after = "<div>X</div><span>B</span>";
        let edit_start = "<div>".len();
        let edit_old_end = "<div>A".len();

        let old_root = parse_modern_root(before).expect("initial parse");

        let before_src = SourceText::new(SourceId::new(3), before, None);
        let old_cst = parse_svelte(before_src).expect("initial CST parse");

        let edit = CstEdit::replace(before, edit_start, edit_old_end, "X");

        let new_root = crate::parse_modern_root_incremental(after, before, &old_root, &old_cst, edit)
            .expect("incremental parse");

        // <span>B</span> was unchanged — should be reused.
        assert_eq!(new_root.fragment.nodes.len(), 2);
        let Node::RegularElement(new_span) = &new_root.fragment.nodes[1] else {
            panic!("expected span element");
        };
        assert_eq!(new_span.name.as_ref(), "span");

        // Verify the div was reparsed with new content.
        let Node::RegularElement(new_div) = &new_root.fragment.nodes[0] else {
            panic!("expected div element");
        };
        let Node::Text(text) = &new_div.fragment.nodes[0] else {
            panic!("expected text in div");
        };
        assert_eq!(text.data.as_ref(), "X");
    }
}
