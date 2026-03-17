use std::sync::Arc;

use tree_sitter::Node as TsNode;

use oxc_span::GetSpan;
use super::super::text_for_node;
use super::{
    parse_modern_nodes_slice, body_start_index,
};
use super::errors::find_direct_named_child;
use super::expressions::{
    named_children_vec, parse_modern_expression_field_or_empty,
    parse_modern_binding_field, line_column_at_offset,
    modern_identifier_expression_with_loc, set_expression_character,
    parse_modern_expression_from_text,
};
use crate::ast::modern::*;

pub(super) fn parse_modern_snippet_block(source: &str, block: TsNode<'_>) -> Option<SnippetBlock> {
    let children = named_children_vec(block);
    let end_idx = children
        .iter()
        .rposition(|c| c.kind() == "block_end")
        .unwrap_or(children.len());

    let name_node = block
        .child_by_field_name("name")
        .or_else(|| block.child_by_field_name("expression"));
    let type_params_node = block.child_by_field_name("type_parameters");
    let params_node = block.child_by_field_name("parameters");
    let expression = parse_snippet_name(source, block, name_node);
    let type_params = parse_snippet_type_params(source, type_params_node);
    let parameters = parse_snippet_params(source, params_node);
    let body_start = body_start_index(block, &children, &["name", "type_parameters", "parameters"]);
    let body_nodes = parse_modern_nodes_slice(source, &children[body_start..end_idx], false);

    // Detect the missing-paren recovery rule from the CST: the grammar's prec(-1)
    // alternative matches `( params }` (no `)`) and tree-sitter inserts MISSING "}".
    // If block has a MISSING child and no `)` anonymous child, it's missing `)`.
    let has_lparen = (0..block.child_count())
        .filter_map(|i| block.child(i as u32))
        .any(|c| !c.is_named() && c.kind() == "(");
    let header_error = if block.has_error() && params_node.is_some() && has_lparen {
        let has_rparen = (0..block.child_count())
            .filter_map(|i| block.child(i as u32))
            .any(|c| !c.is_named() && c.kind() == ")");
        if has_rparen {
            // Has `)` but MISSING `}` — missing right brace
            let error_pos =
                snippet_header_error_pos(block, name_node, type_params_node, params_node);
            Some(SnippetHeaderError {
                kind: SnippetHeaderErrorKind::ExpectedRightBrace,
                start: error_pos,
                end: error_pos,
            })
        } else {
            // Has `(` and params but no `)` — missing right paren
            // Position at end of block_end node to match JS compiler behavior
            let error_pos = children
                .get(end_idx)
                .map(|n| n.end_byte())
                .unwrap_or(block.end_byte().saturating_sub(1));
            Some(SnippetHeaderError {
                kind: SnippetHeaderErrorKind::ExpectedRightParen,
                start: error_pos,
                end: error_pos,
            })
        }
    } else if block.has_error() && params_node.is_none() {
        let error_pos = name_node.and_then(|name| {
            let tail = source.get(name.end_byte()..block.end_byte())?;
            let lparen = tail.find('(')?;
            let rparen = tail.find(')')?;
            (lparen < rparen).then_some(name.end_byte() + rparen + 1)
        });
        error_pos.map(|pos| SnippetHeaderError {
            kind: SnippetHeaderErrorKind::ExpectedRightBrace,
            start: pos,
            end: pos,
        })
    } else {
        None
    };

    Some(SnippetBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        type_params,
        parameters: parameters.into_boxed_slice(),
        body: Fragment {
            r#type: FragmentType::Fragment,
            nodes: body_nodes.into_boxed_slice(),
        },
        header_error,
    })
}

/// Recover multiple snippet blocks from a single ERROR node that contains
/// several `{#snippet ...}{/snippet}` sequences. Returns an empty Vec if
/// the ERROR doesn't match this pattern (caller should try other recovery).
pub(super) fn recover_multiple_snippet_blocks(
    source: &str,
    error_node: TsNode<'_>,
) -> Vec<crate::ast::modern::Node> {
    let raw = source
        .get(error_node.start_byte()..error_node.end_byte())
        .unwrap_or_default();
    if !raw.contains("{#snippet") {
        return vec![];
    }

    // Find all `{#snippet` starts and `{/snippet}` ends in the ERROR text
    let base = error_node.start_byte();
    let mut blocks = Vec::new();
    let mut search_from = 0;
    while let Some(open_rel) = raw[search_from..].find("{#snippet") {
        let open_abs = base + search_from + open_rel;
        // Find the matching {/snippet}
        let after_open = search_from + open_rel;
        if let Some(close_rel) = raw[after_open..].find("{/snippet}") {
            let close_end = base + after_open + close_rel + "{/snippet}".len();
            // Extract the snippet name from between `{#snippet ` and `}`
            let header_start = after_open + "{#snippet".len();
            // Find the closing `}` of the open tag
            let header_text = &raw[header_start..after_open + close_rel];
            let open_brace_end = header_text.find('}').unwrap_or(0);
            let name_text = header_text[..open_brace_end].trim();

            let name_start = if name_text.is_empty() {
                // Empty name: position at the `}` of `{#snippet }`
                open_abs + "{#snippet".len() + open_brace_end
            } else {
                // Find name position in the source
                let name_offset = raw[after_open..].find(name_text).unwrap_or(0);
                base + after_open + name_offset
            };

            let expression = if name_text.is_empty() {
                Expression::empty(name_start, name_start)
            } else {
                parse_modern_expression_from_text(name_text, name_start, 0, 0)
                    .unwrap_or_else(|| Expression::empty(name_start, name_start))
            };

            // Compute body fragment: text between header close `}` and `{/snippet}`
            let header_end_rel = after_open + "{#snippet".len() + open_brace_end + 1;
            let body_start_abs = base + header_end_rel;
            let body_end_abs = base + after_open + close_rel;
            let body = if body_start_abs < body_end_abs {
                if let Some(body_text) = source.get(body_start_abs..body_end_abs) {
                    if !body_text.is_empty() {
                        Fragment {
                            r#type: FragmentType::Fragment,
                            nodes: Box::new([Node::Text(Text {
                                start: body_start_abs,
                                end: body_end_abs,
                                raw: Arc::from(body_text),
                                data: Arc::from(body_text),
                            })]),
                        }
                    } else {
                        empty_fragment()
                    }
                } else {
                    empty_fragment()
                }
            } else {
                empty_fragment()
            };

            blocks.push((open_abs, close_end, crate::ast::modern::Node::SnippetBlock(SnippetBlock {
                start: open_abs,
                end: close_end,
                expression,
                type_params: None,
                parameters: Box::new([]),
                body,
                header_error: None,
            })));

            search_from = after_open + close_rel + "{/snippet}".len();
        } else {
            break;
        }
    }

    if blocks.len() < 2 {
        // Only return if we found multiple blocks; single blocks go through normal recovery
        return vec![];
    }

    // Build result with Text nodes between snippet blocks
    let mut result = Vec::new();
    let mut prev_end = None;
    for (open_abs, close_end, node) in blocks {
        if let Some(pe) = prev_end {
            // Insert Text node for content between previous block end and this block start
            if pe < open_abs {
                if let Some(between_text) = source.get(pe..open_abs) {
                    if !between_text.is_empty() {
                        result.push(Node::Text(Text {
                            start: pe,
                            end: open_abs,
                            raw: Arc::from(between_text),
                            data: Arc::from(between_text),
                        }));
                    }
                }
            }
        }
        prev_end = Some(close_end);
        result.push(node);
    }
    result
}

pub(super) fn recover_malformed_snippet_block(source: &str, error_node: TsNode<'_>) -> Option<SnippetBlock> {
    // Don't recover ERROR nodes whose text starts with a branch opener ({:else, {:then, {:catch}).
    // Tree-sitter reuses snippet_name for identifiers inside those, so they'd be
    // misidentified as malformed snippets.
    let raw = source
        .get(error_node.start_byte()..error_node.end_byte())
        .unwrap_or_default();
    if raw.starts_with("{:") {
        return None;
    }
    recover_snippet_block_missing_right_brace(source, error_node)
        .or_else(|| recover_snippet_block_missing_right_paren(source, error_node))
}

fn recover_snippet_block_missing_right_brace(
    source: &str,
    error_node: TsNode<'_>,
) -> Option<SnippetBlock> {
    let start_node = find_snippet_start(error_node, source)?;
    let name_node = start_node
        .child_by_field_name("name")
        .or_else(|| start_node.child_by_field_name("expression"))
        .or_else(|| find_named_descendant(start_node, "snippet_name"));
    let type_params_node = start_node
        .child_by_field_name("type_parameters")
        .or_else(|| find_named_descendant(start_node, "snippet_type_parameters"));
    let params_node = start_node
        .child_by_field_name("parameters")
        .or_else(|| find_named_descendant(start_node, "snippet_parameters"));
    let has_lparen = (0..start_node.child_count())
        .filter_map(|i| start_node.child(i as u32))
        .any(|c| !c.is_named() && c.kind() == "(");

    let header_error = if params_node.is_some() && has_lparen {
        let error_pos =
            snippet_header_error_pos(start_node, name_node, type_params_node, params_node);
        Some(SnippetHeaderError {
            kind: SnippetHeaderErrorKind::ExpectedRightBrace,
            start: error_pos,
            end: error_pos,
        })
    } else {
        None
    };

    Some(SnippetBlock {
        start: error_node.start_byte(),
        end: error_node.end_byte(),
        expression: parse_snippet_name(source, start_node, name_node),
        type_params: parse_snippet_type_params(source, type_params_node),
        parameters: parse_snippet_params(source, params_node).into_boxed_slice(),
        body: snippet_recovery_body_fragment(source, error_node),
        header_error,
    })
}

fn recover_snippet_block_missing_right_paren(
    source: &str,
    error_node: TsNode<'_>,
) -> Option<SnippetBlock> {
    // In the new grammar, look for snippet_name instead of block_kind
    find_named_descendant(error_node, "snippet_name")?;

    let name_node = find_named_descendant(error_node, "snippet_name");
    let type_params_node = find_named_descendant(error_node, "snippet_type_parameters");
    let params_node = find_named_descendant(error_node, "snippet_parameters");
    let error_pos = error_node.end_byte().saturating_sub(1);

    Some(SnippetBlock {
        start: error_node.start_byte(),
        end: error_node.end_byte(),
        expression: parse_snippet_name(source, error_node, name_node),
        type_params: parse_snippet_type_params(source, type_params_node),
        parameters: parse_snippet_params(source, params_node).into_boxed_slice(),
        body: empty_fragment(),
        header_error: Some(SnippetHeaderError {
            kind: SnippetHeaderErrorKind::ExpectedRightParen,
            start: error_pos,
            end: error_pos,
        }),
    })
}

pub(crate) fn parse_snippet_type_params(
    source: &str,
    node: Option<TsNode<'_>>,
) -> Option<Arc<str>> {
    let raw = node?.utf8_text(source.as_bytes()).ok()?.trim();
    let inner = raw
        .strip_prefix('<')
        .and_then(|tail| tail.strip_suffix('>'))
        .unwrap_or(raw)
        .trim();
    (!inner.is_empty()).then(|| Arc::from(inner))
}

pub(crate) fn parse_snippet_params(
    source: &str,
    params_node: Option<TsNode<'_>>,
) -> Vec<Expression> {
    let Some(params_node) = params_node else {
        return Vec::new();
    };
    let raw = text_for_node(source, params_node);
    let Some(parsed) = crate::js::JsParameters::parse(raw.as_ref()).ok().map(Arc::new) else {
        return named_children_vec(params_node)
            .into_iter()
            .filter(|node| node.kind() == "pattern")
            .filter_map(|node| parse_modern_binding_field(source, node, false))
            .collect::<Vec<_>>();
    };

    let mut parameters = parsed
        .parameters()
        .items
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let span = parameter.span();
            Expression::from_parameter_item(
                parsed.clone(),
                index,
                params_node.start_byte() + span.start as usize - 1,
                params_node.start_byte() + span.end as usize - 1,
            )
        })
        .collect::<Vec<_>>();

    if let Some(rest) = parsed.rest_parameter() {
        let span = rest.span();
        parameters.push(Expression::from_rest_parameter(
            parsed,
            params_node.start_byte() + span.start as usize - 1,
            params_node.start_byte() + span.end as usize - 1,
        ));
    }

    parameters
}

pub(crate) fn parse_snippet_name(
    source: &str,
    owner: TsNode<'_>,
    name_node: Option<TsNode<'_>>,
) -> Expression {
    let mut expression = if let Some(name_node) = name_node {
        let raw = name_node
            .utf8_text(source.as_bytes())
            .ok()
            .unwrap_or_default();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            // Zero-width or whitespace-only name — produce empty identifier with loc
            let start = if name_node.start_byte() == name_node.end_byte() {
                name_node.start_byte()
            } else {
                name_node.end_byte().saturating_sub(1)
            };
            let (line, column) = line_column_at_offset(source, start);
            modern_identifier_expression_with_loc(Arc::from(""), start, start, line, column)
        } else {
            parse_modern_expression_field_or_empty(source, name_node)
        }
    } else {
        // No name node at all — fallback using first body child position
        let start = named_children_vec(owner)
            .first()
            .map(|n| n.start_byte().saturating_sub(1))
            .unwrap_or_else(|| owner.end_byte().saturating_sub(1));
        let (line, column) = line_column_at_offset(source, start);
        modern_identifier_expression_with_loc(Arc::from(""), start, start, line, column)
    };
    set_expression_character(source, &mut expression);
    expression
}

fn empty_fragment() -> Fragment {
    Fragment {
        r#type: FragmentType::Fragment,
        nodes: Box::new([]),
    }
}

fn snippet_recovery_body_fragment(source: &str, error_node: TsNode<'_>) -> Fragment {
    let Some(raw) = source.get(error_node.start_byte()..error_node.end_byte()) else {
        return empty_fragment();
    };
    let Some(header_close) = raw.find('}') else {
        return empty_fragment();
    };
    let Some(block_end_start) = raw.rfind("{/snippet}") else {
        return empty_fragment();
    };
    if header_close + 1 >= block_end_start {
        return empty_fragment();
    }

    let body_start = error_node.start_byte() + header_close + 1;
    let body_end = error_node.start_byte() + block_end_start;
    let Some(body_raw) = source.get(body_start..body_end) else {
        return empty_fragment();
    };
    if body_raw.is_empty() {
        return empty_fragment();
    }

    Fragment {
        r#type: FragmentType::Fragment,
        nodes: Box::new([Node::Text(Text {
            start: body_start,
            end: body_end,
            raw: Arc::from(body_raw),
            data: Arc::from(body_raw),
        })]),
    }
}

fn snippet_header_error_pos(
    start_node: TsNode<'_>,
    name_node: Option<TsNode<'_>>,
    type_params_node: Option<TsNode<'_>>,
    params_node: Option<TsNode<'_>>,
) -> usize {
    if let Some(error) = find_direct_named_child(start_node, "ERROR")
        .or_else(|| find_named_descendant(start_node, "ERROR"))
    {
        return error.start_byte();
    }

    params_node
        .or(type_params_node)
        .or(name_node)
        .map(|node| node.end_byte())
        .unwrap_or_else(|| start_node.end_byte().saturating_sub(1))
}

fn find_snippet_start<'tree>(node: TsNode<'tree>, _source: &str) -> Option<TsNode<'tree>> {
    if node.kind() == "snippet_block" {
        return Some(node);
    }
    // Also match nodes that contain snippet_name (partial snippet in ERROR)
    if find_direct_named_child(node, "snippet_name").is_some() {
        return Some(node);
    }

    for index in 0..node.child_count() {
        let Some(child) = node.child(index as u32) else {
            continue;
        };
        if let Some(found) = find_snippet_start(child, _source) {
            return Some(found);
        }
    }
    None
}

fn find_named_descendant<'tree>(node: TsNode<'tree>, kind: &str) -> Option<TsNode<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }

    for index in 0..node.child_count() {
        let Some(child) = node.child(index as u32) else {
            continue;
        };
        if let Some(found) = find_named_descendant(child, kind) {
            return Some(found);
        }
    }
    None
}

