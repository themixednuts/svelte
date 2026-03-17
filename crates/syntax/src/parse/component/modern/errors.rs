use std::sync::Arc;

use tree_sitter::Node as TsNode;

use super::super::{
    elements::is_void_element_name, find_first_named_child, text_for_node,
};
use super::{
    is_typed_block_kind, is_typed_tag_kind, body_start_index,
    BlockKind, BlockBranchKind,
};
use super::expressions::{
    named_children_vec,
    parse_modern_expression_error,
};
use super::special_tag_expression_node;
use crate::ast::common::{ParseError, ParseErrorKind};

pub(super) fn collect_parse_errors(source: &str, root: TsNode<'_>) -> Vec<ParseError> {
    fn walk(
        source: &str,
        node: TsNode<'_>,
        errors: &mut Vec<ParseError>,
        parent_kind: Option<&str>,
    ) {
        if is_typed_block_kind(node.kind()) {
            collect_block_parse_errors(source, node, errors);
        } else if node.kind() == "ERROR" && !parent_kind.is_some_and(is_typed_block_kind) {
            let error = parse_error_from_error_node(source, node);
            let checkpoint = errors.len();

            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                walk(source, child, errors, Some(node.kind()));
            }

            if let Some(error) = error
                && keep_error(source, node, &error, errors.len() > checkpoint)
            {
                errors.push(error);
            }
            return;
        } else if (node.kind() != "orphan_branch" || parent_kind != Some("ERROR"))
            && let Some(error) = parse_error_from_non_error_node(source, node)
        {
            errors.push(error);
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            walk(source, child, errors, Some(node.kind()));
        }
    }

    fn keep_error(
        _source: &str,
        node: TsNode<'_>,
        error: &ParseError,
        has_descendant_error: bool,
    ) -> bool {
        match error.kind {
            ParseErrorKind::BlockUnclosed => {
                if has_descendant_error {
                    return false;
                }

                // Don't emit unclosed block error for snippet blocks
                let is_snippet = find_direct_named_child(node, "snippet_block").is_some()
                    || find_direct_named_child(node, "snippet_name").is_some();
                !is_snippet
            }
            _ => true,
        }
    }

    let mut errors = Vec::new();
    walk(source, root, &mut errors, None);
    errors.sort_by_key(|error| (error.start, error.end));
    errors.dedup_by(|left, right| left.start == right.start && left.end == right.end);
    errors
}

fn collect_block_parse_errors(source: &str, block: TsNode<'_>, errors: &mut Vec<ParseError>) {
    let children = named_children_vec(block);
    let Some(block_kind) = BlockKind::from_node_kind(block.kind()) else {
        return;
    };

    let body_start = match block_kind {
        BlockKind::If => body_start_index(block, &children, &["expression"]),
        BlockKind::Each => {
            body_start_index(block, &children, &["expression", "binding", "index", "key"])
        }
        BlockKind::Key => body_start_index(block, &children, &["expression"]),
        BlockKind::Await => {
            body_start_index(block, &children, &["expression", "binding", "pending"])
        }
        BlockKind::Snippet => {
            body_start_index(block, &children, &["name", "type_parameters", "parameters"])
        }
    };

    // For await blocks, initialize previous from the pending section so that
    // branches can detect unclosed elements in the preceding section content.
    let mut previous: Option<TsNode<'_>> = None;
    if matches!(block_kind, BlockKind::Await)
        && let Some(pending) = block.child_by_field_name("pending")
    {
        if let Some((branch_kind, start)) = branch_in_section_container(source, pending) {
            let kind = if block_kind.accepts(branch_kind) {
                ParseErrorKind::BlockInvalidContinuationPlacement
            } else {
                block_kind.expected_branch_error()
            };
            errors.push(ParseError {
                kind,
                start,
                end: start,
            });
            return;
        }
        previous = Some(pending);
    }

    let mut has_end = false;
    let mut has_specific_error = false;

    // Pre-check: any body child with parse errors (suppresses generic block_unclosed later)
    let body_has_error = children[body_start..].iter().any(|c| c.has_error());

    // Check ERROR children in the header range for misplaced branches
    for child in children.iter().take(body_start) {
        if child.kind() == "ERROR"
            && let Some(branch_kind) = error_branch_kind(source, *child)
        {
            has_specific_error = true;
            let start = branch_start_in_error(source, *child);
            let kind = if block_kind.accepts(branch_kind) {
                innermost_unclosed_block_kind(source, *child)
                    .map(BlockKind::expected_branch_error)
                    .or_else(|| scope_aware_branch_error(source, block_kind, branch_kind, None))
                    .unwrap_or(ParseErrorKind::BlockInvalidContinuationPlacement)
            } else {
                block_kind.expected_branch_error()
            };
            errors.push(ParseError {
                kind,
                start,
                end: start,
            });
        }
    }

    for child in children.into_iter().skip(body_start) {
        match child.kind() {
            "text" | "entity"
                if text_for_node(source, child)
                    .chars()
                    .all(char::is_whitespace) => {}
            "comment" => {}
            "block_end" => {
                has_end = true;
                break;
            }
            "else_if_clause" | "else_clause" => {
                let branch_kind = match child.kind() {
                    "else_if_clause" => BlockBranchKind::ElseIf,
                    _ => BlockBranchKind::Else,
                };
                let kind = scope_aware_branch_error(source, block_kind, branch_kind, previous);

                if let Some(kind) = kind {
                    has_specific_error = true;
                    errors.push(ParseError {
                        kind,
                        start: child.start_byte().saturating_add(1),
                        end: child.start_byte().saturating_add(1),
                    });
                }
            }
            "await_branch" => {
                let branch_kind = find_first_named_child(child, "branch_kind")
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .and_then(|s| s.trim().parse::<BlockBranchKind>().ok());
                if let Some(branch_kind) = branch_kind {
                    let kind = scope_aware_branch_error(source, block_kind, branch_kind, previous);
                    if let Some(kind) = kind {
                        has_specific_error = true;
                        errors.push(ParseError {
                            kind,
                            start: child.start_byte().saturating_add(1),
                            end: child.start_byte().saturating_add(1),
                        });
                    }
                }
            }
            "orphan_branch" => {
                if let Some(branch_kind) = branch_kind_from_node(source, child) {
                    let kind = scope_aware_branch_error(source, block_kind, branch_kind, previous)
                        .unwrap_or_else(|| {
                            if block_kind.accepts(branch_kind) {
                                ParseErrorKind::BlockInvalidContinuationPlacement
                            } else {
                                block_kind.expected_branch_error()
                            }
                        });
                    has_specific_error = true;
                    let start = branch_start(child);
                    errors.push(ParseError {
                        kind,
                        start,
                        end: start,
                    });
                }
            }
            "ERROR" => {
                if let Some(branch_kind) = error_branch_kind(source, child) {
                    has_specific_error = true;
                    let start = branch_start_in_error(source, child);
                    let kind = if block_kind.accepts(branch_kind) {
                        innermost_unclosed_block_kind(source, child)
                            .map(BlockKind::expected_branch_error)
                            .or_else(|| {
                                scope_aware_branch_error(source, block_kind, branch_kind, previous)
                            })
                            .unwrap_or(ParseErrorKind::BlockInvalidContinuationPlacement)
                    } else {
                        block_kind.expected_branch_error()
                    };
                    errors.push(ParseError {
                        kind,
                        start,
                        end: start,
                    });
                }
                previous = Some(child);
            }
            _ => previous = Some(child),
        }
    }

    if !has_end && !has_specific_error {
        // Check for missing } in block start (ERROR between header and closing brace)
        if let Some(pos) = missing_brace_in_block_start(block) {
            errors.push(ParseError {
                kind: ParseErrorKind::ExpectedTokenRightBrace,
                start: pos,
                end: pos,
            });
            return;
        }

        // If a body child has parse errors, the real error is in the child — suppress
        // generic block_unclosed so the child's specific error takes priority.
        if body_has_error {
            return;
        }

        if let Some((branch, branch_pos)) = next_branch_after_node(source, block) {
            if !block_kind.accepts(branch) {
                errors.push(ParseError {
                    kind: block_kind.expected_branch_error(),
                    start: branch_pos,
                    end: branch_pos,
                });
            }
            // Valid or not, don't report generic block_unclosed — the block's
            // continuation exists but couldn't be included due to a deeper error.
            return;
        }

        errors.push(ParseError {
            kind: ParseErrorKind::BlockUnclosed,
            start: block.start_byte(),
            end: block.start_byte().saturating_add(1),
        });
    }
}

/// Determine the error kind when a branch appears after potentially unclosed content.
/// If the previous node contains an unclosed inner block, report that block's expected
/// branch error instead of a generic continuation placement error.
fn scope_aware_branch_error(
    source: &str,
    block_kind: BlockKind,
    branch_kind: BlockBranchKind,
    previous: Option<TsNode<'_>>,
) -> Option<ParseErrorKind> {
    if block_kind.accepts(branch_kind) {
        if let Some(prev) = previous
            && node_leaves_scope_open(source, prev)
        {
            innermost_unclosed_block_kind(source, prev)
                .map(|ik| ik.expected_branch_error())
                .or(Some(ParseErrorKind::BlockInvalidContinuationPlacement))
        } else {
            None
        }
    } else {
        Some(block_kind.expected_branch_error())
    }
}

/// Find the innermost unclosed typed block within a node, recursing through
/// section containers (await_pending, await_branch_children).
fn innermost_unclosed_block_kind(source: &str, node: TsNode<'_>) -> Option<BlockKind> {
    if is_typed_block_kind(node.kind()) && !has_named_descendant(node, "block_end") {
        return BlockKind::from_node_kind(node.kind());
    }
    match node.kind() {
        "await_pending" | "await_branch_children" => last_significant_child(source, node)
            .and_then(|child| innermost_unclosed_block_kind(source, child)),
        _ => None,
    }
}

fn branch_kind_from_node(source: &str, node: TsNode<'_>) -> Option<BlockBranchKind> {
    match node.kind() {
        "else_if_clause" => Some(BlockBranchKind::ElseIf),
        "else_clause" => Some(BlockBranchKind::Else),
        "orphan_branch" => node
            .child_by_field_name("kind")
            .and_then(|kind| kind.utf8_text(source.as_bytes()).ok())
            .and_then(|text| text.trim().parse().ok()),
        "await_branch" => find_first_named_child(node, "branch_kind")
            .and_then(|kind| kind.utf8_text(source.as_bytes()).ok())
            .and_then(|text| text.trim().parse().ok()),
        "branch_kind" | "shorthand_kind" => node
            .utf8_text(source.as_bytes())
            .ok()
            .and_then(|text| text.trim().parse().ok()),
        _ => None,
    }
}

fn branch_start(node: TsNode<'_>) -> usize {
    node.start_byte().saturating_add(1)
}

/// Check if a typed block's start has an ERROR between header fields and the
/// closing `}`, indicating a missing `}` in the block start syntax.
fn missing_brace_in_block_start(block: TsNode<'_>) -> Option<usize> {
    let mut cursor = block.walk();
    let all: Vec<_> = block.children(&mut cursor).collect();
    for (i, child) in all.iter().enumerate() {
        if child.kind() == "ERROR"
            && child.is_named()
            && all[i + 1..]
                .iter()
                .any(|next| !next.is_named() && next.kind() == "}")
        {
            return Some(child.start_byte());
        }
    }
    None
}

fn parse_error_from_error_node(source: &str, error: TsNode<'_>) -> Option<ParseError> {
    // Check for ERROR containing a valid branch structure with an invalid inner branch.
    // E.g., {:then bar}\n{:else if} inside an await context — the {:else if} is invalid.
    {
        let mut cursor = error.walk();
        let named_children = error.named_children(&mut cursor).collect::<Vec<_>>();
        for (index, child) in named_children.iter().copied().enumerate() {
            let context_kind = match child.kind() {
                "await_branch" => Some(BlockKind::Await),
                "else_clause" | "else_if_clause" => Some(BlockKind::If),
                _ => None,
            };
            if let Some(context_kind) = context_kind {
                for next in named_children.iter().copied().skip(index + 1) {
                    if let Some(inner_branch) = branch_kind_from_node(source, next) {
                        if !context_kind.accepts(inner_branch) {
                            let pos = branch_start(next);
                            return Some(ParseError {
                                kind: context_kind.expected_branch_error(),
                                start: pos,
                                end: pos,
                            });
                        }
                        break;
                    }
                }
            }
        }
    }

    // Check for typed blocks inside ERROR (unclosed block recovery)
    {
        let mut cursor = error.walk();
        if let Some(typed_block) = error
            .named_children(&mut cursor)
            .find(|c| is_typed_block_kind(c.kind()))
            && !has_named_descendant(typed_block, "block_end")
        {
            if let Some((branch_kind, start)) = next_branch_after_node(source, typed_block) {
                let block_kind = BlockKind::from_node_kind(typed_block.kind())
                    .expect("typed block should map to BlockKind");
                if !block_kind.accepts(branch_kind) {
                    return Some(ParseError {
                        kind: block_kind.expected_branch_error(),
                        start,
                        end: start,
                    });
                }
            }
            return Some(ParseError {
                kind: ParseErrorKind::BlockUnclosed,
                start: typed_block.start_byte(),
                end: typed_block.start_byte().saturating_add(1),
            });
        }
    }

    if let Some(name) = raw_text_error_name(source, error) {
        let raw_text = find_direct_named_child(error, "raw_text");
        let kind = match name.as_ref() {
            "script" if raw_text.is_some_and(|node| node.start_byte() == node.end_byte()) => {
                ParseErrorKind::UnexpectedEof
            }
            "script" => ParseErrorKind::ElementUnclosed { name },
            "style" if raw_text.is_some_and(|node| node.start_byte() == node.end_byte()) => {
                ParseErrorKind::ExpectedTokenStyleClose
            }
            "style" => ParseErrorKind::CssExpectedIdentifier,
            _ => {
                let start_tag = find_direct_named_child(error, "start_tag")?;
                return Some(ParseError {
                    kind: ParseErrorKind::ElementUnclosed { name },
                    start: start_tag.start_byte(),
                    end: start_tag.start_byte().saturating_add(1),
                });
            }
        };

        let (start, end) = match kind {
            ParseErrorKind::CssExpectedIdentifier => raw_text
                .map(|node| (node.start_byte(), node.start_byte()))
                .unwrap_or((error.end_byte(), error.end_byte())),
            _ => (error.end_byte(), error.end_byte()),
        };

        return Some(ParseError { kind, start, end });
    }

    if let Some(start_tag) = find_direct_named_child(error, "start_tag")
        && find_direct_named_child(error, "end_tag").is_none()
        && find_direct_named_child(error, "self_closing_tag").is_none()
    {
        let tag_name = find_direct_named_child(start_tag, "tag_name")?;
        return Some(ParseError {
            kind: ParseErrorKind::ElementUnclosed {
                name: text_for_node(source, tag_name),
            },
            start: start_tag.start_byte(),
            end: start_tag.start_byte().saturating_add(1),
        });
    }

    if let Some(tag_name) = invalid_closing_tag_name(source, error) {
        return Some(ParseError {
            kind: ParseErrorKind::ElementInvalidClosingTag { name: tag_name },
            start: error.start_byte(),
            end: error.start_byte(),
        });
    }

    let raw = source
        .get(error.start_byte()..error.end_byte())
        .unwrap_or_default();
    if raw.starts_with("<!--") {
        return Some(ParseError {
            kind: ParseErrorKind::ExpectedTokenCommentClose,
            start: error.end_byte(),
            end: error.end_byte(),
        });
    }

    if error_branch_kind(source, error).is_some() {
        return Some(ParseError {
            kind: ParseErrorKind::BlockInvalidContinuationPlacement,
            start: error.start_byte().saturating_add(1),
            end: error.start_byte().saturating_add(1),
        });
    }

    if raw == "<" {
        return Some(ParseError {
            kind: ParseErrorKind::UnexpectedEof,
            start: error.end_byte(),
            end: error.end_byte(),
        });
    }

    None
}

fn parse_error_from_non_error_node(source: &str, node: TsNode<'_>) -> Option<ParseError> {
    if let Some(error) = malformed_special_tag_whitespace_error(source, node) {
        return Some(error);
    }

    if let Some(error) = malformed_block_whitespace_error(node) {
        return Some(error);
    }

    if let Some(error) = special_tag_expression_parse_error(source, node) {
        return Some(error);
    }

    if node.kind() == "erroneous_end_tag"
        && let Some(name) = erroneous_end_tag_name(source, node)
    {
        if let Some(reason) = autoclosed_by(source, node, name.as_ref()) {
            return Some(ParseError {
                kind: ParseErrorKind::ElementInvalidClosingTagAutoclosed { name, reason },
                start: node.start_byte(),
                end: node.start_byte(),
            });
        }
        return Some(ParseError {
            kind: ParseErrorKind::ElementInvalidClosingTag { name },
            start: node.start_byte(),
            end: node.start_byte(),
        });
    }

    if node.kind() == "expression"
        && let Some(missing) = find_missing(node, "}")
    {
        let start = missing_right_brace_pos(node).unwrap_or_else(|| missing.start_byte());
        return Some(ParseError {
            kind: ParseErrorKind::ExpectedTokenRightBrace,
            start,
            end: start,
        });
    }

    if node.kind() == "expression" && is_attribute_placement_expression(source, node) {
        return None;
    }

    if node.kind() == "orphan_branch" {
        let start = branch_start(node);
        return Some(ParseError {
            kind: ParseErrorKind::BlockInvalidContinuationPlacement,
            start,
            end: start,
        });
    }

    if node.kind() == "expression"
        && let Some((start, message)) = parse_modern_expression_error(source, node)
    {
        // Don't report JS parse errors for expressions that look like block ends
        // (e.g., {/if} misidentified as expression when block structure is broken).
        let raw = source
            .get(node.start_byte()..node.end_byte())
            .unwrap_or_default();
        if raw.starts_with("{/") && raw.ends_with('}') {
            let inner = &raw[2..raw.len() - 1];
            if matches!(inner, "if" | "each" | "await" | "key" | "snippet") {
                return None;
            }
        }
        return Some(ParseError {
            kind: ParseErrorKind::JsParseError { message },
            start,
            end: start,
        });
    }

    if node.kind() == "self_closing_tag"
        && node.end_byte() == source.len()
        && find_missing(node, "/>").is_some()
    {
        return Some(ParseError {
            kind: ParseErrorKind::UnexpectedEof,
            start: source.len(),
            end: source.len(),
        });
    }

    if node.kind() == "element"
        && let Some(start_tag) = find_direct_named_child(node, "start_tag")
    {
        let raw = source
            .get(start_tag.start_byte()..start_tag.end_byte())
            .unwrap_or_default();
        if !raw.contains('>') && start_tag.end_byte() == source.len() {
            return Some(ParseError {
                kind: ParseErrorKind::UnexpectedEof,
                start: source.len(),
                end: source.len(),
            });
        }

        // Element with start_tag but no end_tag at document EOF → unclosed element
        if find_direct_named_child(node, "end_tag").is_none()
            && find_direct_named_child(node, "self_closing_tag").is_none()
            && node.end_byte() == source.len()
            && let Some(tag_name) = find_direct_named_child(start_tag, "tag_name")
        {
            let name = text_for_node(source, tag_name);
            if !is_void_element_name(name.as_ref()) {
                return Some(ParseError {
                    kind: ParseErrorKind::ElementUnclosed { name },
                    start: start_tag.start_byte(),
                    end: start_tag.start_byte().saturating_add(1),
                });
            }
        }
    }

    None
}

fn malformed_special_tag_whitespace_error(_source: &str, node: TsNode<'_>) -> Option<ParseError> {
    if !is_typed_tag_kind(node.kind()) {
        return None;
    }

    // Typed tags with missing whitespace between keyword and expression parse as
    // e.g. html_tag(ERROR(...)) with no expression field. Detect this pattern.
    let has_expression = node.child_by_field_name("expression").is_some();
    if has_expression {
        return None;
    }

    let has_error = find_direct_named_child(node, "ERROR").is_some();
    if !has_error {
        return None;
    }

    // The keyword length: html=4, debug=5, const=5, render=6, attach=6
    let keyword_len = match node.kind() {
        "html_tag" => 4,
        "debug_tag" => 5,
        "const_tag" => 5,
        "render_tag" => 6,
        "attach_tag" => 6,
        _ => return None,
    };

    // Position right after {@ + keyword
    let start = node.start_byte() + 2 + keyword_len;
    Some(ParseError {
        kind: ParseErrorKind::ExpectedWhitespace,
        start,
        end: start,
    })
}

fn malformed_block_whitespace_error(node: TsNode<'_>) -> Option<ParseError> {
    if node.kind() != "malformed_block" {
        return None;
    }

    let sigil = node.child_by_field_name("kind")?;
    Some(ParseError {
        kind: ParseErrorKind::BlockUnexpectedCharacter,
        start: node.start_byte(),
        end: sigil.end_byte(),
    })
}

fn special_tag_expression_parse_error(source: &str, node: TsNode<'_>) -> Option<ParseError> {
    if !is_typed_tag_kind(node.kind()) || is_attribute_value_tag(node) {
        return None;
    }

    let expr = special_tag_expression_node(node)?;
    let (start, message) = parse_modern_expression_error(source, expr)?;
    Some(ParseError {
        kind: ParseErrorKind::JsParseError { message },
        start,
        end: start,
    })
}

fn is_attribute_value_tag(mut node: TsNode<'_>) -> bool {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "quoted_attribute_value" | "unquoted_attribute_value" => return true,
            "attribute" | "start_tag" | "self_closing_tag" | "element" | "document" => {
                return false;
            }
            _ => node = parent,
        }
    }
    false
}

fn is_attribute_placement_expression(_source: &str, node: TsNode<'_>) -> bool {
    if !is_attribute_value_expression(node) {
        return false;
    }

    if !node.has_error() {
        return false;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "ERROR" {
            continue;
        }

        let mut error_cursor = child.walk();
        if child.named_children(&mut error_cursor).any(|error_child| {
            is_typed_block_kind(error_child.kind()) || is_typed_tag_kind(error_child.kind())
        }) {
            return true;
        }
    }

    false
}

fn is_attribute_value_expression(mut node: TsNode<'_>) -> bool {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "quoted_attribute_value" | "unquoted_attribute_value" => return true,
            "attribute" | "start_tag" | "self_closing_tag" | "element" | "document" => {
                return false;
            }
            _ => node = parent,
        }
    }
    false
}

fn missing_right_brace_pos(node: TsNode<'_>) -> Option<usize> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "self_closing_tag" {
            return Some(parent.end_byte().saturating_sub(1));
        }
        if matches!(parent.kind(), "element" | "document") {
            break;
        }
        current = parent;
    }
    None
}

fn autoclosed_by(source: &str, node: TsNode<'_>, name: &str) -> Option<Arc<str>> {
    let reason = previous_significant_sibling(source, node)?;
    if reason.kind() != "element" {
        return None;
    }
    let reason_name = cst_element_name(source, reason)?;
    let current = previous_significant_sibling(source, reason)?;
    if current.kind() != "element" || has_cst_end_tag(current) {
        return None;
    }
    let current_name = cst_element_name(source, current)?;
    if current_name.as_ref() != name || !closing_tag_omitted(name, reason_name.as_ref()) {
        return None;
    }
    Some(reason_name)
}

fn previous_significant_sibling<'tree>(source: &str, node: TsNode<'tree>) -> Option<TsNode<'tree>> {
    let mut current = node.prev_named_sibling();
    while let Some(sibling) = current {
        match sibling.kind() {
            "comment" => current = sibling.prev_named_sibling(),
            "text" | "entity" => {
                if text_for_node(source, sibling)
                    .chars()
                    .all(char::is_whitespace)
                {
                    current = sibling.prev_named_sibling();
                    continue;
                }
                return Some(sibling);
            }
            _ => return Some(sibling),
        }
    }
    None
}

fn cst_element_name(source: &str, node: TsNode<'_>) -> Option<Arc<str>> {
    let start_tag = find_direct_named_child(node, "start_tag")
        .or_else(|| find_direct_named_child(node, "self_closing_tag"))?;
    let tag_name = find_direct_named_child(start_tag, "tag_name")?;
    Some(text_for_node(source, tag_name))
}

fn has_cst_end_tag(node: TsNode<'_>) -> bool {
    find_direct_named_child(node, "end_tag").is_some()
}

fn closing_tag_omitted(current: &str, next: &str) -> bool {
    matches!(
        (current, next),
        ("li", "li")
            | ("dt", "dt" | "dd")
            | ("dd", "dt" | "dd")
            | (
                "p",
                "address"
                    | "article"
                    | "aside"
                    | "blockquote"
                    | "div"
                    | "dl"
                    | "fieldset"
                    | "footer"
                    | "form"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "header"
                    | "hgroup"
                    | "hr"
                    | "main"
                    | "menu"
                    | "nav"
                    | "ol"
                    | "p"
                    | "pre"
                    | "section"
                    | "table"
                    | "ul"
            )
            | ("rt", "rt" | "rp")
            | ("rp", "rt" | "rp")
            | ("optgroup", "optgroup")
            | ("option", "option" | "optgroup")
            | ("thead", "tbody" | "tfoot")
            | ("tbody", "tbody" | "tfoot")
            | ("tfoot", "tbody")
            | ("tr", "tr" | "tbody")
            | ("td", "td" | "th" | "tr")
            | ("th", "td" | "th" | "tr")
    )
}

fn error_branch_kind(source: &str, node: TsNode<'_>) -> Option<BlockBranchKind> {
    let children = named_children_vec(node);

    for child in &children {
        if let Some(kind) = branch_kind_from_node(source, *child) {
            return Some(kind);
        }
    }

    if children.len() == 1 && children[0].kind() == "ERROR" {
        return error_branch_kind(source, children[0]);
    }

    None
}

fn branch_start_in_error(source: &str, node: TsNode<'_>) -> usize {
    let children = named_children_vec(node);
    for child in &children {
        if branch_kind_from_node(source, *child).is_some() {
            return branch_start(*child);
        }
    }

    if children.len() == 1 && children[0].kind() == "ERROR" {
        return branch_start_in_error(source, children[0]);
    }

    node.start_byte().saturating_add(1)
}

fn next_branch_after_node(source: &str, node: TsNode<'_>) -> Option<(BlockBranchKind, usize)> {
    let mut current = node.next_named_sibling();
    while let Some(sibling) = current {
        match sibling.kind() {
            "comment" => current = sibling.next_named_sibling(),
            "text" | "entity"
                if text_for_node(source, sibling)
                    .chars()
                    .all(char::is_whitespace) =>
            {
                current = sibling.next_named_sibling();
            }
            "ERROR" => {
                let kind = error_branch_kind(source, sibling)?;
                return Some((kind, branch_start_in_error(source, sibling)));
            }
            _ => {
                let kind = branch_kind_from_node(source, sibling)?;
                return Some((kind, branch_start(sibling)));
            }
        }
    }

    let parent = node.parent()?;
    match parent.kind() {
        "await_pending" | "await_branch_children" => next_branch_after_node(source, parent),
        _ => None,
    }
}

fn branch_in_section_container(source: &str, node: TsNode<'_>) -> Option<(BlockBranchKind, usize)> {
    let child = last_significant_child(source, node)?;
    match child.kind() {
        "ERROR" => {
            let kind = error_branch_kind(source, child)?;
            Some((kind, branch_start_in_error(source, child)))
        }
        _ => {
            let kind = branch_kind_from_node(source, child)?;
            Some((kind, branch_start(child)))
        }
    }
}

fn node_leaves_scope_open(source: &str, node: TsNode<'_>) -> bool {
    match node.kind() {
        "start_tag" => true,
        "element" => {
            !has_named_descendant(node, "end_tag")
                && !has_named_descendant(node, "self_closing_tag")
        }
        kind if is_typed_block_kind(kind) => !has_named_descendant(node, "block_end"),
        "await_pending" | "await_branch_children" => last_significant_child(source, node)
            .is_some_and(|child| node_leaves_scope_open(source, child)),
        "ERROR" => true,
        _ => false,
    }
}

fn has_named_descendant(node: TsNode<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == kind || has_named_descendant(child, kind))
}

fn has_typed_block_descendant(node: TsNode<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| is_typed_block_kind(child.kind()) || has_typed_block_descendant(child))
}

fn find_missing<'a>(node: TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    for index in 0..node.child_count() {
        let child = node.child(index as u32)?;
        if child.is_missing() && child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = find_missing(child, kind) {
            return Some(found);
        }
    }
    None
}

fn invalid_closing_tag_name(source: &str, node: TsNode<'_>) -> Option<Arc<str>> {
    if has_named_descendant(node, "start_tag")
        || has_named_descendant(node, "self_closing_tag")
        || has_typed_block_descendant(node)
    {
        return None;
    }

    find_descendant(node, |child| child.kind() == "tag_name").map(|tag| text_for_node(source, tag))
}

fn erroneous_end_tag_name(source: &str, node: TsNode<'_>) -> Option<Arc<str>> {
    find_descendant(node, |child| child.kind() == "erroneous_end_tag_name")
        .map(|name| text_for_node(source, name))
}

fn find_descendant<'a, F>(node: TsNode<'a>, matches: F) -> Option<TsNode<'a>>
where
    F: Fn(TsNode<'a>) -> bool + Copy,
{
    if matches(node) {
        return Some(node);
    }

    for index in 0..node.child_count() {
        let Some(child) = node.child(index as u32) else {
            continue;
        };
        if let Some(found) = find_descendant(child, matches) {
            return Some(found);
        }
    }

    None
}

fn last_significant_child<'a>(source: &str, node: TsNode<'a>) -> Option<TsNode<'a>> {
    named_children_vec(node).into_iter().rev().find(|child| {
        !matches!(child.kind(), "comment")
            && (!(matches!(child.kind(), "text" | "entity"))
                || !text_for_node(source, *child)
                    .chars()
                    .all(char::is_whitespace))
    })
}

fn raw_text_error_name(source: &str, error: TsNode<'_>) -> Option<Arc<str>> {
    let start_tag = find_direct_named_child(error, "start_tag")?;
    let name = find_direct_named_child(start_tag, "tag_name")?;
    Some(text_for_node(source, name))
}

pub(super) fn find_direct_named_child<'a>(node: TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}
