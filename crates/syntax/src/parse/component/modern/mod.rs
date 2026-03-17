use std::ops::Range;
use std::sync::Arc;

use html_escape::decode_html_entities as decode_html_entities_cow;
use oxc_span::GetSpan;
use tree_sitter::Node as TsNode;

use super::{
    AttributeKind, ElementKind, SvelteElementKind, classify_attribute_name, classify_element_name,
    find_first_named_child, is_component_name,
    parse_identifier_name, parse_modern_attributes, line_column_from_point, text_for_node,
};
use crate::ast::common::{AttributeValueSyntax, ParseErrorKind, Span};
use crate::ast::modern::*;

pub(super) mod errors;
pub(super) mod expressions;
pub(super) mod snippets;

// Re-export everything that was previously visible from the `modern` module.
pub use expressions::{
    find_matching_brace_close,
    modern_empty_identifier_expression,
    parse_modern_expression_from_text,
    attach_leading_comments_to_expression,
    attach_trailing_comments_to_expression,
    line_column_at_offset,
    legacy_expression_from_modern_expression,
    named_children_vec,
};
pub(crate) use expressions::{
    split_top_level_commas,
    parse_pattern_with_oxc,
};
pub(crate) use expressions::else_clause_body_nodes;
pub(super) use expressions::parse_modern_expression_field;
pub(super) use snippets::{
    parse_snippet_type_params,
    parse_snippet_params,
    parse_snippet_name,
};

// Internal imports from submodules used within this file.
use errors::collect_parse_errors;
use expressions::{
    parse_modern_expression,
    parse_modern_expression_field_or_empty,
    modern_empty_identifier_at_block_tag_end,
    parse_modern_binding_field,
    parse_modern_binding_field_with_error,
    modern_empty_identifier_expression_span,
    location_at_offset,
};
use snippets::{
    parse_modern_snippet_block,
    recover_multiple_snippet_blocks,
    recover_malformed_snippet_block,
};

// ---------------------------------------------------------------------------
// Incremental parsing support
// ---------------------------------------------------------------------------

/// Hint passed through recursive parse functions during incremental parsing.
/// Contains changed ranges from tree-sitter and old AST nodes for reuse.
pub(crate) struct IncrementalHint<'a> {
    pub changed_ranges: &'a [std::ops::Range<usize>],
    /// Old source text for content comparison during node matching.
    pub old_source: &'a str,
    /// Old fragment nodes available for reuse at this level.
    pub old_nodes: &'a [Node],
    /// Old root, available only at the top level for script/style matching.
    pub old_root: Option<&'a Root>,
}

/// Returns `true` if any changed range overlaps the half-open byte interval `[start, end)`.
fn any_range_overlaps(changed: &[std::ops::Range<usize>], start: usize, end: usize) -> bool {
    changed.iter().any(|r| r.start < end && r.end > start)
}

/// Try to find a reusable old `Node` for a CST child that is outside all changed ranges.
///
/// Uses ordered matching: advances `*cursor` through `old_nodes` looking for a node
/// whose byte length matches `new_len` AND whose source content is identical.
/// Returns `Some(cloned_node)` on match.
fn try_reuse_node(
    old_source: &str,
    new_source: &str,
    old_nodes: &[Node],
    cursor: &mut usize,
    new_start: usize,
    new_end: usize,
) -> Option<Node> {
    let new_len = new_end - new_start;
    let new_text = new_source.get(new_start..new_end)?;
    // Scan forward (skip at most a few old nodes that were removed or shifted).
    let scan_limit = (*cursor + 4).min(old_nodes.len());
    for (i, old) in old_nodes.iter().enumerate().take(scan_limit).skip(*cursor) {
        let old_start = old.start();
        let old_end = old.end();
        let old_len = old_end - old_start;
        if old_len == new_len
            && let Some(old_text) = old_source.get(old_start..old_end)
            && old_text == new_text
        {
            *cursor = i + 1;
            return Some(old.clone());
        }
    }
    None
}

/// Extract the child fragment nodes from a `Node`, if it has a fragment.
fn node_child_nodes(node: &Node) -> &[Node] {
    match node {
        Node::RegularElement(el) => &el.fragment.nodes,
        Node::Component(el) => &el.fragment.nodes,
        Node::SlotElement(el) => &el.fragment.nodes,
        Node::SvelteHead(el) => &el.fragment.nodes,
        Node::SvelteBody(el) => &el.fragment.nodes,
        Node::SvelteWindow(el) => &el.fragment.nodes,
        Node::SvelteDocument(el) => &el.fragment.nodes,
        Node::SvelteComponent(el) => &el.fragment.nodes,
        Node::SvelteElement(el) => &el.fragment.nodes,
        Node::SvelteSelf(el) => &el.fragment.nodes,
        Node::SvelteFragment(el) => &el.fragment.nodes,
        Node::SvelteBoundary(el) => &el.fragment.nodes,
        Node::TitleElement(el) => &el.fragment.nodes,
        Node::IfBlock(b) => &b.consequent.nodes,
        Node::EachBlock(b) => &b.body.nodes,
        Node::KeyBlock(b) => &b.fragment.nodes,
        Node::AwaitBlock(_) | Node::SnippetBlock(_) => &[],
        Node::Text(_) | Node::Comment(_) | Node::ExpressionTag(_) | Node::RenderTag(_)
        | Node::HtmlTag(_) | Node::ConstTag(_) | Node::DebugTag(_) => &[],
    }
}

/// Try to reuse a `Script` from the old root by matching script context.
/// Determines context from the CST element's attributes and finds the old
/// script with the same context.
fn try_reuse_script(source: &str, element: TsNode<'_>, old_root: &Root) -> Option<Script> {
    // Determine the context of the new CST script element.
    let mut tag_cursor = element.walk();
    let start_tag = element
        .named_children(&mut tag_cursor)
        .find(|c| c.kind() == "start_tag")?;
    let attrs_text = text_for_node(source, start_tag);
    let new_context = if attrs_text.contains("module")
        || attrs_text.contains("context=\"module\"")
        || attrs_text.contains("context='module'")
    {
        ScriptContext::Module
    } else {
        ScriptContext::Default
    };

    old_root
        .scripts
        .iter()
        .find(|s| s.context == new_context)
        .cloned()
}

/// Try to reuse the CSS from the old root.
fn try_reuse_style(old_root: &Root) -> Option<Css> {
    old_root.css.clone()
}

/// Find an old `Node` by byte length (non-consuming lookahead for building child hints).
/// Returns a reference without advancing the cursor.
fn find_old_node_by_kind<'a>(
    old_nodes: &'a [Node],
    cursor: &mut usize,
    new_len: usize,
    _kind: &str,
) -> Option<&'a Node> {
    let scan_limit = (*cursor + 4).min(old_nodes.len());
    for (i, old) in old_nodes.iter().enumerate().take(scan_limit).skip(*cursor) {
        let old_len = old.end() - old.start();
        if old_len == new_len {
            *cursor = i + 1;
            return Some(old);
        }
    }
    None
}

/// Build a child `IncrementalHint` for a CST child that overlaps a changed range
/// but has a corresponding old AST node whose children can still be partially reused.
fn make_child_hint<'a>(
    parent_hint: &'a IncrementalHint<'a>,
    old_node_cursor: &mut usize,
    child_start: usize,
    child_end: usize,
    kind: &str,
) -> Option<IncrementalHint<'a>> {
    let old = find_old_node_by_kind(
        parent_hint.old_nodes,
        old_node_cursor,
        child_end - child_start,
        kind,
    )?;
    let children = node_child_nodes(old);
    if children.is_empty() {
        return None;
    }
    Some(IncrementalHint {
        changed_ranges: parent_hint.changed_ranges,
        old_source: parent_hint.old_source,
        old_nodes: children,
        old_root: None,
    })
}

// ---------------------------------------------------------------------------

pub(crate) fn parse_root(source: &str, root: TsNode<'_>, loose: bool) -> Root {
    parse_root_inner(source, root, loose, None)
}

pub(crate) fn parse_root_incremental(
    source: &str,
    root: TsNode<'_>,
    loose: bool,
    old_root: &Root,
    old_source: &str,
    changed_ranges: &[Range<usize>],
) -> Root {
    let hint = IncrementalHint {
        changed_ranges,
        old_source,
        old_nodes: &old_root.fragment.nodes,
        old_root: Some(old_root),
    };
    parse_root_inner(source, root, loose, Some(hint))
}

fn parse_root_inner(
    source: &str,
    root: TsNode<'_>,
    loose: bool,
    hint: Option<IncrementalHint<'_>>,
) -> Root {
    let errors = collect_parse_errors(source, root);

    if root.kind() == "ERROR" {
        let fragment_nodes = recover_modern_error_nodes(source, root, false);
        return Root {
            css: None,
            styles: Box::new([]),
            js: Box::new([]),
            scripts: Box::new([]),
            start: root.start_byte(),
            end: root.end_byte(),
            r#type: RootType::Root,
            fragment: crate::ast::modern::Fragment {
                r#type: crate::ast::modern::FragmentType::Fragment,
                nodes: fragment_nodes.into_boxed_slice(),
            },
            options: None,
            module: None,
            instance: None,
            comments: None,
            errors: errors.into_boxed_slice(),
        };
    }

    let mut css = None;
    let mut styles = Vec::new();
    let mut options = None;
    let mut module = None;
    let mut instance = None;
    let mut js = Vec::new();
    let mut fragment_nodes = Vec::new();
    let mut root_comments = Vec::new();
    let mut pending_script_comment: Option<Arc<str>> = None;
    let mut previous_child_end = None;
    let mut old_node_cursor = 0usize;

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if let Some(gap_start) = previous_child_end {
            push_modern_gap_text(source, &mut fragment_nodes, gap_start, child.start_byte());
        }

        let child_start = child.start_byte();
        let child_end = child.end_byte();

        // Incremental reuse: if this child is outside all changed ranges,
        // try to clone the corresponding old AST node instead of parsing.
        if let Some(ref hint) = hint
            && !any_range_overlaps(hint.changed_ranges, child_start, child_end)
        {
            // Scripts: reuse by matching context on old root.
            if child.kind() == "element"
                && let Some(name) = modern_element_name(source, child)
            {
                match classify_element_name(name.as_ref()) {
                    ElementKind::Script => {
                        if let Some(old_root) = hint.old_root
                            && let Some(old_script) = try_reuse_script(source, child, old_root)
                        {
                            js.push(old_script.clone());
                            match old_script.context {
                                ScriptContext::Module => {
                                    if module.is_none() {
                                        module = Some(old_script);
                                    }
                                }
                                ScriptContext::Default => {
                                    if instance.is_none() {
                                        instance = Some(old_script);
                                    }
                                }
                            }
                            pending_script_comment = None;
                            previous_child_end = Some(child_end);
                            continue;
                        }
                    }
                    ElementKind::Style => {
                        if let Some(old_root) = hint.old_root
                            && let Some(old_style) = try_reuse_style(old_root)
                        {
                            if css.is_none() {
                                css = Some(old_style.clone());
                            }
                            styles.push(old_style);
                            pending_script_comment = None;
                            previous_child_end = Some(child_end);
                            continue;
                        }
                    }
                    _ => {}
                }
            }

            // Fragment nodes: try ordered reuse by byte length.
            if let Some(reused) = try_reuse_node(
                hint.old_source,
                source,
                hint.old_nodes,
                &mut old_node_cursor,
                child_start,
                child_end,
            ) {
                fragment_nodes.push(reused);
                previous_child_end = Some(child_end);
                continue;
            }
        }

        match child.kind() {
            "text" | "entity" => {
                let text_node = parse_modern_text(source, child);
                if text_node.data.chars().all(char::is_whitespace) {
                    push_modern_text_node(&mut fragment_nodes, text_node);
                } else {
                    pending_script_comment = None;
                    push_modern_text_node(&mut fragment_nodes, text_node);
                }
            }
            "comment" => {
                let comment = parse_modern_comment(source, child);
                pending_script_comment = Some(comment.data.clone());
                fragment_nodes.push(crate::ast::modern::Node::Comment(comment));
            }
            "expression" => {
                let tag = if loose {
                    Some(parse_modern_expression_tag_loose(source, child))
                } else {
                    parse_modern_expression_tag(source, child)
                };
                if let Some(tag) = tag {
                    fragment_nodes.push(crate::ast::modern::Node::ExpressionTag(tag));
                }
            }
            kind if is_typed_block_kind(kind) => {
                pending_script_comment = None;
                let child_hint = hint.as_ref().and_then(|h| {
                    make_child_hint(h, &mut old_node_cursor, child_start, child_end, kind)
                });
                if let Some(block_node) = parse_modern_block(source, child, child_hint.as_ref()) {
                    fragment_nodes.push(block_node);
                }
            }
            kind if is_typed_tag_kind(kind) => {
                pending_script_comment = None;
                if let Some(tag_node) = parse_modern_tag(source, child) {
                    fragment_nodes.push(tag_node);
                }
            }
            "element" => {
                if let Some((recovered_nodes, recovered_comments)) =
                    parse_modern_collapsed_comment_tag_sequence(source, child)
                {
                    pending_script_comment = None;
                    fragment_nodes.extend(recovered_nodes);
                    root_comments.extend(recovered_comments);
                    previous_child_end = Some(child_end);
                    continue;
                }

                if let Some(name) = modern_element_name(source, child) {
                    match classify_element_name(name.as_ref()) {
                        ElementKind::Script => {
                            if let Some(script) = parse_modern_script(
                                source,
                                child,
                                pending_script_comment.as_deref(),
                            ) {
                                js.push(script.clone());
                                match script.context {
                                    crate::ast::modern::ScriptContext::Module => {
                                        if module.is_none() {
                                            module = Some(script);
                                        }
                                    }
                                    crate::ast::modern::ScriptContext::Default => {
                                        if instance.is_none() {
                                            instance = Some(script);
                                        }
                                    }
                                }
                                pending_script_comment = None;
                                previous_child_end = Some(child_end);
                                continue;
                            }
                        }
                        ElementKind::Svelte(SvelteElementKind::Options) => {
                            options = parse_modern_options(source, child);
                            pending_script_comment = None;
                            previous_child_end = Some(child_end);
                            continue;
                        }
                        ElementKind::Style => {
                            if let Some(style) = parse_modern_style(source, child) {
                                if css.is_none() {
                                    css = Some(style.clone());
                                }
                                styles.push(style);
                                pending_script_comment = None;
                                previous_child_end = Some(child_end);
                                continue;
                            }
                        }
                        _ => {}
                    }
                }

                pending_script_comment = None;
                let child_hint = hint.as_ref().and_then(|h| {
                    make_child_hint(h, &mut old_node_cursor, child_start, child_end, "element")
                });
                fragment_nodes.push(parse_modern_element_node(
                    source, child, false, false, loose, child_hint.as_ref(),
                ));
            }
            "ERROR" => {
                pending_script_comment = None;
                let mut recovered = recover_modern_error_nodes(source, child, false);
                fragment_nodes.append(&mut recovered);
            }
            _ => {}
        }

        previous_child_end = Some(child_end);
    }

    root_comments.extend(collect_modern_tag_comments(source, root));
    root_comments.sort_by_key(|comment| {
        (
            comment.start,
            comment.end,
            match comment.r#type {
                RootCommentType::Line => 0u8,
                RootCommentType::Block => 1u8,
            },
        )
    });
    root_comments.dedup_by(|left, right| {
        left.start == right.start
            && left.end == right.end
            && left.r#type == right.r#type
            && left.value == right.value
    });

    Root {
        css,
        styles: styles.into_boxed_slice(),
        js: Box::new([]),
        scripts: js.into_boxed_slice(),
        start: root.start_byte(),
        end: root.end_byte(),
        r#type: RootType::Root,
        fragment: crate::ast::modern::Fragment {
            r#type: crate::ast::modern::FragmentType::Fragment,
            nodes: fragment_nodes.into_boxed_slice(),
        },
        options,
        module,
        instance,
        comments: (!root_comments.is_empty()).then(|| root_comments.into_boxed_slice()),
        errors: errors.into_boxed_slice(),
    }
}



fn parse_modern_text(source: &str, node: TsNode<'_>) -> Text {
    let raw = text_for_node(source, node);
    let data = Arc::from(decode_html_entities_cow(raw.as_ref()).into_owned());

    Text {
        start: node.start_byte(),
        end: node.end_byte(),
        raw,
        data,
    }
}

pub(crate) fn parse_modern_comment(source: &str, node: TsNode<'_>) -> Comment {
    let raw = text_for_node(source, node);
    let data_raw: &str = raw.as_ref();
    let data_raw: &str = if let Some(tail) = data_raw.strip_prefix("<!--") {
        tail.strip_suffix("-->").unwrap_or(tail)
    } else {
        data_raw
    };
    let data: Arc<str> = Arc::from(data_raw);

    Comment {
        start: node.start_byte(),
        end: node.end_byte(),
        data,
    }
}

pub(crate) fn push_modern_text_node(nodes: &mut Vec<Node>, text: Text) {
    if let Some(Node::Text(last)) = nodes.last_mut()
        && last.end == text.start
    {
        let merged_raw = format!("{}{}", last.raw, text.raw);
        let merged_data = format!("{}{}", last.data, text.data);
        last.end = text.end;
        last.raw = Arc::from(merged_raw);
        last.data = Arc::from(merged_data);
        return;
    }

    nodes.push(Node::Text(text));
}

pub(super) fn parse_modern_script(
    source: &str,
    element: TsNode<'_>,
    _leading_comment: Option<&str>,
) -> Option<Script> {
    let start_tag = find_first_named_child(element, "start_tag")?;
    let end_tag = find_first_named_child(element, "end_tag")?;
    let attributes = parse_modern_attributes(source, start_tag, false);

    let context = attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Attribute(NamedAttribute { name, value, .. })
                if name.as_ref() == "module" =>
            {
                Some(ScriptContext::Module)
            }
            Attribute::Attribute(NamedAttribute { name, value, .. })
                if name.as_ref() == "context" && modern_attribute_value_is_module(value) =>
            {
                Some(ScriptContext::Module)
            }
            _ => None,
        })
        .unwrap_or(ScriptContext::Default);

    let is_ts = attributes.iter().any(|attribute| {
        matches!(
            attribute,
            Attribute::Attribute(NamedAttribute { name, value, .. })
                if name.as_ref() == "lang"
                    && matches!(
                        value,
                        AttributeValueKind::Values(values)
                            if matches!(
                                values.first(),
                                Some(AttributeValue::Text(Text { data, .. }))
                                    if data.as_ref() == "ts"
                            )
                    )
        )
    });

    let content_start = start_tag.end_byte();
    let content_end = end_tag.start_byte();
    let content_source = source.get(content_start..content_end).unwrap_or_default();
    let content = crate::parse::parse_modern_program_content_with_offsets(
        content_source,
        content_start,
        start_tag.start_position().row + 1,
        0,
        end_tag.end_position().row + 1,
        end_tag.end_position().column,
        is_ts,
    )
    .unwrap_or_else(|| crate::parse::ParsedProgramContent {
        parsed: Arc::new(crate::js::JsProgram::parse(
            content_source,
            if is_ts {
                oxc_span::SourceType::ts().with_module(true)
            } else {
                oxc_span::SourceType::mjs()
            },
        )),
    });

    let content_json = Some(Arc::from(content.parsed.to_estree_json(
        source,
        content_start,
        element.end_byte(),
    )));

    Some(Script {
        r#type: ScriptType::Script,
        start: element.start_byte(),
        end: element.end_byte(),
        content_start,
        content_end,
        context,
        content: content.parsed,
        content_json,
        attributes: attributes.into_boxed_slice(),
    })
}

pub(super) fn parse_modern_options(source: &str, element: TsNode<'_>) -> Option<Options> {
    let tag_node = find_first_named_child(element, "self_closing_tag")
        .or_else(|| find_first_named_child(element, "start_tag"))?;
    let attributes = parse_modern_attributes(source, tag_node, false);
    let fragment = parse_modern_options_fragment(source, element);

    let mut custom_element = None;
    let mut runes = None;

    for attribute in &attributes {
        if let Attribute::Attribute(NamedAttribute {
            name,
            value,
            value_syntax,
            ..
        }) = attribute
        {
            if name.as_ref() == "customElement"
                && let AttributeValueKind::Values(values) = value
                && let Some(AttributeValue::Text(Text { data, .. })) = values.first()
            {
                custom_element = Some(CustomElement { tag: data.clone() });
            }

            if name.as_ref() == "runes" {
                match value_syntax {
                    AttributeValueSyntax::Boolean => runes = Some(true),
                    _ if matches!(value, AttributeValueKind::ExpressionTag(_)) => {
                        let AttributeValueKind::ExpressionTag(tag) = value else {
                            unreachable!("checked expression tag");
                        };
                        if tag.expression.literal_bool().is_some() {
                            runes = tag.expression.literal_bool();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Some(Options {
        start: element.start_byte(),
        end: element.end_byte(),
        attributes: attributes.into_boxed_slice(),
        fragment,
        custom_element,
        runes,
    })
}

fn parse_modern_options_fragment(source: &str, element: TsNode<'_>) -> Fragment {
    let mut nodes = Vec::new();
    let mut cursor = element.walk();

    for child in element.named_children(&mut cursor) {
        match child.kind() {
            "start_tag" | "self_closing_tag" | "end_tag" => {}
            "text" | "entity" | "raw_text" => {
                push_modern_text_node(&mut nodes, parse_modern_text(source, child));
            }
            "comment" => nodes.push(Node::Comment(parse_modern_comment(source, child))),
            "expression" => {
                if let Some(tag) = parse_modern_expression_tag(source, child) {
                    nodes.push(Node::ExpressionTag(tag));
                }
            }
            kind if is_typed_tag_kind(kind) => {
                if let Some(tag) = parse_modern_tag(source, child) {
                    nodes.push(tag);
                }
            }
            kind if is_typed_block_kind(kind) => {
                if let Some(block) = parse_modern_block(source, child, None) {
                    nodes.push(block);
                }
            }
            "element" => nodes.push(parse_modern_element_node(
                source, child, false, false, false, None,
            )),
            "ERROR" => {
                let mut recovered = recover_modern_error_nodes(source, child, false);
                nodes.append(&mut recovered);
            }
            _ => {}
        }
    }

    Fragment {
        r#type: FragmentType::Fragment,
        nodes: nodes.into_boxed_slice(),
    }
}

pub(super) fn parse_modern_style(source: &str, element: TsNode<'_>) -> Option<Css> {
    let start_tag = find_first_named_child(element, "start_tag")?;
    let end_tag = find_first_named_child(element, "end_tag");
    let content_start = start_tag.end_byte();
    let content_end = end_tag
        .map(|node: TsNode<'_>| node.start_byte())
        .unwrap_or(element.end_byte());
    let attributes = parse_modern_attributes(source, start_tag, false).into_boxed_slice();

    let children = crate::parse::parse_modern_css_nodes(source, content_start, content_end);

    Some(Css {
        r#type: CssType::StyleSheet,
        start: element.start_byte(),
        end: element.end_byte(),
        attributes,
        children: children.into_boxed_slice(),
        content: CssContent {
            start: content_start,
            end: content_end,
            styles: Arc::from(source.get(content_start..content_end).unwrap_or_default()),
            comment: None,
        },
    })
}

fn modern_attribute_value_is_module(value: &AttributeValueKind) -> bool {
    match value {
        AttributeValueKind::Boolean(_) => false,
        AttributeValueKind::Values(values) => values.iter().any(|value| {
            matches!(
                value,
                AttributeValue::Text(Text { data, .. }) if data.as_ref() == "module"
            )
        }),
        AttributeValueKind::ExpressionTag(tag) => {
            tag.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == "module")
                || tag.expression.literal_string()
                    .is_some_and(|value| value.as_ref() == "module")
        }
    }
}

pub(super) fn modern_element_name(source: &str, element: TsNode<'_>) -> Option<Arc<str>> {
    let mut cursor = element.walk();
    for child in element.named_children(&mut cursor) {
        match child.kind() {
            "start_tag" | "self_closing_tag" => {
                if let Some(tag_name) = find_first_named_child(child, "tag_name") {
                    return Some(text_for_node(source, tag_name));
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn recover_modern_error_nodes(
    source: &str,
    error_node: TsNode<'_>,
    in_shadowroot_template: bool,
) -> Vec<crate::ast::modern::Node> {
    // Try splitting the ERROR into multiple snippet blocks first
    let multi_snippets = recover_multiple_snippet_blocks(source, error_node);
    if !multi_snippets.is_empty() {
        return multi_snippets;
    }
    if let Some(block) = recover_malformed_snippet_block(source, error_node) {
        return vec![crate::ast::modern::Node::SnippetBlock(block)];
    }
    let error_children = named_children_vec(error_node);
    parse_modern_nodes_slice(source, &error_children, in_shadowroot_template)
}

fn parse_modern_collapsed_comment_tag_sequence(
    source: &str,
    node: TsNode<'_>,
) -> Option<(Vec<crate::ast::modern::Node>, Vec<RootComment>)> {
    if node.kind() != "element" {
        return None;
    }

    let start_tag = find_first_named_child(node, "start_tag")?;
    if start_tag.start_byte() != node.start_byte() || start_tag.end_byte() != node.end_byte() {
        return None;
    }

    let raw = text_for_node(source, start_tag);
    let raw_ref = raw.as_ref();
    if !(raw_ref.contains("//") || raw_ref.contains("/*")) || !raw_ref.contains("</") {
        return None;
    }

    parse_collapsed_tag_sequence_from_text(source, node.start_byte(), raw_ref)
}

fn parse_collapsed_tag_sequence_from_text(
    source: &str,
    base: usize,
    raw: &str,
) -> Option<(Vec<crate::ast::modern::Node>, Vec<RootComment>)> {
    let bytes = raw.as_bytes();
    let mut index = 0usize;
    let mut nodes = Vec::new();
    let mut comments = Vec::new();

    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            let ws_start = index;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index > ws_start {
                push_modern_text_node(
                    &mut nodes,
                    Text {
                        start: base + ws_start,
                        end: base + index,
                        raw: Arc::from(&raw[ws_start..index]),
                        data: Arc::from(&raw[ws_start..index]),
                    },
                );
            }
            continue;
        }

        if bytes.get(index) != Some(&b'<') {
            let text_start = index;
            while index < bytes.len() && bytes[index] != b'<' {
                index += 1;
            }
            push_modern_text_node(
                &mut nodes,
                Text {
                    start: base + text_start,
                    end: base + index,
                    raw: Arc::from(&raw[text_start..index]),
                    data: Arc::from(&raw[text_start..index]),
                },
            );
            continue;
        }

        let tag_start = index;
        index += 1;
        if bytes.get(index) == Some(&b'/') {
            break;
        }

        let name_start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'-'
                || bytes[index] == b':')
        {
            index += 1;
        }
        if index == name_start {
            return None;
        }
        let name = &raw[name_start..index];

        let mut attributes = Vec::new();
        loop {
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index >= bytes.len() {
                return None;
            }
            if bytes[index] == b'>' {
                index += 1;
                break;
            }

            if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
                let comment_start = index;
                index += 2;
                let value_start = index;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                let comment_end = index;
                comments.push(modern_root_comment(
                    source,
                    RootCommentType::Line,
                    base + comment_start,
                    base + comment_end,
                    Arc::from(&raw[value_start..comment_end]),
                ));
                continue;
            }

            if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
                let comment_start = index;
                index += 2;
                let value_start = index;
                let tail = &raw[index..];
                let rel_end = tail.find("*/")?;
                let value_end = index + rel_end;
                index = value_end + 2;
                comments.push(modern_root_comment(
                    source,
                    RootCommentType::Block,
                    base + comment_start,
                    base + index,
                    Arc::from(&raw[value_start..value_end]),
                ));
                continue;
            }

            let attr_start = index;
            while index < bytes.len()
                && !bytes[index].is_ascii_whitespace()
                && bytes[index] != b'='
                && bytes[index] != b'>'
            {
                index += 1;
            }
            if index == attr_start {
                return None;
            }
            let attr_name = &raw[attr_start..index];
            let name_loc = modern_name_location(source, base + attr_start, base + index);

            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }

            let value = if bytes.get(index) == Some(&b'=') {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }

                if let Some(quote) = bytes
                    .get(index)
                    .copied()
                    .filter(|q| *q == b'"' || *q == b'\'')
                {
                    index += 1;
                    let value_start = index;
                    while index < bytes.len() && bytes[index] != quote {
                        index += 1;
                    }
                    let value_end = index;
                    if bytes.get(index) == Some(&quote) {
                        index += 1;
                    }

                    AttributeValueKind::Values(
                        vec![AttributeValue::Text(Text {
                            start: base + value_start,
                            end: base + value_end,
                            raw: Arc::from(&raw[value_start..value_end]),
                            data: Arc::from(&raw[value_start..value_end]),
                        })]
                        .into_boxed_slice(),
                    )
                } else {
                    AttributeValueKind::Boolean(true)
                }
            } else {
                AttributeValueKind::Boolean(true)
            };
            let value_syntax = match &value {
                AttributeValueKind::Boolean(_) => AttributeValueSyntax::Boolean,
                AttributeValueKind::Values(_) | AttributeValueKind::ExpressionTag(_) => {
                    AttributeValueSyntax::Quoted
                }
            };

            attributes.push(Attribute::Attribute(NamedAttribute {
                start: base + attr_start,
                end: base + index,
                name: Arc::from(attr_name),
                name_loc,
                value,
                value_syntax,
                error: None,
            }));
        }

        let close_tag = format!("</{name}>");
        let close_rel = raw[index..].find(&close_tag)?;
        let close_start = index + close_rel;
        let close_end = close_start + close_tag.len();

        let name_loc =
            modern_name_location(source, base + name_start, base + name_start + name.len());
        nodes.push(crate::ast::modern::Node::RegularElement(RegularElement {
            start: base + tag_start,
            end: base + close_end,
            name: Arc::from(name),
            name_loc,
            self_closing: false,
            has_end_tag: true,
            attributes: attributes.into_boxed_slice(),
            fragment: crate::ast::modern::Fragment {
                r#type: crate::ast::modern::FragmentType::Fragment,
                nodes: Box::new([]),
            },
        }));

        index = close_end;
    }

    Some((nodes, comments))
}

pub(super) fn modern_name_location(source: &str, start: usize, end: usize) -> SourceRange {
    SourceRange {
        start: location_at_offset(source, start),
        end: location_at_offset(source, end),
    }
}

pub(super) fn modern_root_comment(
    source: &str,
    kind: RootCommentType,
    start: usize,
    end: usize,
    value: Arc<str>,
) -> RootComment {
    RootComment {
        r#type: kind,
        start,
        end,
        value,
        loc: SourceRange {
            start: location_at_offset(source, start),
            end: location_at_offset(source, end),
        },
    }
}

fn collect_modern_tag_comments(source: &str, root: TsNode<'_>) -> Vec<RootComment> {
    let mut out = Vec::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "tag_comment"
                && let Some(comment) = parse_modern_tag_comment(source, child)
            {
                out.push(comment);
            }
            stack.push(child);
        }
    }

    out
}

fn parse_modern_tag_comment(source: &str, node: TsNode<'_>) -> Option<RootComment> {
    let raw = text_for_node(source, node);
    let raw_ref = raw.as_ref();

    if let Some(value) = raw_ref.strip_prefix("//") {
        return Some(modern_root_comment(
            source,
            RootCommentType::Line,
            node.start_byte(),
            node.end_byte(),
            Arc::from(value),
        ));
    }

    if let Some(tail) = raw_ref.strip_prefix("/*")
        && let Some(inner) = tail.strip_suffix("*/")
    {
        return Some(modern_root_comment(
            source,
            RootCommentType::Block,
            node.start_byte(),
            node.end_byte(),
            Arc::from(inner),
        ));
    }

    None
}

fn parse_modern_block(
    source: &str,
    block: TsNode<'_>,
    _hint: Option<&IncrementalHint<'_>>,
) -> Option<Node> {
    // TODO: Thread hint into individual block parsers for fragment reuse.
    match block.kind() {
        "if_block" => parse_modern_if_block(source, block).map(Node::IfBlock),
        "each_block" => parse_modern_each_block(source, block).map(Node::EachBlock),
        "key_block" => parse_modern_key_block(source, block).map(Node::KeyBlock),
        "await_block" => parse_modern_await_block(source, block).map(Node::AwaitBlock),
        "snippet_block" => parse_modern_snippet_block(source, block).map(Node::SnippetBlock),
        _ => None,
    }
}

fn parse_modern_tag(source: &str, tag: TsNode<'_>) -> Option<Node> {
    match tag.kind() {
        "render_tag" => Some(Node::RenderTag(RenderTag {
            start: tag.start_byte(),
            end: tag.end_byte(),
            expression: parse_special_tag_expression(source, tag)?,
        })),
        "html_tag" => Some(Node::HtmlTag(HtmlTag {
            start: tag.start_byte(),
            end: tag.end_byte(),
            expression: parse_special_tag_expression(source, tag)?,
        })),
        "const_tag" => Some(Node::ConstTag(ConstTag {
            start: tag.start_byte(),
            end: tag.end_byte(),
            declaration: parse_const_tag_declaration(source, tag)
                .or_else(|| parse_special_tag_expression(source, tag))?,
        })),
        "debug_tag" => {
            let arguments = parse_modern_debug_tag_arguments(source, tag);
            let identifiers = debug_tag_identifiers(&arguments);
            Some(Node::DebugTag(DebugTag {
                start: tag.start_byte(),
                end: tag.end_byte(),
                arguments,
                identifiers,
            }))
        }
        _ => None,
    }
}

fn special_tag_expression_node(tag: TsNode<'_>) -> Option<TsNode<'_>> {
    find_first_named_child(tag, "expression_value")
        .or_else(|| find_first_named_child(tag, "expression"))
}

fn parse_special_tag_expression(source: &str, tag: TsNode<'_>) -> Option<Expression> {
    special_tag_expression_node(tag).and_then(|node| parse_modern_expression_field(source, node))
}

fn parse_const_tag_declaration(source: &str, tag: TsNode<'_>) -> Option<Expression> {
    if tag.kind() != "const_tag" || tag.end_byte() <= tag.start_byte() + 3 {
        return None;
    }

    let declaration_source = source.get(tag.start_byte() + 2..tag.end_byte().saturating_sub(1))?;
    let program = crate::parse::parse_modern_program_content_with_offsets(
        declaration_source,
        tag.start_byte() + 2,
        tag.start_position().row + 1,
        tag.start_position().column + 2,
        tag.end_position().row + 1,
        tag.end_position().column.saturating_sub(1),
        true,
    )?;

    let [declaration] = program.parsed.program().body.as_slice() else {
        return None;
    };

    let span = declaration.span();
    Some(Expression::from_statement(
        program.parsed,
        0,
        tag.start_byte() + 2 + span.start as usize,
        tag.start_byte() + 2 + span.end as usize,
    ))
}

fn parse_modern_debug_tag_arguments(source: &str, tag: TsNode<'_>) -> Box<[Expression]> {
    let expr_node = special_tag_expression_node(tag);
    let Some(expr_node) = expr_node else {
        return Box::new([]);
    };

    parse_modern_expression_field(source, expr_node)
        .map(split_debug_tag_arguments)
        .unwrap_or_default()
}

pub(crate) fn split_debug_tag_arguments(expression: Expression) -> Box<[Expression]> {
    crate::parse::oxc_query::split_debug_tag_arguments(expression)
}

fn debug_tag_identifiers(arguments: &[Expression]) -> Box<[Identifier]> {
    arguments
        .iter()
        .filter_map(|argument| modern_identifier_from_expression(argument.clone()))
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn modern_identifier_from_expression(expression: Expression) -> Option<Identifier> {
    let name = expression.identifier_name()?;
    Some(Identifier {
        start: expression.start,
        end: expression.end,
        loc: None,
        name,
    })
}

fn parse_modern_if_block(source: &str, block: TsNode<'_>) -> Option<IfBlock> {
    let children = named_children_vec(block);

    let test_expr = block
        .child_by_field_name("expression")
        .map(|node| parse_modern_expression_field_or_empty(source, node))
        .unwrap_or_else(|| modern_empty_identifier_at_block_tag_end(block));

    let end_idx = children
        .iter()
        .rposition(|c| c.kind() == "block_end")
        .unwrap_or(children.len());
    let body_start = body_start_index(block, &children, &["expression"]);
    let branch_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            matches!(node.kind(), "else_if_clause" | "else_clause").then_some(idx)
        })
        .collect();

    let consequent_end = branch_indices.first().copied().unwrap_or(end_idx);
    let consequent = Fragment {
        r#type: FragmentType::Fragment,
        nodes: parse_modern_nodes_slice(source, &children[body_start..consequent_end], false)
            .into_boxed_slice(),
    };

    let alternate = if branch_indices.is_empty() {
        None
    } else {
        parse_modern_alternate(source, &children, &branch_indices, 0, end_idx).map(Box::new)
    };

    Some(IfBlock {
        elseif: false,
        start: block.start_byte(),
        end: block.end_byte(),
        test: test_expr,
        consequent,
        alternate,
    })
}

fn parse_modern_each_block(source: &str, block: TsNode<'_>) -> Option<EachBlock> {
    let children = named_children_vec(block);
    let end_idx = children
        .iter()
        .rposition(|c| c.kind() == "block_end")
        .unwrap_or(children.len());
    let has_as_clause = cst_node_has_direct_token(block, "as");

    let mut expression = block
        .child_by_field_name("expression")
        .map(|node| parse_modern_expression_field_or_empty(source, node))
        .unwrap_or_else(|| modern_empty_identifier_at_block_tag_end(block));

    let (context, context_error) = block
        .child_by_field_name("binding")
        .map(|node| parse_modern_binding_field_with_error(source, node, true))
        .unwrap_or((None, None));

    let mut index = block
        .child_by_field_name("index")
        .map(|node| text_for_node(source, node).trim().to_string())
        .filter(|text| !text.is_empty())
        .map(Arc::<str>::from);

    let mut key = block
        .child_by_field_name("key")
        .map(|node| parse_modern_expression_field_or_empty(source, node));

    let mut invalid_key_without_as = false;
    if !has_as_clause
        && context.is_none()
        && key.is_none()
        && let Some(expression_field) = block.child_by_field_name("expression")
        && let Some(recovered) = recover_each_header_without_as_key(source, expression_field)
    {
        expression = recovered.expression;
        index = recovered.index;
        key = Some(recovered.key);
        invalid_key_without_as = true;
    }

    let body_start = body_start_index(block, &children, &["expression", "binding", "index", "key"]);
    let branch_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| (node.kind() == "else_clause").then_some(idx))
        .collect();

    let body_end = branch_indices.first().copied().unwrap_or(end_idx);
    let body_nodes = parse_modern_nodes_slice(source, &children[body_start..body_end], false);
    let fallback = branch_indices.iter().find_map(|branch_index| {
        let branch = *children.get(*branch_index)?;
        if branch.kind() != "else_clause" {
            return None;
        }
        let body_nodes = else_clause_body_nodes(branch);
        Some(Fragment {
            r#type: FragmentType::Fragment,
            nodes: parse_modern_nodes_slice(source, &body_nodes, false).into_boxed_slice(),
        })
    });

    Some(EachBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        body: Fragment {
            r#type: FragmentType::Fragment,
            nodes: body_nodes.into_boxed_slice(),
        },
        has_as_clause,
        invalid_key_without_as,
        context,
        context_error,
        index,
        key,
        fallback,
    })
}

struct EachHeaderMissingAsRecovery {
    expression: Expression,
    index: Option<Arc<str>>,
    key: Expression,
}

fn recover_each_header_without_as_key(
    source: &str,
    expression_field: TsNode<'_>,
) -> Option<EachHeaderMissingAsRecovery> {
    let raw = expression_field.utf8_text(source.as_bytes()).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let field_abs = expression_field.start_byte() + raw.find(trimmed).unwrap_or(0);
    let segments = split_top_level_commas(trimmed);
    if segments.len() < 2 {
        return None;
    }

    let expression_segment = segments.first()?.0.trim();
    if expression_segment.is_empty() {
        return None;
    }
    let expression_abs = field_abs + trimmed.find(expression_segment).unwrap_or(0);
    let (expression_line, expression_col) = line_column_at_offset(source, expression_abs);
    let expression = parse_modern_expression_from_text(
        expression_segment,
        expression_abs,
        expression_line,
        expression_col,
    )?;

    let tail_offset = segments.get(1)?.1;
    let tail = trimmed.get(tail_offset..)?.trim();
    let tail_abs = field_abs + tail_offset + trimmed.get(tail_offset..)?.find(tail).unwrap_or(0);
    let (binding_raw, key_raw, key_inner_offset) = split_trailing_parenthesized_group(tail)?;

    let binding = binding_raw.trim();
    if binding.is_empty() || parse_identifier_name(binding).is_none() {
        return None;
    }
    let index = Some(Arc::<str>::from(binding));

    let key_expression = key_raw.trim();
    if key_expression.is_empty() {
        return None;
    }
    let key_abs = tail_abs + key_inner_offset + key_raw.find(key_expression).unwrap_or(0);
    let (key_line, key_col) = line_column_at_offset(source, key_abs);
    let key = parse_modern_expression_from_text(key_expression, key_abs, key_line, key_col)?;

    Some(EachHeaderMissingAsRecovery {
        expression,
        index,
        key,
    })
}

fn split_trailing_parenthesized_group(text: &str) -> Option<(&str, &str, usize)> {
    let trimmed = text.trim_end();
    if !trimmed.ends_with(')') {
        return None;
    }

    let mut depth = 0usize;
    for (idx, ch) in trimmed.char_indices().rev() {
        match ch {
            ')' => depth += 1,
            '(' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let before = &trimmed[..idx];
                    let inner_start = idx + ch.len_utf8();
                    let inner = &trimmed[inner_start..trimmed.len() - 1];
                    return Some((before, inner, inner_start));
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_modern_key_block(source: &str, block: TsNode<'_>) -> Option<KeyBlock> {
    let children = named_children_vec(block);
    let end_idx = children
        .iter()
        .rposition(|c| c.kind() == "block_end")
        .unwrap_or(children.len());

    let expression = block
        .child_by_field_name("expression")
        .and_then(|node| parse_modern_expression_field(source, node))?;
    let body_start = body_start_index(block, &children, &["expression"]);
    let fragment = Fragment {
        r#type: FragmentType::Fragment,
        nodes: parse_modern_nodes_slice(source, &children[body_start..end_idx], false)
            .into_boxed_slice(),
    };

    Some(KeyBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        fragment,
    })
}

fn parse_modern_await_block(source: &str, block: TsNode<'_>) -> Option<AwaitBlock> {
    let children = named_children_vec(block);
    let end_idx = children
        .iter()
        .rposition(|c| c.kind() == "block_end")
        .unwrap_or(children.len());

    // Detect shorthand: {#await expr then v}...{/await}
    let inline_kind = find_first_named_child(block, "shorthand_kind")
        .and_then(|node| node.utf8_text(source.as_bytes()).ok())
        .map(str::trim)
        .and_then(BlockBranchKind::parse_await_shorthand)
        .or_else(|| {
            if cst_node_has_direct_token(block, "then") {
                Some(BlockBranchKind::Then)
            } else if cst_node_has_direct_token(block, "catch") {
                Some(BlockBranchKind::Catch)
            } else {
                None
            }
        });

    let inline_binding_field = block
        .child_by_field_name("binding")
        .and_then(|node| parse_modern_binding_field(source, node, true));
    let expression = block
        .child_by_field_name("expression")
        .map(|node| parse_modern_expression_field_or_empty(source, node))
        .unwrap_or_else(|| modern_empty_identifier_at_block_tag_end(block));

    let branch_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| (node.kind() == "await_branch").then_some(idx))
        .collect();
    let first_branch_idx = branch_indices.first().copied().unwrap_or(end_idx);

    let parse_await_children_field = |node: TsNode<'_>| -> Vec<crate::ast::modern::Node> {
        let child_nodes = named_children_vec(node);
        parse_modern_nodes_slice(source, &child_nodes, false)
    };

    let pending = if inline_kind.is_some() {
        None
    } else {
        let mut pending_nodes = Vec::new();

        if let Some(pending_node) = block
            .child_by_field_name("pending")
            .filter(|node| node.kind() == "await_pending")
        {
            pending_nodes.extend(parse_await_children_field(pending_node));
        }

        let body_start = body_start_index(block, &children, &["expression", "binding", "pending"]);
        for node in &children[body_start..first_branch_idx] {
            if node.kind() == "await_pending" {
                continue;
            }
            let mut recovered = parse_modern_nodes_slice(source, std::slice::from_ref(node), false);
            if recovered.is_empty()
                && node.kind() == "ERROR"
                && let Some(text) = recover_await_error_pending_text(source, *node)
            {
                push_modern_text_node(&mut recovered, text);
            }
            pending_nodes.extend(recovered);
        }

        (branch_indices.is_empty() || !pending_nodes.is_empty()).then_some(Fragment {
            r#type: FragmentType::Fragment,
            nodes: pending_nodes.into_boxed_slice(),
        })
    };

    let inline_binding = inline_binding_field;
    let mut value = None;
    let mut error = None;
    let mut then_fragment = None;
    let mut catch_fragment = None;

    match inline_kind {
        Some(BlockBranchKind::Then) => value = inline_binding,
        Some(BlockBranchKind::Catch) => error = inline_binding,
        _ => {}
    }

    if let Some(inline_branch_kind) = inline_kind {
        let inline_nodes = find_first_named_child(block, "await_branch_children")
            .map(parse_await_children_field)
            .unwrap_or_default();

        let fragment = Fragment {
            r#type: FragmentType::Fragment,
            nodes: inline_nodes.into_boxed_slice(),
        };

        match inline_branch_kind {
            BlockBranchKind::Then => then_fragment = Some(fragment),
            BlockBranchKind::Catch => catch_fragment = Some(fragment),
            _ => {}
        }
    }

    for branch_child_idx in branch_indices.iter().copied() {
        let branch_node = *children.get(branch_child_idx)?;

        let kind = find_first_named_child(branch_node, "branch_kind")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .and_then(|s| BlockBranchKind::parse_await_shorthand(s.trim()));
        let Some(kind) = kind else {
            continue;
        };

        let binding_expr = branch_node
            .child_by_field_name("binding")
            .and_then(|node| parse_modern_binding_field(source, node, true));

        let fragment_nodes = find_first_named_child(branch_node, "await_branch_children")
            .map(parse_await_children_field)
            .unwrap_or_default();
        let fragment = Fragment {
            r#type: FragmentType::Fragment,
            nodes: fragment_nodes.into_boxed_slice(),
        };

        match kind {
            BlockBranchKind::Then => {
                if value.is_none() {
                    value = binding_expr;
                }
                then_fragment = Some(fragment);
            }
            BlockBranchKind::Catch => {
                if error.is_none() {
                    error = binding_expr;
                }
                catch_fragment = Some(fragment);
            }
            _ => {}
        }
    }

    Some(AwaitBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        value,
        error,
        pending,
        then: then_fragment,
        catch: catch_fragment,
    })
}

fn recover_await_error_pending_text(source: &str, error_node: TsNode<'_>) -> Option<Text> {
    let start = error_node.start_byte();
    let end = error_node.end_byte();
    if start >= end || end > source.len() {
        return None;
    }

    let raw = &source[start..end];
    let close = raw.find('}')?;
    let tail = raw
        .get((close + 1)..)?
        .trim_start_matches(char::is_whitespace);
    if tail.is_empty() {
        return None;
    }

    let tail_start = start + close + 1 + (raw[(close + 1)..].len() - tail.len());
    Some(Text {
        start: tail_start,
        end,
        raw: Arc::from(tail),
        data: Arc::from(decode_html_entities_cow(tail).into_owned()),
    })
}


fn parse_modern_element_node(
    source: &str,
    node: TsNode<'_>,
    in_shadowroot_template: bool,
    in_svelte_head: bool,
    loose: bool,
    hint: Option<&IncrementalHint<'_>>,
) -> Node {
    let mut tag_cursor = node.walk();
    let mut start_tag: Option<TsNode<'_>> = None;
    let mut end_tag: Option<TsNode<'_>> = None;
    let mut self_closing_tag: Option<TsNode<'_>> = None;
    let mut trailing_text: Option<TsNode<'_>> = None;
    for child in node.named_children(&mut tag_cursor) {
        match child.kind() {
            "start_tag" => start_tag = Some(child),
            "end_tag" => end_tag = Some(child),
            "self_closing_tag" => self_closing_tag = Some(child),
            "text" if trailing_text.is_none() => {
                trailing_text = Some(child);
            }
            _ => {}
        }
    }

    if let (Some(start_tag_node), Some(text_node)) = (start_tag, trailing_text)
        && text_node.start_byte() != start_tag_node.end_byte()
    {
        trailing_text = None;
    }

    if let Some(start_tag) = start_tag
        && end_tag.is_none()
        && self_closing_tag.is_none()
        && !text_for_node(source, start_tag).trim_end().ends_with('>')
    {
        return parse_modern_loose_start_tag_node(source, start_tag, trailing_text);
    }

    let element =
        parse_modern_regular_element(source, node, in_shadowroot_template, in_svelte_head, loose, hint);
    classify_modern_element(element, in_shadowroot_template, in_svelte_head)
}

/// Extract the `this={expr}` attribute's expression from an attribute list,
/// returning the expression and the remaining attributes with `this` removed.
fn extract_this_expression(attributes: Box<[Attribute]>) -> (Option<Expression>, Box<[Attribute]>) {
    let mut this_expr = None;
    let mut remaining = Vec::with_capacity(attributes.len());

    for attr in Vec::from(attributes) {
        if this_expr.is_none()
            && let Attribute::Attribute(ref named) = attr
            && classify_attribute_name(named.name.as_ref()) == AttributeKind::This
            && let AttributeValueKind::ExpressionTag(ref tag) = named.value
        {
            this_expr = Some(tag.expression.clone());
            continue;
        }
        if this_expr.is_none()
            && let Attribute::Attribute(ref named) = attr
            && classify_attribute_name(named.name.as_ref()) == AttributeKind::This
            && let AttributeValueKind::Values(ref values) = named.value
            && values.len() == 1
            && let AttributeValue::Text(text) = &values[0]
        {
            this_expr = Some(modern_string_literal_expression(
                text.data.clone(),
                text.start,
                text.end,
            ));
            continue;
        }
        remaining.push(attr);
    }

    (this_expr, remaining.into_boxed_slice())
}

fn modern_string_literal_expression(value: Arc<str>, start: usize, end: usize) -> Expression {
    let raw = format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"));
    match crate::js::JsExpression::parse(raw, oxc_span::SourceType::ts().with_module(true)) {
        Ok(parsed) => Expression::from_expression(Arc::new(parsed), start, end),
        Err(_) => Expression::empty(start, end),
    }
}

/// Classify a parsed `RegularElement` into the correct Svelte* node type.
fn classify_modern_element(
    element: RegularElement,
    in_shadowroot_template: bool,
    in_svelte_head: bool,
) -> Node {
    match classify_element_name(element.name.as_ref()) {
        ElementKind::Slot if !in_shadowroot_template => Node::SlotElement(SlotElement {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        ElementKind::Svelte(kind) => classify_svelte_element(element, kind),
        _ if element.name.as_ref() == "title" && in_svelte_head => {
            Node::TitleElement(crate::ast::modern::TitleElement {
                start: element.start,
                end: element.end,
                name: element.name,
                name_loc: element.name_loc,
                attributes: element.attributes,
                fragment: element.fragment,
            })
        }
        _ if is_component_name(element.name.as_ref()) => Node::Component(Component {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        _ => Node::RegularElement(element),
    }
}

fn classify_svelte_element(element: RegularElement, kind: SvelteElementKind) -> Node {
    match kind {
        SvelteElementKind::Head => Node::SvelteHead(crate::ast::modern::SvelteHead {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        SvelteElementKind::Body => Node::SvelteBody(crate::ast::modern::SvelteBody {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        SvelteElementKind::Window => Node::SvelteWindow(crate::ast::modern::SvelteWindow {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        SvelteElementKind::Document => Node::SvelteDocument(crate::ast::modern::SvelteDocument {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        SvelteElementKind::Component => {
            let (expression, attributes) = extract_this_expression(element.attributes);
            Node::SvelteComponent(crate::ast::modern::SvelteComponent {
                start: element.start,
                end: element.end,
                name: element.name,
                name_loc: element.name_loc,
                attributes,
                fragment: element.fragment,
                expression,
            })
        }
        SvelteElementKind::Element => {
            let (expression, attributes) = extract_this_expression(element.attributes);
            Node::SvelteElement(crate::ast::modern::SvelteElement {
                start: element.start,
                end: element.end,
                name: element.name,
                name_loc: element.name_loc,
                attributes,
                fragment: element.fragment,
                expression,
            })
        }
        SvelteElementKind::SelfTag => Node::SvelteSelf(crate::ast::modern::SvelteSelf {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        SvelteElementKind::Fragment => Node::SvelteFragment(crate::ast::modern::SvelteFragment {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        SvelteElementKind::Boundary => Node::SvelteBoundary(crate::ast::modern::SvelteBoundary {
            start: element.start,
            end: element.end,
            name: element.name,
            name_loc: element.name_loc,
            attributes: element.attributes,
            fragment: element.fragment,
        }),
        // Options is handled at the root level, Unknown falls through to RegularElement
        SvelteElementKind::Options | SvelteElementKind::Unknown => Node::RegularElement(element),
    }
}

fn parse_modern_regular_element(
    source: &str,
    node: TsNode<'_>,
    in_shadowroot_template: bool,
    in_svelte_head: bool,
    loose: bool,
    hint: Option<&IncrementalHint<'_>>,
) -> RegularElement {
    let mut cursor = node.walk();
    let mut start_tag: Option<TsNode<'_>> = None;
    let mut end_tag: Option<TsNode<'_>> = None;
    let mut self_closing_tag: Option<TsNode<'_>> = None;

    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "start_tag" => start_tag = Some(child),
            "end_tag" => end_tag = Some(child),
            "self_closing_tag" => self_closing_tag = Some(child),
            _ => {}
        }
    }

    let tag_node = start_tag.or(self_closing_tag);
    let tag_name = tag_node.and_then(|tag| find_first_named_child(tag, "tag_name"));

    let name = tag_name
        .map(|tag_name| text_for_node(source, tag_name))
        .unwrap_or_else(|| Arc::from(""));

    let name_loc = if let Some(tag_name) = tag_name {
        SourceRange {
            start: line_column_from_point(
                source,
                tag_name.start_position(),
                tag_name.start_byte(),
            ),
            end: line_column_from_point(source, tag_name.end_position(), tag_name.end_byte()),
        }
    } else {
        SourceRange {
            start: line_column_from_point(source, node.start_position(), node.start_byte()),
            end: line_column_from_point(source, node.start_position(), node.start_byte()),
        }
    };

    let attributes = tag_node
        .map(|tag| parse_modern_attributes(source, tag, loose))
        .unwrap_or_default();

    let is_shadowroot_template =
        matches!(classify_element_name(name.as_ref()), ElementKind::Template)
            && attributes.iter().any(|attr| {
                matches!(
                    attr,
                    Attribute::Attribute(NamedAttribute { name, .. })
                        if name.as_ref() == "shadowrootmode"
                )
            });

    let children_in_svelte_head = in_svelte_head
        || matches!(
            classify_element_name(name.as_ref()),
            ElementKind::Svelte(SvelteElementKind::Head)
        );

    let mut fragment_nodes = Vec::new();
    let malformed_unclosed_start_tag = start_tag
        .map(|tag| {
            end_tag.is_none()
                && self_closing_tag.is_none()
                && !text_for_node(source, tag).trim_end().ends_with('>')
        })
        .unwrap_or(false);
    let mut old_node_cursor = 0usize;
    let mut inner_cursor = node.walk();
    for child in node.named_children(&mut inner_cursor) {
        if malformed_unclosed_start_tag
            && let Some(tag) = start_tag
            && child.start_byte() >= tag.end_byte()
            && child.kind() != "start_tag"
        {
            continue;
        }

        if start_tag.is_some_and(|tag| tag.has_error())
            && end_tag.is_none()
            && self_closing_tag.is_none()
            && child.kind() == "text"
            && source
                .get(child.start_byte()..child.end_byte())
                .is_some_and(|raw| raw.contains("/>"))
        {
            continue;
        }

        let child_start = child.start_byte();
        let child_end = child.end_byte();

        // Incremental reuse for element children.
        if let Some(hint) = &hint
            && !any_range_overlaps(hint.changed_ranges, child_start, child_end)
            && let Some(reused) = try_reuse_node(
                hint.old_source,
                source,
                hint.old_nodes,
                &mut old_node_cursor,
                child_start,
                child_end,
            )
        {
            fragment_nodes.push(reused);
            continue;
        }

        match child.kind() {
            "start_tag" if Some(child) != start_tag && Some(child) != self_closing_tag => {
                fragment_nodes.push(parse_modern_loose_start_tag_node(source, child, None));
            }
            "end_tag" | "self_closing_tag" => {}
            "text" | "entity" | "raw_text" => {
                push_modern_text_node(&mut fragment_nodes, parse_modern_text(source, child));
            }
            "comment" => fragment_nodes.push(Node::Comment(parse_modern_comment(source, child))),
            "expression" => {
                if let Some(tag) = parse_modern_expression_tag(source, child) {
                    fragment_nodes.push(Node::ExpressionTag(tag));
                }
            }
            "element" => {
                let child_hint = hint.as_ref().and_then(|h| {
                    make_child_hint(h, &mut old_node_cursor, child_start, child_end, "element")
                });
                fragment_nodes.push(parse_modern_element_node(
                    source,
                    child,
                    in_shadowroot_template || is_shadowroot_template,
                    children_in_svelte_head,
                    loose,
                    child_hint.as_ref(),
                ));
            }
            kind if is_typed_block_kind(kind) => {
                let child_hint = hint.as_ref().and_then(|h| {
                    make_child_hint(h, &mut old_node_cursor, child_start, child_end, kind)
                });
                if let Some(block_node) = parse_modern_block(source, child, child_hint.as_ref()) {
                    fragment_nodes.push(block_node);
                }
            }
            kind if is_typed_tag_kind(kind) => {
                if let Some(tag_node) = parse_modern_tag(source, child) {
                    fragment_nodes.push(tag_node);
                }
            }
            "ERROR" => {
                fragment_nodes.extend(recover_modern_error_nodes(
                    source,
                    child,
                    in_shadowroot_template || is_shadowroot_template,
                ));
            }
            _ => {}
        }
    }

    RegularElement {
        start: node.start_byte(),
        end: node.end_byte(),
        name,
        name_loc,
        self_closing: self_closing_tag.is_some(),
        has_end_tag: end_tag.is_some(),
        attributes: attributes.into_boxed_slice(),
        fragment: Fragment {
            r#type: FragmentType::Fragment,
            nodes: fragment_nodes.into_boxed_slice(),
        },
    }
}

fn parse_modern_alternate(
    source: &str,
    children: &[TsNode<'_>],
    branch_indices: &[usize],
    branch_index: usize,
    block_end_idx: usize,
) -> Option<Alternate> {
    let branch_child_idx = *branch_indices.get(branch_index)?;
    let branch = *children.get(branch_child_idx)?;

    match branch.kind() {
        "else_if_clause" => {
            let test = branch
                .child_by_field_name("expression")
                .map(|node| parse_modern_expression_field_or_empty(source, node))
                .unwrap_or_else(|| modern_empty_identifier_at_block_tag_end(branch));
            // In the new grammar, body nodes are children of else_if_clause itself
            let clause_children = named_children_vec(branch);
            let clause_body_start = body_start_index(branch, &clause_children, &["expression"]);
            let consequent = Fragment {
                r#type: FragmentType::Fragment,
                nodes: parse_modern_nodes_slice(
                    source,
                    &clause_children[clause_body_start..],
                    false,
                )
                .into_boxed_slice(),
            };

            let nested_alternate = if branch_index + 1 < branch_indices.len() {
                parse_modern_alternate(
                    source,
                    children,
                    branch_indices,
                    branch_index + 1,
                    block_end_idx,
                )
                .map(Box::new)
            } else {
                None
            };

            let nested_if = IfBlock {
                elseif: true,
                start: branch.start_byte(),
                end: children
                    .get(block_end_idx)
                    .map(|n| n.end_byte())
                    .unwrap_or(branch.end_byte()),
                test,
                consequent,
                alternate: nested_alternate,
            };

            Some(Alternate::Fragment(Fragment {
                r#type: FragmentType::Fragment,
                nodes: vec![Node::IfBlock(nested_if)].into_boxed_slice(),
            }))
        }
        "else_clause" => {
            let body_nodes = else_clause_body_nodes(branch);
            Some(Alternate::Fragment(Fragment {
                r#type: FragmentType::Fragment,
                nodes: parse_modern_nodes_slice(source, &body_nodes, false).into_boxed_slice(),
            }))
        }
        _ => {
            if branch_index + 1 < branch_indices.len() {
                parse_modern_alternate(
                    source,
                    children,
                    branch_indices,
                    branch_index + 1,
                    block_end_idx,
                )
            } else {
                None
            }
        }
    }
}

fn parse_modern_nodes_slice(
    source: &str,
    nodes: &[TsNode<'_>],
    in_shadowroot_template: bool,
) -> Vec<Node> {
    let mut out = Vec::new();
    let mut previous_end = None;

    let mut index = 0usize;
    while index < nodes.len() {
        let node = nodes[index];
        if let Some(gap_start) = previous_end {
            push_modern_gap_text(source, &mut out, gap_start, node.start_byte());
        }

        match node.kind() {
            "text" | "entity" => push_modern_text_node(&mut out, parse_modern_text(source, node)),
            "comment" => out.push(Node::Comment(parse_modern_comment(source, node))),
            "expression" => {
                if let Some(tag) = parse_modern_expression_tag(source, node) {
                    out.push(Node::ExpressionTag(tag));
                }
            }
            "element" => out.push(parse_modern_element_node(
                source,
                node,
                in_shadowroot_template,
                false,
                false,
                None,
            )),
            "start_tag" => {
                if let Some(name) = start_end_tag_name(source, node)
                    && let Some(close_index) =
                        find_matching_loose_end_tag(source, nodes, index, name.as_ref())
                {
                    let child_nodes = parse_modern_nodes_slice(
                        source,
                        &nodes[(index + 1)..close_index],
                        in_shadowroot_template,
                    );
                    out.push(parse_modern_loose_start_tag_node_with_fragment(
                        source,
                        node,
                        child_nodes,
                        Some(nodes[close_index].end_byte()),
                    ));
                    index = close_index + 1;
                    continue;
                }

                let mut stop = nodes.len();
                for (lookahead, candidate) in nodes.iter().enumerate().skip(index + 1) {
                    if is_loose_start_tag_boundary(*candidate) {
                        stop = lookahead;
                        break;
                    }
                }

                let child_nodes = parse_modern_nodes_slice(
                    source,
                    &nodes[(index + 1)..stop],
                    in_shadowroot_template,
                );
                let end_override = (stop > index + 1).then(|| nodes[stop - 1].end_byte());
                out.push(parse_modern_loose_start_tag_node_with_fragment(
                    source,
                    node,
                    child_nodes,
                    end_override,
                ));
                index = stop;
                continue;
            }
            "self_closing_tag" => out.push(parse_modern_loose_start_tag_node(source, node, None)),
            kind if is_typed_block_kind(kind) => {
                if let Some(block_node) = parse_modern_block(source, node, None) {
                    out.push(block_node);
                }
            }
            kind if is_typed_tag_kind(kind) => {
                if let Some(tag_node) = parse_modern_tag(source, node) {
                    out.push(tag_node);
                }
            }
            "block_open" => {
                // Recover unclosed block: block_open + (gap with keyword) + expression + block_close
                if let Some((block_node, consumed)) =
                    recover_loose_unclosed_block(source, nodes, index, in_shadowroot_template)
                {
                    out.push(block_node);
                    previous_end = Some(nodes[index + consumed - 1].end_byte());
                    index += consumed;
                    continue;
                }
            }
            "tag_name" => out.push(parse_modern_loose_tag_name_node(source, node)),
            "ERROR" => {
                out.extend(recover_modern_error_nodes(
                    source,
                    node,
                    in_shadowroot_template,
                ));
            }
            _ => {}
        }

        previous_end = Some(node.end_byte());
        index += 1;
    }

    out
}

/// Recover an unclosed block from loose token sequence: block_open, expression, block_close, ...
/// Returns the block node and the number of tokens consumed from the nodes slice.
fn recover_loose_unclosed_block(
    source: &str,
    nodes: &[TsNode<'_>],
    start_index: usize,
    in_shadowroot_template: bool,
) -> Option<(Node, usize)> {
    let block_open = nodes[start_index];
    if block_open.kind() != "block_open" {
        return None;
    }

    // Find expression and block_close after block_open
    let mut expr_index = None;
    let mut close_index = None;
    let mut binding_index = None;

    for i in (start_index + 1)..nodes.len().min(start_index + 6) {
        match nodes[i].kind() {
            "expression" if expr_index.is_none() => expr_index = Some(i),
            "pattern" if binding_index.is_none() => binding_index = Some(i),
            "block_close" if close_index.is_none() => {
                close_index = Some(i);
                break;
            }
            _ => {}
        }
    }

    let close_idx = close_index?;
    let block_start = block_open.start_byte();
    let block_close_end = nodes[close_idx].end_byte();

    // Determine keyword from source text between block_open and expression (or block_close)
    let keyword_start = block_open.end_byte();
    let keyword_end = expr_index
        .map(|i| nodes[i].start_byte())
        .unwrap_or(nodes[close_idx].start_byte());
    let keyword_text = source.get(keyword_start..keyword_end)?.trim();

    // Determine what children follow until the next boundary
    let children_start = close_idx + 1;
    let mut children_end = nodes.len();
    for i in children_start..nodes.len() {
        let kind = nodes[i].kind();
        // Stop at block_end tokens (like {/if})
        if kind == "block_end" || kind == "ERROR" {
            // Check if this ERROR is an end tag
            let text = text_for_node(source, nodes[i]);
            if text.starts_with("</") || text.starts_with("{/") {
                children_end = i;
                break;
            }
        }
        // Stop at start_tag or end_tag of parent elements
        if kind == "start_tag" || kind == "end_tag" {
            children_end = i;
            break;
        }
        // Stop at the next block_open (another unclosed block at same level)
        if kind == "block_open" {
            children_end = i;
            break;
        }
    }

    let child_nodes = if children_start < children_end {
        parse_modern_nodes_slice(source, &nodes[children_start..children_end], in_shadowroot_template)
    } else {
        Vec::new()
    };
    let block_end = if children_end > children_start {
        nodes[children_end - 1].end_byte()
    } else {
        block_close_end
    };

    let consumed = children_end - start_index;

    match keyword_text {
        "if" => {
            let test = expr_index
                .and_then(|i| parse_modern_expression_field(source, nodes[i]))
                .unwrap_or_else(|| Expression::empty(block_close_end, block_close_end));
            Some((
                Node::IfBlock(IfBlock {
                    elseif: false,
                    start: block_start,
                    end: block_end,
                    test,
                    consequent: Fragment {
                        r#type: FragmentType::Fragment,
                        nodes: child_nodes.into_boxed_slice(),
                    },
                    alternate: None,
                }),
                consumed,
            ))
        }
        "key" => {
            let expression = expr_index
                .and_then(|i| parse_modern_expression_field(source, nodes[i]))
                .unwrap_or_else(|| Expression::empty(block_close_end, block_close_end));
            Some((
                Node::KeyBlock(KeyBlock {
                    start: block_start,
                    end: block_end,
                    expression,
                    fragment: Fragment {
                        r#type: FragmentType::Fragment,
                        nodes: child_nodes.into_boxed_slice(),
                    },
                }),
                consumed,
            ))
        }
        kw if kw.starts_with("each") => {
            let expression = expr_index
                .and_then(|i| parse_modern_expression_field(source, nodes[i]))
                .unwrap_or_else(|| Expression::empty(block_close_end, block_close_end));
            let context = binding_index
                .and_then(|i| parse_modern_expression_field(source, nodes[i]))
                .unwrap_or_else(|| Expression::empty(block_close_end, block_close_end));
            Some((
                Node::EachBlock(EachBlock {
                    start: block_start,
                    end: block_end,
                    expression,
                    context: Some(context),
                    context_error: None,
                    has_as_clause: true,
                    invalid_key_without_as: false,
                    body: Fragment {
                        r#type: FragmentType::Fragment,
                        nodes: child_nodes.into_boxed_slice(),
                    },
                    fallback: None,
                    index: None,
                    key: None,
                }),
                consumed,
            ))
        }
        _ => None,
    }
}

fn push_modern_gap_text(source: &str, nodes: &mut Vec<Node>, start: usize, end: usize) {
    if start >= end {
        return;
    }
    let Some(raw) = source.get(start..end) else {
        return;
    };
    if raw.is_empty() {
        return;
    }
    push_modern_text_node(
        nodes,
        Text {
            start,
            end,
            raw: Arc::from(raw),
            data: Arc::from(raw),
        },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    If,
    Each,
    Await,
    Key,
    Snippet,
}

impl BlockKind {
    fn from_node_kind(kind: &str) -> Option<Self> {
        match kind {
            "if_block" => Some(Self::If),
            "each_block" => Some(Self::Each),
            "await_block" => Some(Self::Await),
            "key_block" => Some(Self::Key),
            "snippet_block" => Some(Self::Snippet),
            _ => None,
        }
    }
}

impl std::str::FromStr for BlockKind {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "if" => Ok(Self::If),
            "each" => Ok(Self::Each),
            "await" => Ok(Self::Await),
            "key" => Ok(Self::Key),
            "snippet" => Ok(Self::Snippet),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockBranchKind {
    Else,
    ElseIf,
    Then,
    Catch,
}

impl std::str::FromStr for BlockBranchKind {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "else" => Ok(Self::Else),
            "else if" => Ok(Self::ElseIf),
            "then" => Ok(Self::Then),
            "catch" => Ok(Self::Catch),
            _ => Err(()),
        }
    }
}

impl BlockBranchKind {
    fn parse_await_shorthand(raw: &str) -> Option<Self> {
        match raw {
            "then" => Some(Self::Then),
            "catch" => Some(Self::Catch),
            _ => None,
        }
    }
}

impl BlockKind {
    fn accepts(self, branch: BlockBranchKind) -> bool {
        match self {
            Self::If => matches!(branch, BlockBranchKind::Else | BlockBranchKind::ElseIf),
            Self::Each => branch == BlockBranchKind::Else,
            Self::Await => matches!(branch, BlockBranchKind::Then | BlockBranchKind::Catch),
            Self::Key | Self::Snippet => false,
        }
    }

    fn expected_branch_error(self) -> ParseErrorKind {
        match self {
            Self::Await => ParseErrorKind::ExpectedTokenAwaitBranch,
            Self::If | Self::Each | Self::Key | Self::Snippet => ParseErrorKind::ExpectedTokenElse,
        }
    }
}

pub(crate) fn is_typed_block_kind(kind: &str) -> bool {
    matches!(
        kind,
        "if_block" | "each_block" | "await_block" | "key_block" | "snippet_block"
    )
}

pub(crate) fn is_typed_tag_kind(kind: &str) -> bool {
    matches!(
        kind,
        "html_tag" | "debug_tag" | "const_tag" | "render_tag" | "attach_tag"
    )
}

/// Find the index in `children` where body nodes start, by skipping
/// field children from the hidden block start rule.
pub(crate) fn body_start_index(
    block: TsNode<'_>,
    children: &[TsNode<'_>],
    field_names: &[&str],
) -> usize {
    let mut max_idx = 0;
    for name in field_names {
        if let Some(field_node) = block.child_by_field_name(name)
            && let Some(idx) = children.iter().position(|c| c.id() == field_node.id())
        {
            max_idx = max_idx.max(idx + 1);
        }
    }
    max_idx
}

fn cst_node_has_direct_token(node: TsNode<'_>, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == token)
}

pub fn parse_modern_expression_tag(source: &str, node: TsNode<'_>) -> Option<ExpressionTag> {
    let expression = parse_modern_expression(source, node)?;

    Some(ExpressionTag {
        r#type: ExpressionTagType::ExpressionTag,
        start: node.start_byte(),
        end: node.end_byte(),
        expression,
    })
}

/// Loose-mode expression tag: always produces a tag, falling back to an empty
/// Identifier when the expression cannot be parsed.
pub(crate) fn parse_modern_expression_tag_loose(source: &str, node: TsNode<'_>) -> ExpressionTag {
    let expression = parse_modern_expression(source, node)
        .unwrap_or_else(|| loose_empty_expression_for_braces(source, node));

    ExpressionTag {
        r#type: ExpressionTagType::ExpressionTag,
        start: node.start_byte(),
        end: node.end_byte(),
        expression,
    }
}

/// Produce an empty Identifier expression spanning the content between `{` and `}`.
fn loose_empty_expression_for_braces(source: &str, node: TsNode<'_>) -> Expression {
    let raw = node.utf8_text(source.as_bytes()).ok().unwrap_or_default();
    let inner_start = node.start_byte().saturating_add(1);
    let inner_end = if raw.ends_with('}') {
        node.end_byte().saturating_sub(1)
    } else {
        node.end_byte()
    };
    modern_empty_identifier_expression_span(inner_start, inner_end.saturating_sub(inner_start))
}

fn loose_tag_name_range(
    source: &str,
    start: usize,
    fallback_end: usize,
) -> Option<(Arc<str>, usize)> {
    let raw = source.get(start..)?;
    let len = raw
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != '>' && *ch != '/')
        .map(char::len_utf8)
        .sum::<usize>();

    if len == 0 {
        let fallback = source.get(start..fallback_end).unwrap_or_default();
        if fallback.is_empty() {
            return None;
        }
        return Some((Arc::from(fallback), fallback_end));
    }

    let end = start + len;
    let text = source.get(start..end).unwrap_or_default();
    Some((Arc::from(text), end))
}

fn loose_tag_name_and_loc(
    source: &str,
    container: TsNode<'_>,
    name_node: Option<TsNode<'_>>,
) -> (Arc<str>, SourceRange) {
    let name_start = name_node.map(|node| node.start_byte()).unwrap_or_else(|| {
        container
            .start_byte()
            .saturating_add(1)
            .min(container.end_byte())
    });
    let fallback_end = name_node.map(|node| node.end_byte()).unwrap_or(name_start);

    if let Some((name, name_end)) = loose_tag_name_range(source, name_start, fallback_end) {
        return (
            name,
            SourceRange {
                start: line_column_from_point(
                    source,
                    name_node
                        .map(|node| node.start_position())
                        .unwrap_or_else(|| container.start_position()),
                    name_start,
                ),
                end: location_at_offset(source, name_end),
            },
        );
    }

    (
        Arc::from(""),
        SourceRange {
            start: line_column_from_point(
                source,
                container.start_position(),
                container.start_byte(),
            ),
            end: line_column_from_point(
                source,
                container.start_position(),
                container.start_byte(),
            ),
        },
    )
}

fn parse_modern_loose_tag_name_node(source: &str, node: TsNode<'_>) -> Node {
    let (name, name_loc) = loose_tag_name_and_loc(source, node, Some(node));
    let start = if node.start_byte() > 0
        && source.as_bytes().get(node.start_byte().saturating_sub(1)) == Some(&b'<')
    {
        node.start_byte() - 1
    } else {
        node.start_byte()
    };
    let end = name_loc.end.character;

    let fragment = Fragment {
        r#type: FragmentType::Fragment,
        nodes: Box::new([]),
    };

    let element = RegularElement {
        start,
        end,
        name,
        name_loc,
        self_closing: node.kind() == "self_closing_tag",
        has_end_tag: false,
        attributes: Box::new([]),
        fragment,
    };
    classify_modern_element(element, false, false)
}

fn parse_modern_loose_start_tag_node(
    source: &str,
    node: TsNode<'_>,
    trailing_text: Option<TsNode<'_>>,
) -> Node {
    let end_override = trailing_text.map(|text| text.end_byte());
    let fragment_nodes = trailing_text
        .map(|text| vec![Node::Text(parse_modern_text(source, text))])
        .unwrap_or_default();
    parse_modern_loose_start_tag_node_with_fragment(source, node, fragment_nodes, end_override)
}

fn parse_modern_loose_start_tag_node_with_fragment(
    source: &str,
    node: TsNode<'_>,
    fragment_nodes: Vec<Node>,
    end_override: Option<usize>,
) -> Node {
    let name_node = find_first_named_child(node, "tag_name");
    let (name, name_loc) = loose_tag_name_and_loc(source, node, name_node);

    let end = end_override.unwrap_or_else(|| node.end_byte());

    let attributes = parse_modern_attributes(source, node, false);
    let fragment = Fragment {
        r#type: FragmentType::Fragment,
        nodes: fragment_nodes.into_boxed_slice(),
    };

    let element = RegularElement {
        start: node.start_byte(),
        end,
        name,
        name_loc,
        self_closing: node.kind() == "self_closing_tag",
        has_end_tag: false,
        attributes: attributes.into_boxed_slice(),
        fragment,
    };
    classify_modern_element(element, false, false)
}

fn is_loose_start_tag_boundary(node: TsNode<'_>) -> bool {
    matches!(
        node.kind(),
        "start_tag"
            | "self_closing_tag"
            | "end_tag"
            | "block_end"
            | "else_if_clause"
            | "else_clause"
            | "await_branch"
    ) || is_typed_block_kind(node.kind())
}

fn start_end_tag_name(source: &str, node: TsNode<'_>) -> Option<Arc<str>> {
    find_first_named_child(node, "tag_name").map(|name| text_for_node(source, name))
}

fn find_matching_loose_end_tag(
    source: &str,
    nodes: &[TsNode<'_>],
    start_index: usize,
    target_name: &str,
) -> Option<usize> {
    let mut depth = 0usize;

    for (index, node) in nodes.iter().enumerate().skip(start_index + 1) {
        match node.kind() {
            "start_tag" => {
                if let Some(name) = start_end_tag_name(source, *node)
                    && name.as_ref() == target_name
                {
                    depth += 1;
                }
            }
            "end_tag" => {
                if let Some(name) = start_end_tag_name(source, *node)
                    && name.as_ref() == target_name
                {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth = depth.saturating_sub(1);
                }
            }
            "ERROR" => {
                // Check if this ERROR node contains an end tag pattern like </name>
                if let Some(name) = error_node_end_tag_name(source, *node)
                    && name.as_ref() == target_name
                {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth = depth.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    None
}

/// Check if an ERROR node represents a malformed end tag, extracting the tag name.
fn error_node_end_tag_name<'a>(source: &'a str, error: TsNode<'_>) -> Option<Arc<str>> {
    let text = text_for_node(source, error);
    let text = text.trim();
    if text.starts_with("</") && text.ends_with('>') {
        let name = text[2..text.len() - 1].trim();
        if !name.is_empty() {
            return Some(Arc::from(name));
        }
    }
    // Also handle </name without closing >
    if text.starts_with("</") {
        let name = text[2..].trim();
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
            return Some(Arc::from(name));
        }
    }
    None
}

