use super::modern::{
    RawField, estree_node_field, estree_node_field_mut, estree_node_field_str, estree_node_type,
    modern_element_name, parse_modern_script, parse_modern_style, recover_modern_error_nodes,
};
use super::*;
use crate::ast::common::Span;
use crate::ast::legacy;
use crate::ast::modern;
use crate::{SourceId, SourceText};

pub(crate) fn parse_root(source: &str, root: Node<'_>, _loose: bool) -> legacy::Root {
    let mut html_cst_children = Vec::new();
    let mut module = None;
    let mut instance = None;
    let mut css = None;
    let mut leading_consumed_until = 0usize;

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "element"
            && let Some(name) = modern_element_name(source, child)
        {
            match classify_element_name(name.as_ref()) {
                ElementKind::Script => {
                    if let Some(script) = parse_modern_script(source, child, None) {
                        match script.context {
                            modern::ScriptContext::Module => module = Some(script.into()),
                            modern::ScriptContext::Default => instance = Some(script.into()),
                        }
                        if child.start_byte() <= leading_consumed_until {
                            leading_consumed_until = leading_consumed_until.max(child.end_byte());
                        }
                        continue;
                    }
                }
                ElementKind::Style => {
                    css = parse_modern_style(source, child).map(legacy_style_from_modern_css);
                    if child.start_byte() <= leading_consumed_until {
                        leading_consumed_until = leading_consumed_until.max(child.end_byte());
                    }
                    continue;
                }
                _ => {}
            }
        }

        html_cst_children.push(child);
    }

    let html_children = parse_legacy_nodes_slice(source, &html_cst_children);

    let mut merged_children = Vec::new();
    let mut consumed_until = leading_consumed_until;
    for node in html_children {
        let start = legacy_node_start(&node);
        let end = legacy_node_end(&node);

        if start > consumed_until {
            let raw = source.get(consumed_until..start).unwrap_or_default();
            if !raw.is_empty() {
                push_legacy_text_node(
                    &mut merged_children,
                    legacy::Text {
                        start: consumed_until,
                        end: start,
                        raw: Some(Arc::from(raw)),
                        data: decode_html_entities(raw),
                    },
                );
            }
        }

        if start < consumed_until && end <= consumed_until {
            continue;
        }
        consumed_until = consumed_until.max(end);
        merged_children.push(node);
    }
    let html_children = merged_children;

    let first_meaningful = html_children
        .iter()
        .find(|node| !is_legacy_whitespace_text(node));
    let last_meaningful = html_children
        .iter()
        .rfind(|node| !is_legacy_whitespace_text(node));
    let (fragment_start, fragment_end) = if first_meaningful.is_none() && !html_children.is_empty()
    {
        (
            html_children.first().map(legacy_node_end),
            html_children.last().map(legacy_node_start),
        )
    } else {
        (
            first_meaningful.map(legacy_node_start),
            last_meaningful.map(legacy_node_end),
        )
    };

    let comments = collect_legacy_document_comments(
        source,
        &html_children,
        instance.as_ref(),
        module.as_ref(),
    );

    let mut document = legacy::Root {
        html: legacy::Fragment {
            r#type: legacy::FragmentType::Fragment,
            start: fragment_start,
            end: fragment_end,
            children: html_children.into_boxed_slice(),
        },
        css,
        instance,
        module,
        comments,
    };

    if !source.is_ascii() {
        remap_legacy_document_offsets_utf16(source, &mut document);
    }

    document
}

fn remap_legacy_document_offsets_utf16(source: &str, document: &mut legacy::Root) {
    let map = build_utf16_offset_map(source);
    remap_legacy_fragment_offsets(source, &map, &mut document.html);
}

fn build_utf16_offset_map(source: &str) -> Vec<usize> {
    let mut map = vec![0usize; source.len() + 1];
    let mut utf16_offset = 0usize;

    for (byte_index, ch) in source.char_indices() {
        map[byte_index] = utf16_offset;
        let byte_len = ch.len_utf8();
        for i in 1..byte_len {
            map[byte_index + i] = utf16_offset;
        }
        utf16_offset += ch.len_utf16();
    }
    map[source.len()] = utf16_offset;
    map
}

fn remap_legacy_fragment_offsets(source: &str, map: &[usize], fragment: &mut legacy::Fragment) {
    if let Some(start) = fragment.start.as_mut() {
        *start = remap_offset(*start, map);
    }
    if let Some(end) = fragment.end.as_mut() {
        *end = remap_offset(*end, map);
    }
    remap_legacy_nodes_offsets(source, map, &mut fragment.children);
}

fn remap_legacy_nodes_offsets(source: &str, map: &[usize], nodes: &mut [legacy::Node]) {
    for node in nodes {
        match node {
            legacy::Node::Text(text) => {
                text.start = remap_offset(text.start, map);
                text.end = remap_offset(text.end, map);
            }
            legacy::Node::Comment(comment) => {
                comment.start = remap_offset(comment.start, map);
                comment.end = remap_offset(comment.end, map);
            }
            legacy::Node::MustacheTag(tag) => {
                tag.start = remap_offset(tag.start, map);
                tag.end = remap_offset(tag.end, map);
                remap_legacy_expression_offsets(source, map, &mut tag.expression);
            }
            legacy::Node::RawMustacheTag(tag) => {
                tag.start = remap_offset(tag.start, map);
                tag.end = remap_offset(tag.end, map);
                remap_legacy_expression_offsets(source, map, &mut tag.expression);
            }
            legacy::Node::DebugTag(tag) => {
                tag.start = remap_offset(tag.start, map);
                tag.end = remap_offset(tag.end, map);
                for identifier in tag.identifiers.iter_mut() {
                    identifier.start = remap_offset(identifier.start, map);
                    identifier.end = remap_offset(identifier.end, map);
                    if let Some(loc) = identifier.loc.as_mut() {
                        let (start_line, start_column) =
                            line_column_at_offset(source, identifier.start);
                        let (end_line, end_column) = line_column_at_offset(source, identifier.end);
                        loc.start.line = start_line;
                        loc.start.column = start_column;
                        loc.end.line = end_line;
                        loc.end.column = end_column;
                    }
                }
            }
            legacy::Node::Element(element) => {
                element.start = remap_offset(element.start, map);
                element.end = remap_offset(element.end, map);
                remap_legacy_nodes_offsets(source, map, &mut element.children);
            }
            legacy::Node::Head(head) => {
                head.start = remap_offset(head.start, map);
                head.end = remap_offset(head.end, map);
                remap_legacy_nodes_offsets(source, map, &mut head.children);
            }
            legacy::Node::InlineComponent(component) => {
                component.start = remap_offset(component.start, map);
                component.end = remap_offset(component.end, map);
                if let Some(expression) = component.expression.as_mut() {
                    remap_legacy_expression_offsets(source, map, expression);
                }
                remap_legacy_nodes_offsets(source, map, &mut component.children);
            }
            legacy::Node::IfBlock(block) => {
                block.start = remap_offset(block.start, map);
                block.end = remap_offset(block.end, map);
                remap_legacy_expression_offsets(source, map, &mut block.expression);
                remap_legacy_nodes_offsets(source, map, &mut block.children);
                if let Some(else_block) = block.else_block.as_mut() {
                    else_block.start = remap_offset(else_block.start, map);
                    else_block.end = remap_offset(else_block.end, map);
                    remap_legacy_nodes_offsets(source, map, &mut else_block.children);
                }
            }
            legacy::Node::EachBlock(block) => {
                block.start = remap_offset(block.start, map);
                block.end = remap_offset(block.end, map);
                if let Some(context) = block.context.as_mut() {
                    remap_legacy_expression_offsets(source, map, context);
                }
                remap_legacy_expression_offsets(source, map, &mut block.expression);
                if let Some(key) = block.key.as_mut() {
                    remap_legacy_expression_offsets(source, map, key);
                }
                remap_legacy_nodes_offsets(source, map, &mut block.children);
                if let Some(else_block) = block.else_block.as_mut() {
                    else_block.start = remap_offset(else_block.start, map);
                    else_block.end = remap_offset(else_block.end, map);
                    remap_legacy_nodes_offsets(source, map, &mut else_block.children);
                }
            }
            legacy::Node::KeyBlock(block) => {
                block.start = remap_offset(block.start, map);
                block.end = remap_offset(block.end, map);
                remap_legacy_expression_offsets(source, map, &mut block.expression);
                remap_legacy_nodes_offsets(source, map, &mut block.children);
            }
            legacy::Node::AwaitBlock(block) => {
                block.start = remap_offset(block.start, map);
                block.end = remap_offset(block.end, map);
                remap_legacy_expression_offsets(source, map, &mut block.expression);
                if let Some(value) = block.value.as_mut() {
                    remap_legacy_expression_offsets(source, map, value);
                }
                if let Some(error) = block.error.as_mut() {
                    remap_legacy_expression_offsets(source, map, error);
                }
                if let Some(start) = block.pending.start.as_mut() {
                    *start = remap_offset(*start, map);
                }
                if let Some(end) = block.pending.end.as_mut() {
                    *end = remap_offset(*end, map);
                }
                remap_legacy_nodes_offsets(source, map, &mut block.pending.children);
                if let Some(start) = block.then.start.as_mut() {
                    *start = remap_offset(*start, map);
                }
                if let Some(end) = block.then.end.as_mut() {
                    *end = remap_offset(*end, map);
                }
                remap_legacy_nodes_offsets(source, map, &mut block.then.children);
                if let Some(start) = block.catch.start.as_mut() {
                    *start = remap_offset(*start, map);
                }
                if let Some(end) = block.catch.end.as_mut() {
                    *end = remap_offset(*end, map);
                }
                remap_legacy_nodes_offsets(source, map, &mut block.catch.children);
            }
            legacy::Node::SnippetBlock(block) => {
                block.start = remap_offset(block.start, map);
                block.end = remap_offset(block.end, map);
                remap_legacy_expression_offsets(source, map, &mut block.expression);
                for parameter in block.parameters.iter_mut() {
                    remap_legacy_expression_offsets(source, map, parameter);
                }
                if let Some(error) = block.header_error.as_mut() {
                    error.start = remap_offset(error.start, map);
                    error.end = remap_offset(error.end, map);
                }
                remap_legacy_nodes_offsets(source, map, &mut block.children);
            }
        }
    }
}

fn remap_legacy_expression_offsets(
    source: &str,
    map: &[usize],
    expression: &mut legacy::Expression,
) {
    match expression {
        legacy::Expression::Identifier(identifier) => {
            let start_byte = identifier.start;
            let end_byte = identifier.end;
            identifier.start = remap_offset(start_byte, map);
            identifier.end = remap_offset(end_byte, map);
            if let Some(loc) = identifier.loc.as_mut() {
                let (start_line, start_col) = line_column_at_offset(source, start_byte);
                let (end_line, end_col) = line_column_at_offset(source, end_byte);
                loc.start.line = start_line;
                loc.start.column = start_col;
                loc.end.line = end_line;
                loc.end.column = end_col;
                if loc.start.character.is_some() {
                    loc.start.character = Some(identifier.start);
                }
                if loc.end.character.is_some() {
                    loc.end.character = Some(identifier.end);
                }
            }
        }
        legacy::Expression::Literal(literal) => {
            let start_byte = literal.start;
            let end_byte = literal.end;
            literal.start = remap_offset(start_byte, map);
            literal.end = remap_offset(end_byte, map);
            if let Some(loc) = literal.loc.as_mut() {
                let (start_line, start_col) = line_column_at_offset(source, start_byte);
                let (end_line, end_col) = line_column_at_offset(source, end_byte);
                loc.start.line = start_line;
                loc.start.column = start_col;
                loc.end.line = end_line;
                loc.end.column = end_col;
                if loc.start.character.is_some() {
                    loc.start.character = Some(literal.start);
                }
                if loc.end.character.is_some() {
                    loc.end.character = Some(literal.end);
                }
            }
        }
        legacy::Expression::BinaryExpression(binary) => {
            let start_byte = binary.start;
            let end_byte = binary.end;
            binary.start = remap_offset(start_byte, map);
            binary.end = remap_offset(end_byte, map);
            if let Some(loc) = binary.loc.as_mut() {
                let (start_line, start_col) = line_column_at_offset(source, start_byte);
                let (end_line, end_col) = line_column_at_offset(source, end_byte);
                loc.start.line = start_line;
                loc.start.column = start_col;
                loc.end.line = end_line;
                loc.end.column = end_col;
                if loc.start.character.is_some() {
                    loc.start.character = Some(binary.start);
                }
                if loc.end.character.is_some() {
                    loc.end.character = Some(binary.end);
                }
            }
            remap_legacy_expression_offsets(source, map, &mut binary.left);
            remap_legacy_expression_offsets(source, map, &mut binary.right);
        }
        legacy::Expression::CallExpression(call) => {
            let start_byte = call.start;
            let end_byte = call.end;
            call.start = remap_offset(start_byte, map);
            call.end = remap_offset(end_byte, map);
            if let Some(loc) = call.loc.as_mut() {
                let (start_line, start_col) = line_column_at_offset(source, start_byte);
                let (end_line, end_col) = line_column_at_offset(source, end_byte);
                loc.start.line = start_line;
                loc.start.column = start_col;
                loc.end.line = end_line;
                loc.end.column = end_col;
                if loc.start.character.is_some() {
                    loc.start.character = Some(call.start);
                }
                if loc.end.character.is_some() {
                    loc.end.character = Some(call.end);
                }
            }
            remap_legacy_expression_offsets(source, map, &mut call.callee);
            for argument in call.arguments.iter_mut() {
                remap_legacy_expression_offsets(source, map, argument);
            }
        }
        _ => {}
    }
}

fn remap_offset(offset: usize, map: &[usize]) -> usize {
    map.get(offset).copied().unwrap_or(offset)
}

fn legacy_style_from_modern_css(css: modern::Css) -> legacy::Style {
    let children = css
        .children
        .into_vec()
        .into_iter()
        .map(legacy_style_node_from_modern)
        .collect::<Vec<_>>()
        .into_boxed_slice();

    legacy::Style {
        r#type: legacy::StyleType::Style,
        start: css.start,
        end: css.end,
        attributes: css.attributes,
        children,
        content: css.content,
    }
}

fn legacy_style_node_from_modern(node: modern::CssNode) -> legacy::StyleNode {
    match node {
        modern::CssNode::Rule(rule) => legacy::StyleNode::Rule(legacy::StyleRule {
            prelude: legacy_style_selector_list_from_modern(rule.prelude),
            block: rule.block,
            start: rule.start,
            end: rule.end,
        }),
        modern::CssNode::Atrule(rule) => legacy::StyleNode::Atrule(legacy::StyleAtrule {
            start: rule.start,
            end: rule.end,
            name: rule.name,
            prelude: rule.prelude,
            block: rule.block,
        }),
    }
}

fn legacy_style_selector_list_from_modern(
    list: modern::CssSelectorList,
) -> legacy::StyleSelectorList {
    let children = list
        .children
        .into_vec()
        .into_iter()
        .map(|complex| {
            let mut simple = Vec::new();
            for relative in complex.children.into_vec() {
                simple.extend(relative.selectors.into_vec());
            }
            legacy::StyleSelector {
                r#type: legacy::StyleSelectorType::Selector,
                start: complex.start,
                end: complex.end,
                children: simple.into_boxed_slice(),
            }
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();

    legacy::StyleSelectorList {
        r#type: list.r#type,
        start: list.start,
        end: list.end,
        children,
    }
}

fn legacy_node_from_modern_loose(node: modern::Node) -> Option<legacy::Node> {
    match node {
        modern::Node::Text(text) => Some(legacy::Node::Text(legacy::Text {
            start: text.start,
            end: text.end,
            raw: Some(text.raw),
            data: text.data,
        })),
        modern::Node::ExpressionTag(tag) => Some(legacy::Node::MustacheTag(legacy::MustacheTag {
            start: tag.start,
            end: tag.end,
            expression: legacy_expression_from_modern_or_empty(tag.expression),
        })),
        modern::Node::DebugTag(tag) => Some(legacy::Node::DebugTag(legacy::DebugTag {
            start: tag.start,
            end: tag.end,
            arguments: tag
                .arguments
                .into_vec()
                .into_iter()
                .map(legacy_expression_from_modern_or_empty)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            identifiers: tag
                .identifiers
                .into_vec()
                .into_iter()
                .map(|identifier| legacy::IdentifierExpression {
                    name: identifier.name,
                    start: identifier.start,
                    end: identifier.end,
                    loc: identifier.loc,
                    fields: BTreeMap::new(),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        })),
        modern::Node::HtmlTag(_) | modern::Node::ConstTag(_) => None,
        modern::Node::Comment(comment) => Some(legacy::Node::Comment(legacy::Comment {
            start: comment.start,
            end: comment.end,
            ignores: parse_svelte_ignores(&comment.data),
            data: comment.data,
        })),
        modern::Node::RegularElement(element) => legacy_element_from_parts(
            element.start,
            element.end,
            element.name,
            element.attributes,
            element.fragment,
        ),
        modern::Node::TitleElement(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SvelteHead(head) => {
            let children = head
                .fragment
                .nodes
                .into_vec()
                .into_iter()
                .filter_map(legacy_node_from_modern_loose)
                .collect::<Vec<_>>()
                .into_boxed_slice();
            let attributes = legacy_attributes_from_modern(head.attributes.into_vec());
            Some(legacy::Node::Head(legacy::Head {
                start: head.start,
                end: head.end,
                name: head.name,
                attributes: attributes.into_boxed_slice(),
                children,
            }))
        }
        modern::Node::SvelteBody(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SvelteWindow(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SvelteDocument(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SvelteSelf(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SvelteFragment(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SvelteBoundary(el) => {
            legacy_element_from_parts(el.start, el.end, el.name, el.attributes, el.fragment)
        }
        modern::Node::SlotElement(element) => legacy_element_from_parts(
            element.start,
            element.end,
            element.name,
            element.attributes,
            element.fragment,
        ),
        modern::Node::SvelteComponent(component) => legacy_inline_component_from_parts(
            component.start,
            component.end,
            component.name,
            component.attributes,
            component.fragment,
        ),
        modern::Node::SvelteElement(el) => legacy_inline_component_from_parts(
            el.start,
            el.end,
            el.name,
            el.attributes,
            el.fragment,
        ),
        modern::Node::Component(component) => legacy_inline_component_from_parts(
            component.start,
            component.end,
            component.name,
            component.attributes,
            component.fragment,
        ),
        modern::Node::IfBlock(block) => {
            let children = trim_legacy_block_children(
                block
                    .consequent
                    .nodes
                    .into_vec()
                    .into_iter()
                    .filter_map(legacy_node_from_modern_loose)
                    .collect::<Vec<_>>(),
            )
            .into_boxed_slice();

            let else_block = match block.alternate.map(|b| *b) {
                Some(modern::Alternate::Fragment(fragment)) => {
                    let else_children = trim_legacy_block_children(
                        fragment
                            .nodes
                            .into_vec()
                            .into_iter()
                            .filter_map(legacy_node_from_modern_loose)
                            .collect::<Vec<_>>(),
                    )
                    .into_boxed_slice();
                    let start = else_children
                        .first()
                        .map(legacy_node_start)
                        .unwrap_or(block.end);
                    let end = else_children.last().map(legacy_node_end).unwrap_or(start);
                    Some(legacy::ElseBlock {
                        r#type: legacy::ElseBlockType::ElseBlock,
                        start,
                        end,
                        children: else_children,
                    })
                }
                Some(modern::Alternate::IfBlock(nested)) => {
                    let nested = legacy_node_from_modern_loose(modern::Node::IfBlock(nested))?;
                    let legacy::Node::IfBlock(nested_if) = nested else {
                        return None;
                    };
                    Some(legacy::ElseBlock {
                        r#type: legacy::ElseBlockType::ElseBlock,
                        start: nested_if.start,
                        end: nested_if.start,
                        children: vec![legacy::Node::IfBlock(nested_if)].into_boxed_slice(),
                    })
                }
                None => None,
            };

            Some(legacy::Node::IfBlock(legacy::IfBlock {
                start: block.start,
                end: block.end,
                expression: legacy_expression_from_modern_or_empty(block.test),
                children,
                else_block,
                elseif: block.elseif.then_some(true),
            }))
        }
        modern::Node::EachBlock(block) => {
            let children = trim_legacy_block_children(
                block
                    .body
                    .nodes
                    .into_vec()
                    .into_iter()
                    .filter_map(legacy_node_from_modern_loose)
                    .collect::<Vec<_>>(),
            )
            .into_boxed_slice();
            let else_block = block.fallback.and_then(|fallback| {
                let else_children = trim_legacy_block_children(
                    fallback
                        .nodes
                        .into_vec()
                        .into_iter()
                        .filter_map(legacy_node_from_modern_loose)
                        .collect::<Vec<_>>(),
                )
                .into_boxed_slice();

                if else_children.is_empty() {
                    return None;
                }

                let start = else_children
                    .first()
                    .map(legacy_node_start)
                    .unwrap_or(block.end);
                let end = else_children.last().map(legacy_node_end).unwrap_or(start);
                Some(legacy::ElseBlock {
                    r#type: legacy::ElseBlockType::ElseBlock,
                    start,
                    end,
                    children: else_children,
                })
            });
            Some(legacy::Node::EachBlock(legacy::EachBlock {
                start: block.start,
                end: block.end,
                children,
                context: block.context.map(legacy_expression_from_modern_or_empty),
                expression: legacy_expression_from_modern_or_empty(block.expression),
                index: block.index,
                key: block.key.map(legacy_expression_from_modern_or_empty),
                else_block,
            }))
        }
        modern::Node::KeyBlock(block) => {
            let children = trim_legacy_block_children(
                block
                    .fragment
                    .nodes
                    .into_vec()
                    .into_iter()
                    .filter_map(legacy_node_from_modern_loose)
                    .collect::<Vec<_>>(),
            )
            .into_boxed_slice();
            Some(legacy::Node::KeyBlock(legacy::KeyBlock {
                start: block.start,
                end: block.end,
                expression: legacy_expression_from_modern_or_empty(block.expression),
                children,
            }))
        }
        modern::Node::AwaitBlock(block) => {
            let pending_children = block
                .pending
                .unwrap_or(modern::Fragment {
                    r#type: modern::FragmentType::Fragment,
                    nodes: Box::new([]),
                })
                .nodes
                .into_vec()
                .into_iter()
                .filter_map(legacy_node_from_modern_loose)
                .collect::<Vec<_>>()
                .into_boxed_slice();
            let then_children = block
                .then
                .unwrap_or(modern::Fragment {
                    r#type: modern::FragmentType::Fragment,
                    nodes: Box::new([]),
                })
                .nodes
                .into_vec()
                .into_iter()
                .filter_map(legacy_node_from_modern_loose)
                .collect::<Vec<_>>()
                .into_boxed_slice();
            let catch_children = block
                .catch
                .unwrap_or(modern::Fragment {
                    r#type: modern::FragmentType::Fragment,
                    nodes: Box::new([]),
                })
                .nodes
                .into_vec()
                .into_iter()
                .filter_map(legacy_node_from_modern_loose)
                .collect::<Vec<_>>()
                .into_boxed_slice();

            let pending_is_empty = pending_children.is_empty();
            let then_is_empty = then_children.is_empty();
            let catch_is_empty = catch_children.is_empty();

            Some(legacy::Node::AwaitBlock(legacy::AwaitBlock {
                start: block.start,
                end: block.end,
                expression: legacy_expression_from_modern_or_empty(block.expression),
                value: block.value.map(legacy_expression_from_modern_or_empty),
                error: block.error.map(legacy_expression_from_modern_or_empty),
                pending: legacy::PendingBlock {
                    r#type: legacy::PendingBlockType::PendingBlock,
                    start: None,
                    end: None,
                    children: pending_children,
                    skip: pending_is_empty,
                },
                then: legacy::ThenBlock {
                    r#type: legacy::ThenBlockType::ThenBlock,
                    start: None,
                    end: None,
                    children: then_children,
                    skip: then_is_empty,
                },
                catch: legacy::CatchBlock {
                    r#type: legacy::CatchBlockType::CatchBlock,
                    start: None,
                    end: None,
                    children: catch_children,
                    skip: catch_is_empty,
                },
            }))
        }
        modern::Node::SnippetBlock(block) => {
            let children = block
                .body
                .nodes
                .into_vec()
                .into_iter()
                .filter_map(legacy_node_from_modern_loose)
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Some(legacy::Node::SnippetBlock(legacy::SnippetBlock {
                start: block.start,
                end: block.end,
                expression: legacy_expression_from_modern_or_empty(block.expression),
                type_params: block.type_params,
                parameters: block
                    .parameters
                    .into_vec()
                    .into_iter()
                    .map(legacy_expression_from_modern_or_empty)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                children,
                header_error: block.header_error,
            }))
        }
        modern::Node::RenderTag(tag) => Some(legacy::Node::MustacheTag(legacy::MustacheTag {
            start: tag.start,
            end: tag.end,
            expression: legacy_expression_from_modern_or_empty(tag.expression),
        })),
    }
}

pub(super) fn legacy_nodes_from_modern_loose(nodes: Vec<modern::Node>) -> Vec<legacy::Node> {
    nodes
        .into_iter()
        .filter_map(legacy_node_from_modern_loose)
        .collect()
}

fn legacy_nodes_from_modern_error_recovery(nodes: Vec<modern::Node>) -> Vec<legacy::Node> {
    legacy_nodes_from_modern_loose(nodes)
        .into_iter()
        .map(|node| match node {
            legacy::Node::SnippetBlock(mut block) => {
                block.children = Box::new([]);
                legacy::Node::SnippetBlock(block)
            }
            other => other,
        })
        .collect()
}

fn legacy_element_from_parts(
    start: usize,
    end: usize,
    name: Arc<str>,
    attributes: Box<[modern::Attribute]>,
    fragment: modern::Fragment,
) -> Option<legacy::Node> {
    let children = fragment
        .nodes
        .into_vec()
        .into_iter()
        .filter_map(legacy_node_from_modern_loose)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let attributes = legacy_attributes_from_modern(attributes.into_vec());
    Some(legacy::Node::Element(legacy::Element {
        start,
        end,
        name,
        tag: None,
        attributes: attributes.into_boxed_slice(),
        children,
    }))
}

fn legacy_inline_component_from_parts(
    start: usize,
    end: usize,
    name: Arc<str>,
    attributes: Box<[modern::Attribute]>,
    fragment: modern::Fragment,
) -> Option<legacy::Node> {
    let children = fragment
        .nodes
        .into_vec()
        .into_iter()
        .filter_map(legacy_node_from_modern_loose)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let attributes = legacy_attributes_from_modern(attributes.into_vec());
    Some(legacy::Node::InlineComponent(legacy::InlineComponent {
        start,
        end,
        name,
        expression: None,
        attributes: attributes.into_boxed_slice(),
        children,
    }))
}

fn legacy_attributes_from_modern(attributes: Vec<modern::Attribute>) -> Vec<legacy::Attribute> {
    let mut out = Vec::new();
    for attribute in attributes {
        match attribute {
            modern::Attribute::Attribute(named) => {
                out.push(legacy::Attribute::Attribute(legacy::NamedAttribute {
                    start: named.start,
                    end: named.end,
                    name: named.name,
                    name_loc: named.name_loc,
                    value: named.value.into(),
                }));
            }
            modern::Attribute::BindDirective(directive) => {
                out.push(legacy::Attribute::Binding(directive.into()));
            }
            modern::Attribute::OnDirective(directive) => {
                out.push(legacy::Attribute::EventHandler(directive.into()));
            }
            modern::Attribute::ClassDirective(directive) => {
                out.push(legacy::Attribute::Class(directive.into()));
            }
            modern::Attribute::LetDirective(directive) => {
                out.push(legacy::Attribute::Let(directive.into()));
            }
            modern::Attribute::UseDirective(directive) => {
                out.push(legacy::Attribute::Action(directive.into()));
            }
            modern::Attribute::AnimateDirective(directive) => {
                out.push(legacy::Attribute::Animation(directive.into()));
            }
            modern::Attribute::StyleDirective(directive) => {
                out.push(legacy::Attribute::StyleDirective(directive.into()));
            }
            modern::Attribute::TransitionDirective(directive) => {
                out.push(legacy::Attribute::Transition(directive.into()));
            }
            modern::Attribute::SpreadAttribute(spread) => {
                out.push(legacy::Attribute::Spread(legacy::SpreadAttribute {
                    start: spread.start,
                    end: spread.end,
                    expression: legacy_expression_from_modern_or_empty(spread.expression),
                }));
            }
            modern::Attribute::AttachTag(_) => {}
        }
    }
    out
}

fn legacy_expression_from_modern_or_empty(expression: modern::Expression) -> legacy::Expression {
    if let Some(converted) = legacy_expression_from_modern_expression(expression.clone(), false) {
        return converted;
    }
    let (start, end) = modern_expression_bounds(&expression).unwrap_or((0, 0));
    legacy_empty_identifier_expression(start, end, None)
}

fn modern_expression_bounds(expression: &modern::Expression) -> Option<(usize, usize)> {
    let raw = &expression.0;
    let start = estree_value_to_usize(estree_node_field(raw, RawField::Start))?;
    let end = estree_value_to_usize(estree_node_field(raw, RawField::End))?;
    Some((start, end))
}

fn modern_identifier_expression_with_loc(
    name: Arc<str>,
    start: usize,
    end: usize,
    loc: Option<modern::Loc>,
) -> modern::Expression {
    let mut fields = BTreeMap::new();
    fields.insert(
        "type".to_string(),
        modern::EstreeValue::String(Arc::from("Identifier")),
    );
    fields.insert("start".to_string(), modern::EstreeValue::UInt(start as u64));
    fields.insert("end".to_string(), modern::EstreeValue::UInt(end as u64));
    fields.insert("name".to_string(), modern::EstreeValue::String(name));
    if let Some(loc) = loc
        && let Ok(loc_value) = serde_json::to_value(loc)
        && let Ok(loc_raw) = serde_json::from_value::<modern::EstreeNode>(loc_value)
    {
        fields.insert("loc".to_string(), modern::EstreeValue::Object(loc_raw));
    }
    modern::Expression(modern::EstreeNode { fields }, Default::default())
}

// moved from api.rs during api cleanup
fn parse_legacy_children(
    source: &str,
    parent: Node<'_>,
    _recover_errors: bool,
    recovery_depth: usize,
) -> Vec<legacy::Node> {
    let mut cursor = parent.walk();
    let children = parent.named_children(&mut cursor).collect::<Vec<_>>();
    parse_legacy_nodes_slice_with_depth(source, &children, recovery_depth)
}

fn parse_legacy_element(source: &str, node: Node<'_>) -> legacy::Element {
    let mut cursor = node.walk();
    let mut start_tag: Option<Node<'_>> = None;
    let mut end_tag: Option<Node<'_>> = None;
    let mut self_closing_tag: Option<Node<'_>> = None;

    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "start_tag" => start_tag = Some(child),
            "end_tag" => end_tag = Some(child),
            "self_closing_tag" => self_closing_tag = Some(child),
            _ => {}
        }
    }

    let tag_node = start_tag.or(self_closing_tag);
    let name = tag_node
        .map(|tag| legacy_tag_name_from_tag_node(source, tag))
        .unwrap_or_default();

    let attributes = tag_node
        .map(|tag| parse_legacy_attributes(source, tag))
        .unwrap_or_default();

    let mut recovered_content_start = start_tag
        .map(|tag| tag.end_byte())
        .unwrap_or(node.start_byte());
    if let Some(start_tag_node) = start_tag
        && start_tag_node.has_error()
    {
        let attribute_end = attributes
            .iter()
            .map(legacy_attribute_end)
            .max()
            .unwrap_or(recovered_content_start);
        if let Some(rel_gt) = source
            .get(attribute_end..node.end_byte())
            .and_then(|tail| tail.find('>'))
        {
            recovered_content_start = attribute_end + rel_gt + 1;
        }
    }

    if matches!(classify_element_name(name.as_ref()), ElementKind::Textarea) {
        let content_start = start_tag
            .map(|tag| tag.end_byte())
            .unwrap_or(node.start_byte());
        let close_start =
            find_valid_legacy_closing_tag_start(source, content_start, node.end_byte(), "textarea");
        let content_end = match (end_tag.map(|tag| tag.start_byte()), close_start) {
            (Some(end_tag_start), Some(close_start)) => end_tag_start.min(close_start),
            (Some(end_tag_start), None) => end_tag_start,
            (None, Some(close_start)) => close_start,
            (None, None) => node.end_byte(),
        };
        let children = parse_legacy_textarea_children(source, content_start, content_end);

        return legacy::Element {
            start: node.start_byte(),
            end: node.end_byte(),
            name,
            tag: None,
            attributes: attributes.into_boxed_slice(),
            children: children.into_boxed_slice(),
        };
    }

    let mut element_end = node.end_byte();

    let mut children = Vec::new();
    let mut inner_cursor = node.walk();
    for child in node.named_children(&mut inner_cursor) {
        if child.start_byte() < recovered_content_start {
            if (child.kind() == "text" || child.kind() == "entity")
                && child.end_byte() > recovered_content_start
            {
                let raw = source
                    .get(recovered_content_start..child.end_byte())
                    .unwrap_or_default();
                if !raw.is_empty() {
                    push_legacy_text_node(
                        &mut children,
                        legacy::Text {
                            start: recovered_content_start,
                            end: child.end_byte(),
                            raw: Some(Arc::from(raw)),
                            data: decode_html_entities(raw),
                        },
                    );
                }
            }
            continue;
        }

        if child.end_byte() <= recovered_content_start {
            continue;
        }

        match child.kind() {
            "element" => children.push(legacy_node_from_element_with_source(
                source,
                parse_legacy_element(source, child),
            )),
            "text" | "entity" => {
                push_legacy_text_node(&mut children, parse_legacy_text(source, child));
            }
            "expression" => {
                if let Some(tag) = parse_mustache_tag(source, child) {
                    children.push(legacy::Node::MustacheTag(tag));
                }
            }
            kind if super::modern::is_typed_tag_kind(kind) => {
                if let Some(tag_node) = parse_legacy_tag(source, child) {
                    children.push(tag_node);
                }
            }
            "comment" => children.push(legacy::Node::Comment(parse_legacy_comment(source, child))),
            kind if super::modern::is_typed_block_kind(kind) => {
                if let Some(block) = parse_legacy_block(source, child) {
                    children.push(block);
                }
            }
            "ERROR" => {
                let mut recovered = parse_legacy_children(source, child, false, 0);
                children.append(&mut recovered);
            }
            "start_tag" | "end_tag" | "self_closing_tag" => {}
            _ => {}
        }
    }

    if let Some(first_start) = children.first().map(legacy_node_start)
        && recovered_content_start < first_start
    {
        let raw = source
            .get(recovered_content_start..first_start)
            .unwrap_or_default();
        if !raw.is_empty() {
            children.insert(
                0,
                legacy::Node::Text(legacy::Text {
                    start: recovered_content_start,
                    end: first_start,
                    raw: Some(Arc::from(raw)),
                    data: decode_html_entities(raw),
                }),
            );
        }
    }

    if matches!(classify_element_name(name.as_ref()), ElementKind::Style)
        && children.is_empty()
        && start_tag.is_some()
        && end_tag.is_some()
    {
        let empty_at = start_tag
            .map(|tag| tag.end_byte())
            .unwrap_or(node.start_byte());
        children.push(legacy::Node::Text(legacy::Text {
            start: empty_at,
            end: empty_at,
            raw: None,
            data: Arc::from(""),
        }));
    }

    if self_closing_tag.is_some() && end_tag.is_none() {
        let close_pattern = format!("</{}>", name);
        if !close_pattern.is_empty()
            && let Some(rel_close) = source
                .get(node.end_byte()..)
                .and_then(|s| s.find(&close_pattern))
        {
            let close_start = node.end_byte() + rel_close;
            let close_end = close_start + close_pattern.len();
            let inner = source.get(node.end_byte()..close_start).unwrap_or_default();
            if !inner.contains('<') {
                if children.is_empty() && !inner.is_empty() {
                    children.push(legacy::Node::Text(legacy::Text {
                        start: node.end_byte(),
                        end: close_start,
                        raw: Some(Arc::from(inner)),
                        data: decode_html_entities(inner),
                    }));
                }
                element_end = close_end;
            }
        }
    }

    if end_tag.is_none() && self_closing_tag.is_none() && is_legacy_void_element(name.as_ref()) {
        if let Some(start_tag) = start_tag {
            element_end = start_tag.end_byte();
        }
        children.clear();
    }

    if children.is_empty()
        && (node.has_error()
            || start_tag.is_some_and(|tag| tag.has_error())
            || end_tag.is_some_and(|tag| tag.has_error()))
    {
        let node_source = source
            .get(node.start_byte()..node.end_byte())
            .unwrap_or_default();
        if let (Some(rel_gt), Some(rel_lt)) = (node_source.find('>'), node_source.rfind('<'))
            && rel_gt < rel_lt
        {
            let content_start = node.start_byte() + rel_gt + 1;
            let content_end = node.start_byte() + rel_lt;
            if content_start < content_end {
                let raw = source.get(content_start..content_end).unwrap_or_default();
                if !raw.is_empty() {
                    children.push(legacy::Node::Text(legacy::Text {
                        start: content_start,
                        end: content_end,
                        raw: Some(Arc::from(raw)),
                        data: decode_html_entities(raw),
                    }));
                }
            }
        }
    }

    legacy::Element {
        start: node.start_byte(),
        end: element_end,
        name,
        tag: None,
        attributes: attributes.into_boxed_slice(),
        children: children.into_boxed_slice(),
    }
}

fn parse_legacy_doctype(source: &str, node: Node<'_>) -> Option<legacy::Node> {
    let raw = node.utf8_text(source.as_bytes()).ok()?.trim();
    if !raw.starts_with("<!doctype") || !raw.ends_with('>') {
        return None;
    }

    let inner = raw
        .strip_prefix("<!doctype")
        .and_then(|text| text.strip_suffix('>'))
        .unwrap_or_default();
    let base = node.start_byte() + "<!doctype".len();

    let mut attributes = Vec::new();
    let mut token_start: Option<usize> = None;
    for (idx, ch) in inner.char_indices() {
        if ch.is_whitespace() {
            if let Some(start_idx) = token_start.take() {
                let token = inner.get(start_idx..idx).unwrap_or_default();
                if !token.is_empty() {
                    let abs_start = base + start_idx;
                    let abs_end = base + idx;
                    attributes.push(legacy::Attribute::Attribute(legacy::NamedAttribute {
                        start: abs_start,
                        end: abs_end,
                        name: Arc::from(token),
                        name_loc: legacy::NameLocation {
                            start: source_location_at_offset(source, abs_start),
                            end: source_location_at_offset(source, abs_end),
                        },
                        value: legacy::AttributeValueList::Boolean(true),
                    }));
                }
            }
            continue;
        }

        if token_start.is_none() {
            token_start = Some(idx);
        }
    }

    if let Some(start_idx) = token_start {
        let token = inner.get(start_idx..).unwrap_or_default();
        if !token.is_empty() {
            let abs_start = base + start_idx;
            let abs_end = base + inner.len();
            attributes.push(legacy::Attribute::Attribute(legacy::NamedAttribute {
                start: abs_start,
                end: abs_end,
                name: Arc::from(token),
                name_loc: legacy::NameLocation {
                    start: source_location_at_offset(source, abs_start),
                    end: source_location_at_offset(source, abs_end),
                },
                value: legacy::AttributeValueList::Boolean(true),
            }));
        }
    }

    Some(legacy::Node::Element(legacy::Element {
        start: node.start_byte(),
        end: node.end_byte(),
        name: Arc::from("!doctype"),
        tag: None,
        attributes: attributes.into_boxed_slice(),
        children: Box::new([]),
    }))
}

fn legacy_tag_name_from_tag_node(source: &str, tag_node: Node<'_>) -> Arc<str> {
    let field_name = find_first_named_child(tag_node, "tag_name")
        .map(|node| text_for_node(source, node))
        .unwrap_or_default();

    let raw = tag_node
        .utf8_text(source.as_bytes())
        .ok()
        .unwrap_or_default()
        .trim_start();

    let after_lt = if let Some(rest) = raw.strip_prefix("</") {
        rest
    } else {
        raw.strip_prefix('<').unwrap_or(raw)
    };
    let parsed = after_lt
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != '>' && *ch != '/')
        .collect::<String>();

    if parsed.is_empty() {
        return field_name;
    }
    if !field_name.is_empty() && parsed.starts_with(field_name.as_ref()) {
        return Arc::from(parsed);
    }
    if field_name.is_empty() {
        return Arc::from(parsed);
    }
    field_name
}

fn parse_legacy_attributes(source: &str, tag_node: Node<'_>) -> Vec<legacy::Attribute> {
    let mut cursor = tag_node.walk();
    let mut out = Vec::new();

    for child in tag_node.named_children(&mut cursor) {
        if child.kind() != "attribute" {
            continue;
        }

        let mut attribute_end = child.end_byte();

        let name_node = find_first_named_child(child, "attribute_name");
        let mut name = if let Some(name_node) = name_node {
            text_for_node(source, name_node)
        } else {
            Arc::from("")
        };
        let mut name_loc = if let Some(name_node) = name_node {
            legacy::NameLocation {
                start: source_location_from_point(
                    source,
                    name_node.start_position(),
                    name_node.start_byte(),
                ),
                end: source_location_from_point(
                    source,
                    name_node.end_position(),
                    name_node.end_byte(),
                ),
            }
        } else {
            legacy::NameLocation {
                start: source_location_from_point(
                    source,
                    child.start_position(),
                    child.start_byte(),
                ),
                end: source_location_from_point(source, child.start_position(), child.start_byte()),
            }
        };

        let mut values = Vec::new();
        let mut spread_expression: Option<legacy::Expression> = None;
        let mut has_shorthand = false;
        let mut has_expression = false;
        let mut directive = None;
        let mut attr_cursor = child.walk();
        for attr_child in child.named_children(&mut attr_cursor) {
            match attr_child.kind() {
                "attribute_name" => directive = parse_directive_head(source, attr_child),
                "quoted_attribute_value" => {
                    let mut found_named = false;
                    let mut quoted_cursor = attr_child.walk();
                    for value_node in attr_child.named_children(&mut quoted_cursor) {
                        found_named = true;
                        if value_node.kind() == "attribute_value" || value_node.kind() == "entity" {
                            let text = parse_legacy_text(source, value_node);
                            let (parts, found_expression) =
                                split_legacy_unquoted_attribute_value_parts(source, text);
                            if found_expression {
                                has_expression = true;
                            }
                            for part in parts {
                                match part {
                                    legacy::AttributeValue::Text(text) => {
                                        push_legacy_attribute_text(&mut values, text)
                                    }
                                    other => values.push(other),
                                }
                            }
                        } else if value_node.kind() == "expression"
                            && let Some(tag) = parse_mustache_tag(source, value_node)
                        {
                            has_expression = true;
                            values.push(legacy::AttributeValue::MustacheTag(tag));
                        }
                    }

                    if !found_named {
                        let quote_start = attr_child.start_byte().saturating_add(1);
                        let quote_end = attr_child.end_byte().saturating_sub(1);
                        let raw = source.get(quote_start..quote_end).unwrap_or_default();
                        let (parts, found_expression) = split_legacy_unquoted_attribute_value_parts(
                            source,
                            legacy::Text {
                                start: quote_start,
                                end: quote_end,
                                raw: Some(Arc::from(raw)),
                                data: decode_html_entities(raw),
                            },
                        );
                        if found_expression {
                            has_expression = true;
                        }
                        if parts.is_empty() {
                            push_legacy_attribute_text(
                                &mut values,
                                legacy::Text {
                                    start: quote_start,
                                    end: quote_start,
                                    raw: Some(Arc::from("")),
                                    data: Arc::from(""),
                                },
                            );
                        } else {
                            for part in parts {
                                match part {
                                    legacy::AttributeValue::Text(text) => {
                                        push_legacy_attribute_text(&mut values, text)
                                    }
                                    other => values.push(other),
                                }
                            }
                        }
                    }
                }
                "unquoted_attribute_value" | "attribute_value" => {
                    let recovered =
                        recover_legacy_unquoted_attribute_value(source, attr_child, tag_node)
                            .unwrap_or_else(|| parse_legacy_text(source, attr_child));
                    attribute_end = attribute_end.max(recovered.end);
                    let (parts, found_expression) =
                        split_legacy_unquoted_attribute_value_parts(source, recovered);
                    if found_expression {
                        has_expression = true;
                    }
                    for part in parts {
                        match part {
                            legacy::AttributeValue::Text(text) => {
                                push_legacy_attribute_text(&mut values, text)
                            }
                            other => values.push(other),
                        }
                    }
                }
                "expression" => {
                    if let Some(tag) = parse_mustache_tag(source, attr_child) {
                        has_expression = true;
                        values.push(legacy::AttributeValue::MustacheTag(tag));
                    }
                }
                "shorthand_attribute" => {
                    has_shorthand = true;
                    if let Some(shorthand) = parse_attribute_shorthand(source, attr_child) {
                        if let legacy::Expression::Identifier(identifier) = &shorthand.expression
                            && let Some(loc) = identifier.loc.as_ref()
                        {
                            name = identifier.name.clone();
                            name_loc = legacy::NameLocation {
                                start: SourceLocation {
                                    line: loc.start.line,
                                    column: loc.start.column,
                                    character: loc.start.character.unwrap_or(identifier.start),
                                },
                                end: SourceLocation {
                                    line: loc.end.line,
                                    column: loc.end.column,
                                    character: loc.end.character.unwrap_or(identifier.end),
                                },
                            };
                        }

                        values.push(legacy::AttributeValue::AttributeShorthand(shorthand));
                    }
                }
                "spread_attribute" => {
                    if let Some((expression_text, expression_start)) =
                        spread_attribute_expression_text(source, attr_child)
                    {
                        let (line, column) = line_column_at_offset(source, expression_start);
                        spread_expression = parse_legacy_expression_from_text(
                            expression_text.as_ref(),
                            expression_start,
                            line,
                            column,
                            false,
                        );
                    }
                }
                _ => {}
            }
        }

        if let Some(expression) = spread_expression {
            out.push(legacy::Attribute::Spread(legacy::SpreadAttribute {
                start: child.start_byte(),
                end: child.end_byte(),
                expression,
            }));
            continue;
        }

        if let Some(head) = directive {
            if head.kind == Some(DirectiveKind::Style) {
                let style_value = if values.is_empty() {
                    legacy::AttributeValueList::Boolean(true)
                } else {
                    legacy::AttributeValueList::Values(values.into_boxed_slice())
                };

                out.push(legacy::Attribute::StyleDirective(legacy::StyleDirective {
                    start: child.start_byte(),
                    end: attribute_end,
                    name: head.name,
                    name_loc,
                    modifiers: head.modifiers,
                    value: style_value,
                }));
                continue;
            }

            let expression = values.iter().find_map(|value| match value {
                legacy::AttributeValue::MustacheTag(tag) => Some(tag.expression.clone()),
                legacy::AttributeValue::AttributeShorthand(shorthand) => {
                    Some(shorthand.expression.clone())
                }
                legacy::AttributeValue::Text(_) => None,
            });

            let expression = expression.or_else(|| {
                recover_legacy_braced_expression_from_attribute_values(source, &values)
            });

            let expression = if expression.is_some() {
                expression
            } else if head.kind == Some(DirectiveKind::Bind) && !head.name.is_empty() {
                let fallback_start = name_loc.end.character.saturating_sub(head.name.len());
                let fallback_end = name_loc.end.character;
                Some(legacy::Expression::Identifier(
                    legacy::IdentifierExpression {
                        start: fallback_start,
                        end: fallback_end,
                        name: head.name.clone(),
                        loc: None,
                        fields: BTreeMap::new(),
                    },
                ))
            } else {
                None
            };

            let directive = legacy::DirectiveAttribute {
                start: child.start_byte(),
                end: attribute_end,
                name: head.name,
                name_loc: name_loc.clone(),
                expression,
                modifiers: head.modifiers,
            };

            match head.kind {
                Some(DirectiveKind::Let) => out.push(legacy::Attribute::Let(directive)),
                Some(DirectiveKind::Use) => out.push(legacy::Attribute::Action(directive)),
                Some(DirectiveKind::Bind) => out.push(legacy::Attribute::Binding(directive)),
                Some(DirectiveKind::Class) => out.push(legacy::Attribute::Class(directive)),
                Some(DirectiveKind::Animate) => out.push(legacy::Attribute::Animation(directive)),
                Some(DirectiveKind::On) => out.push(legacy::Attribute::EventHandler(directive)),
                Some(kind) if kind.is_transition() => {
                    out.push(legacy::Attribute::Transition(legacy::TransitionDirective {
                        start: directive.start,
                        end: directive.end,
                        name: directive.name,
                        name_loc: directive.name_loc,
                        expression: directive.expression,
                        modifiers: directive.modifiers,
                        intro: kind.is_intro(),
                        outro: kind.is_outro(),
                    }))
                }
                _ => out.push(legacy::Attribute::Attribute(legacy::NamedAttribute {
                    start: child.start_byte(),
                    end: attribute_end,
                    name: head.prefix,
                    name_loc,
                    value: legacy::AttributeValueList::Values(values.into_boxed_slice()),
                })),
            }

            continue;
        }

        if !has_shorthand
            && !has_expression
            && values.is_empty()
            && let Some(recovered) =
                recover_legacy_boolean_attribute_value(source, child, tag_node, &name)
        {
            attribute_end = attribute_end.max(recovered.end);
            values.push(legacy::AttributeValue::Text(recovered));
        }

        let value = if has_shorthand || has_expression || !values.is_empty() {
            legacy::AttributeValueList::Values(values.into_boxed_slice())
        } else {
            legacy::AttributeValueList::Boolean(true)
        };

        out.push(legacy::Attribute::Attribute(legacy::NamedAttribute {
            start: child.start_byte(),
            end: attribute_end,
            name,
            name_loc,
            value,
        }));
    }

    out
}

fn legacy_attribute_end(attribute: &legacy::Attribute) -> usize {
    match attribute {
        legacy::Attribute::Attribute(attribute) => attribute.end,
        legacy::Attribute::Spread(attribute) => attribute.end,
        legacy::Attribute::Transition(attribute) => attribute.end,
        legacy::Attribute::StyleDirective(attribute) => attribute.end,
        legacy::Attribute::Let(attribute)
        | legacy::Attribute::Action(attribute)
        | legacy::Attribute::Binding(attribute)
        | legacy::Attribute::Class(attribute)
        | legacy::Attribute::Animation(attribute)
        | legacy::Attribute::EventHandler(attribute) => attribute.end,
    }
}

fn recover_legacy_unquoted_attribute_value(
    source: &str,
    value_node: Node<'_>,
    tag_node: Node<'_>,
) -> Option<legacy::Text> {
    let value = parse_legacy_text(source, value_node);

    if source.as_bytes().get(value.start).copied() == Some(b'{')
        && let Some(close) = find_matching_brace_close(source, value.start, tag_node.end_byte())
    {
        let end = close + 1;
        if end > value.end {
            let raw = source.get(value.start..end).unwrap_or_default();
            return Some(legacy::Text {
                start: value.start,
                end,
                raw: Some(Arc::from(raw)),
                data: decode_html_entities(raw),
            });
        }
    }

    let mut end = value.end;
    let bytes = source.as_bytes();
    while end < tag_node.end_byte() {
        let byte = *bytes.get(end)?;
        if byte.is_ascii_whitespace() || byte == b'>' {
            break;
        }
        end += 1;
    }

    if end == value.end {
        return None;
    }

    let raw = source.get(value.start..end).unwrap_or_default();
    Some(legacy::Text {
        start: value.start,
        end,
        raw: Some(Arc::from(raw)),
        data: decode_html_entities(raw),
    })
}

fn recover_legacy_boolean_attribute_value(
    source: &str,
    attribute_node: Node<'_>,
    tag_node: Node<'_>,
    name: &Arc<str>,
) -> Option<legacy::Text> {
    if name.is_empty() {
        return None;
    }

    let mut cursor = attribute_node.start_byte() + name.len();
    let bytes = source.as_bytes();
    while cursor < tag_node.end_byte() && bytes.get(cursor)?.is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor >= tag_node.end_byte() || *bytes.get(cursor)? != b'=' {
        return None;
    }
    cursor += 1;
    while cursor < tag_node.end_byte() && bytes.get(cursor)?.is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor >= tag_node.end_byte() {
        return None;
    }

    let start = cursor;
    let mut end = cursor;
    while end < tag_node.end_byte() {
        let byte = *bytes.get(end)?;
        if byte.is_ascii_whitespace() || byte == b'>' {
            break;
        }
        end += 1;
    }

    if end <= start {
        return None;
    }

    let raw = source.get(start..end).unwrap_or_default();
    Some(legacy::Text {
        start,
        end,
        raw: Some(Arc::from(raw)),
        data: decode_html_entities(raw),
    })
}

fn split_legacy_unquoted_attribute_value_parts(
    source: &str,
    value: legacy::Text,
) -> (Vec<legacy::AttributeValue>, bool) {
    let raw = value.raw.as_deref().unwrap_or_default();
    if !raw.contains('{') {
        return (vec![legacy::AttributeValue::Text(value)], false);
    }

    let mut out = Vec::new();
    let mut has_expression = false;
    let mut index = 0usize;

    while index < raw.len() {
        let remaining = &raw[index..];
        let Some(rel_open) = remaining.find('{') else {
            let start = value.start + index;
            let end = value.end;
            let text = raw.get(index..).unwrap_or_default();
            if !text.is_empty() {
                out.push(legacy::AttributeValue::Text(legacy::Text {
                    start,
                    end,
                    raw: Some(Arc::from(text)),
                    data: decode_html_entities(text),
                }));
            }
            break;
        };

        if rel_open > 0 {
            let text_start = value.start + index;
            let text_end = value.start + index + rel_open;
            let text = raw.get(index..(index + rel_open)).unwrap_or_default();
            if !text.is_empty() {
                out.push(legacy::AttributeValue::Text(legacy::Text {
                    start: text_start,
                    end: text_end,
                    raw: Some(Arc::from(text)),
                    data: decode_html_entities(text),
                }));
            }
        }

        let open = index + rel_open;
        let rest = &raw[(open + 1)..];
        let Some(rel_close) = rest.find('}') else {
            let text = raw.get(open..).unwrap_or_default();
            let start = value.start + open;
            out.push(legacy::AttributeValue::Text(legacy::Text {
                start,
                end: value.end,
                raw: Some(Arc::from(text)),
                data: decode_html_entities(text),
            }));
            break;
        };

        let close = open + 1 + rel_close;
        let expr_body = raw.get((open + 1)..close).unwrap_or_default();
        let trimmed = expr_body.trim();
        let leading = expr_body.find(trimmed).unwrap_or(0);
        let expr_start = value.start + open + 1 + leading;
        let (line, column) = line_column_at_offset(source, expr_start);

        if let Some(expression) =
            parse_legacy_expression_from_text(trimmed, expr_start, line, column, false)
        {
            has_expression = true;
            out.push(legacy::AttributeValue::MustacheTag(legacy::MustacheTag {
                start: value.start + open,
                end: value.start + close + 1,
                expression,
            }));
        } else {
            let text = raw.get(open..(close + 1)).unwrap_or_default();
            out.push(legacy::AttributeValue::Text(legacy::Text {
                start: value.start + open,
                end: value.start + close + 1,
                raw: Some(Arc::from(text)),
                data: decode_html_entities(text),
            }));
        }

        index = close + 1;
    }

    (out, has_expression)
}

fn parse_legacy_textarea_children(source: &str, start: usize, end: usize) -> Vec<legacy::Node> {
    if start > end {
        return Vec::new();
    }

    let raw = source.get(start..end).unwrap_or_default();
    if raw.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut index = 0usize;

    while index < raw.len() {
        let Some(rel_open) = raw[index..].find('{') else {
            let text = &raw[index..];
            if !text.is_empty() {
                push_legacy_text_node(
                    &mut out,
                    legacy::Text {
                        start: start + index,
                        end,
                        raw: Some(Arc::from(text)),
                        data: decode_html_entities(text),
                    },
                );
            }
            break;
        };

        let open = index + rel_open;
        if open > index {
            let text = &raw[index..open];
            push_legacy_text_node(
                &mut out,
                legacy::Text {
                    start: start + index,
                    end: start + open,
                    raw: Some(Arc::from(text)),
                    data: decode_html_entities(text),
                },
            );
        }

        let mut depth = 0usize;
        let mut close = None;
        for (rel_idx, ch) in raw[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(open + rel_idx);
                        break;
                    }
                }
                _ => {}
            }
        }

        let Some(close) = close else {
            let text = &raw[open..];
            push_legacy_text_node(
                &mut out,
                legacy::Text {
                    start: start + open,
                    end,
                    raw: Some(Arc::from(text)),
                    data: decode_html_entities(text),
                },
            );
            break;
        };

        let inner = &raw[(open + 1)..close];
        let trimmed = inner.trim();
        let leading = inner.find(trimmed).unwrap_or(0);
        let expr_start = start + open + 1 + leading;
        let (line, column) = line_column_at_offset(source, expr_start);

        if let Some(expression) =
            parse_legacy_expression_from_text(trimmed, expr_start, line, column, false)
        {
            out.push(legacy::Node::MustacheTag(legacy::MustacheTag {
                start: start + open,
                end: start + close + 1,
                expression,
            }));
        } else {
            let text = &raw[open..=close];
            push_legacy_text_node(
                &mut out,
                legacy::Text {
                    start: start + open,
                    end: start + close + 1,
                    raw: Some(Arc::from(text)),
                    data: decode_html_entities(text),
                },
            );
        }

        index = close + 1;
    }

    out
}

fn recover_legacy_braced_expression_from_attribute_values(
    source: &str,
    values: &[legacy::AttributeValue],
) -> Option<legacy::Expression> {
    let [legacy::AttributeValue::Text(text)] = values else {
        return None;
    };

    let raw = text.raw.as_deref()?;
    let inner = raw.strip_prefix('{')?.strip_suffix('}')?;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading = inner.find(trimmed).unwrap_or(0);
    let expr_start = text.start + 1 + leading;
    let (line, column) = line_column_at_offset(source, expr_start);
    parse_legacy_expression_from_text(trimmed, expr_start, line, column, false)
}

pub(crate) fn parse_modern_attributes(
    source: &str,
    tag_node: Node<'_>,
    loose: bool,
) -> Vec<modern::Attribute> {
    let mut cursor = tag_node.walk();
    let mut out = Vec::new();

    for child in tag_node.named_children(&mut cursor) {
        if child.kind() == "ERROR" {
            if merge_trailing_expression_brace(source, child, &mut out) {
                continue;
            }
            if let Some((name, start, end)) =
                recover_modern_invalid_attribute_from_error(source, child)
            {
                out.push(modern::Attribute::Attribute(modern::NamedAttribute {
                    start,
                    end,
                    name,
                    name_loc: legacy::NameLocation {
                        start: source_location_at_offset(source, start),
                        end: source_location_at_offset(source, end),
                    },
                    value: modern::AttributeValueList::Boolean(true),
                    value_syntax: modern::AttributeValueSyntax::Boolean,
                    error: Some(modern::AttrError {
                        kind: modern::AttrErrorKind::InvalidName,
                        start,
                        end,
                    }),
                }));
            }
            continue;
        }
        if child.kind() != "attribute" {
            continue;
        }

        let mut has_attach_tag = false;
        let mut expression_values = Vec::new();
        let mut value_parts = Vec::new();
        let mut has_expression = false;

        let name_node = find_first_named_child(child, "attribute_name");
        let mut name = if let Some(name_node) = name_node {
            text_for_node(source, name_node)
        } else {
            Arc::from("")
        };

        let attribute_start = name_node
            .map(|n| n.start_byte())
            .unwrap_or(child.start_byte());

        let mut name_loc = if let Some(name_node) = name_node {
            legacy::NameLocation {
                start: source_location_from_point(
                    source,
                    name_node.start_position(),
                    name_node.start_byte(),
                ),
                end: source_location_from_point(
                    source,
                    name_node.end_position(),
                    name_node.end_byte(),
                ),
            }
        } else {
            let start = child.start_byte().saturating_add(1).min(child.end_byte());
            legacy::NameLocation {
                start: source_location_at_offset(source, start),
                end: source_location_at_offset(source, start),
            }
        };

        let mut directive = None;
        let mut spread_expression: Option<modern::Expression> = None;
        let mut directive_value_syntax = modern::DirectiveValueSyntax::Implicit;
        let mut directive_value_start = 0usize;
        let mut attribute_value_syntax = modern::AttributeValueSyntax::Boolean;
        let mut error = None;

        let mut attr_cursor = child.walk();
        for attr_child in child.named_children(&mut attr_cursor) {
            match attr_child.kind() {
                "spread_attribute" => {
                    if let Some((expression_text, expression_start)) =
                        spread_attribute_expression_text(source, attr_child)
                    {
                        let (line, column) = line_column_at_offset(source, expression_start);
                        spread_expression =
                            crate::compiler::phases::parse::parse_modern_expression_with_oxc(
                                expression_text.as_ref(),
                                expression_start,
                                line,
                                column,
                            )
                            .or_else(|| {
                                parse_modern_expression_from_text(
                                    expression_text.as_ref(),
                                    expression_start,
                                    line,
                                    column,
                                )
                            });
                    }
                }
                "attribute_name" => directive = parse_directive_head(source, attr_child),
                "quoted_attribute_value" => {
                    attribute_value_syntax = modern::AttributeValueSyntax::Quoted;
                    if error.is_none() {
                        error = detect_attr_error_in_value(source, attr_child);
                    }
                    if name.as_ref() == "generics" {
                        let value_start = attr_child.start_byte().saturating_add(1);
                        let value_end = attr_child.end_byte().saturating_sub(1);
                        let raw_value = source.get(value_start..value_end).unwrap_or_default();
                        value_parts.push(modern::AttributeValue::Text(modern::Text {
                            start: value_start,
                            end: value_end,
                            raw: Arc::from(raw_value),
                            data: Arc::from(raw_value),
                        }));
                        continue;
                    }

                    let parts = collect_modern_attribute_value_parts(source, attr_child, loose);
                    extend_modern_expression_values(
                        &parts,
                        &mut expression_values,
                        &mut has_expression,
                    );
                    value_parts.extend(parts);

                    if value_parts.is_empty() {
                        let quote_start = attr_child.start_byte().saturating_add(1);
                        value_parts.push(modern::AttributeValue::Text(modern::Text {
                            start: quote_start,
                            end: quote_start,
                            raw: Arc::from(""),
                            data: Arc::from(""),
                        }));
                    }
                }
                "unquoted_attribute_value" => {
                    attribute_value_syntax = modern::AttributeValueSyntax::Unquoted;
                    if error.is_none() {
                        error = detect_attr_error_in_value(source, attr_child);
                    }
                    let parts = collect_modern_attribute_value_parts(source, attr_child, loose);
                    extend_modern_expression_values(
                        &parts,
                        &mut expression_values,
                        &mut has_expression,
                    );

                    if !matches!(
                        directive_value_syntax,
                        modern::DirectiveValueSyntax::Invalid
                    ) {
                        if attribute_value_is_single_expression(&parts) {
                            directive_value_syntax = modern::DirectiveValueSyntax::Expression;
                        } else if let Some(start) = first_modern_attribute_value_start(&parts) {
                            directive_value_syntax = modern::DirectiveValueSyntax::Invalid;
                            directive_value_start = start;
                        }
                    }
                    value_parts.extend(parts);
                }
                "attribute_value" => {
                    attribute_value_syntax = modern::AttributeValueSyntax::Unquoted;
                    if !matches!(
                        directive_value_syntax,
                        modern::DirectiveValueSyntax::Invalid
                    ) {
                        let raw_value = text_for_node(source, attr_child);
                        if !raw_value.trim().is_empty() {
                            directive_value_syntax = modern::DirectiveValueSyntax::Invalid;
                            directive_value_start = attr_child.start_byte();
                        }
                    }
                    value_parts.push(modern::AttributeValue::Text(parse_modern_text(
                        source, attr_child,
                    )));
                }
                "expression" => {
                    attribute_value_syntax = modern::AttributeValueSyntax::Expression;
                    if error.is_none() {
                        error = detect_attr_error_in_expression(source, attr_child);
                    }
                    if !matches!(
                        directive_value_syntax,
                        modern::DirectiveValueSyntax::Invalid
                    ) {
                        directive_value_syntax = modern::DirectiveValueSyntax::Expression;
                    }
                    let tag = if loose {
                        Some(crate::api::modern::parse_modern_expression_tag_loose(
                            source, attr_child,
                        ))
                    } else {
                        parse_modern_expression_tag(source, attr_child)
                    };
                    if let Some(tag) = tag {
                        has_expression = true;
                        expression_values.push(tag.clone());
                        value_parts.push(modern::AttributeValue::ExpressionTag(tag));
                    }
                }
                "shorthand_attribute" => {
                    attribute_value_syntax = modern::AttributeValueSyntax::Expression;
                    let raw = text_for_node(source, attr_child);
                    if let Some(inner) = raw
                        .as_ref()
                        .strip_prefix('{')
                        .and_then(|text| text.strip_suffix('}'))
                    {
                        let trimmed = inner.trim();
                        if !trimmed.is_empty() {
                            let leading = inner.find(trimmed).unwrap_or(0);
                            let expression_start = attr_child.start_byte() + 1 + leading;
                            let (line, column) = line_column_at_offset(source, expression_start);
                            let expression = parse_modern_expression_from_text(
                                trimmed,
                                expression_start,
                                line,
                                column,
                            )
                            .unwrap_or_else(|| modern_empty_identifier_expression(attr_child));

                            if estree_node_type(&expression.0) == Some("Identifier")
                                && let Some(identifier_name) =
                                    estree_node_field_str(&expression.0, RawField::Name)
                            {
                                let identifier_name = identifier_name.to_string();
                                name = Arc::from(identifier_name.as_str());
                                let start = estree_value_to_usize(estree_node_field(
                                    &expression.0,
                                    RawField::Start,
                                ))
                                .unwrap_or(expression_start);
                                let end = estree_value_to_usize(estree_node_field(
                                    &expression.0,
                                    RawField::End,
                                ))
                                .unwrap_or(start + identifier_name.len());
                                name_loc = legacy::NameLocation {
                                    start: source_location_at_offset(source, start),
                                    end: source_location_at_offset(source, end),
                                };
                            }

                            let tag = modern::ExpressionTag {
                                r#type: modern::ExpressionTagType::ExpressionTag,
                                start: attr_child.start_byte(),
                                end: attr_child.end_byte(),
                                expression,
                            };
                            has_expression = true;
                            expression_values.push(tag.clone());
                            value_parts.push(modern::AttributeValue::ExpressionTag(tag));
                        }
                    }
                }
                "ERROR" if error.is_none() => {
                    if name_node.is_some() && child_has_attribute_value(child) {
                        let position = attr_child.start_byte();
                        error = Some(modern::AttrError {
                            kind: modern::AttrErrorKind::ExpectedEquals,
                            start: position,
                            end: position,
                        });
                    }
                }
                "attach_tag" => {
                    let raw = attr_child
                        .utf8_text(source.as_bytes())
                        .ok()
                        .unwrap_or_default();
                    if let Some(expr_text) = raw
                        .strip_prefix("{@attach")
                        .and_then(|tail| tail.strip_suffix('}'))
                        .map(str::trim)
                    {
                        let expression = parse_modern_expression_from_text(
                            expr_text,
                            attr_child.start_byte() + (raw.find(expr_text).unwrap_or(0)),
                            attr_child.start_position().row + 1,
                            attr_child.start_position().column + raw.find(expr_text).unwrap_or(0),
                        )
                        .unwrap_or_else(|| modern_empty_identifier_expression(attr_child));
                        out.push(modern::Attribute::AttachTag(modern::AttachTag {
                            start: attribute_start,
                            end: child.end_byte(),
                            expression,
                        }));
                        has_attach_tag = true;
                    }
                }
                _ => {}
            }
        }

        if has_attach_tag {
            continue;
        }

        if error.is_none()
            && value_parts.is_empty()
            && !has_expression
            && let Some(start) = missing_attribute_value_start(source, tag_node, child)
        {
            error = Some(modern::AttrError {
                kind: modern::AttrErrorKind::ExpectedValue,
                start,
                end: start,
            });
        }

        if let Some(expression) = spread_expression {
            out.push(modern::Attribute::SpreadAttribute(
                modern::SpreadAttribute {
                    start: child.start_byte(),
                    end: child.end_byte(),
                    expression,
                },
            ));
            continue;
        }

        if let Some(head) = directive {
            if !value_parts.is_empty() {
                let is_single_expression = value_parts.len() == 1
                    && matches!(
                        value_parts.first(),
                        Some(modern::AttributeValue::ExpressionTag(_))
                    );

                if is_single_expression {
                    if !matches!(
                        directive_value_syntax,
                        modern::DirectiveValueSyntax::Invalid
                    ) {
                        directive_value_syntax = modern::DirectiveValueSyntax::Expression;
                    }
                } else if !matches!(
                    directive_value_syntax,
                    modern::DirectiveValueSyntax::Invalid
                ) {
                    let first_value_start = match value_parts.first() {
                        Some(modern::AttributeValue::Text(text)) => text.start,
                        Some(modern::AttributeValue::ExpressionTag(tag)) => tag.start,
                        None => 0,
                    };
                    directive_value_syntax = modern::DirectiveValueSyntax::Invalid;
                    directive_value_start = first_value_start;
                }
            }

            if matches!(head.kind, Some(DirectiveKind::Bind | DirectiveKind::Class))
                && !head.name.is_empty()
                && expression_values.is_empty()
                && !matches!(
                    directive_value_syntax,
                    modern::DirectiveValueSyntax::Invalid
                )
            {
                directive_value_syntax = modern::DirectiveValueSyntax::Expression;
            }

            let expression = expression_values
                .first()
                .map(|tag| tag.expression.clone())
                .or_else(|| {
                    (matches!(head.kind, Some(DirectiveKind::Bind | DirectiveKind::Class))
                        && !head.name.is_empty())
                    .then(|| {
                        shorthand_directive_identifier_expression(
                            source,
                            name_node,
                            &head,
                            &name_loc,
                            child.start_byte(),
                            child.end_byte(),
                        )
                    })
                    .flatten()
                })
                .unwrap_or_else(|| modern_empty_identifier_expression(child));

            let directive = modern::DirectiveAttribute {
                start: attribute_start,
                end: child.end_byte(),
                name: head.name,
                name_loc: name_loc.clone(),
                expression,
                modifiers: head.modifiers,
                value_syntax: directive_value_syntax,
                value_start: directive_value_start,
            };

            match head.kind {
                Some(DirectiveKind::On) => out.push(modern::Attribute::OnDirective(directive)),
                Some(DirectiveKind::Bind) => out.push(modern::Attribute::BindDirective(directive)),
                Some(DirectiveKind::Class) => {
                    out.push(modern::Attribute::ClassDirective(directive))
                }
                Some(DirectiveKind::Let) => out.push(modern::Attribute::LetDirective(directive)),
                Some(DirectiveKind::Style) => {
                    let value = if value_parts.is_empty() {
                        modern::AttributeValueList::Boolean(true)
                    } else if value_parts.len() == 1 {
                        match value_parts.first() {
                            Some(modern::AttributeValue::ExpressionTag(tag)) => {
                                modern::AttributeValueList::ExpressionTag(tag.clone())
                            }
                            _ => modern::AttributeValueList::Values(value_parts.into_boxed_slice()),
                        }
                    } else {
                        modern::AttributeValueList::Values(value_parts.into_boxed_slice())
                    };
                    out.push(modern::Attribute::StyleDirective(modern::StyleDirective {
                        start: attribute_start,
                        end: child.end_byte(),
                        name: directive.name,
                        name_loc,
                        modifiers: directive.modifiers,
                        value,
                        value_syntax: attribute_value_syntax,
                    }));
                }
                Some(kind) if kind.is_transition() => {
                    out.push(modern::Attribute::TransitionDirective(
                        modern::TransitionDirective {
                            start: directive.start,
                            end: directive.end,
                            name: directive.name,
                            name_loc: directive.name_loc,
                            expression: directive.expression,
                            modifiers: directive.modifiers,
                            intro: kind.is_intro(),
                            outro: kind.is_outro(),
                            value_syntax: directive.value_syntax,
                            value_start: directive.value_start,
                        },
                    ));
                }
                Some(DirectiveKind::Animate) => {
                    out.push(modern::Attribute::AnimateDirective(directive))
                }
                Some(DirectiveKind::Use) => out.push(modern::Attribute::UseDirective(directive)),
                _ => {
                    out.push(modern::Attribute::Attribute(modern::NamedAttribute {
                        start: attribute_start,
                        end: child.end_byte(),
                        name: name.clone(),
                        name_loc,
                        value: modern::AttributeValueList::Values(value_parts.into_boxed_slice()),
                        value_syntax: attribute_value_syntax,
                        error,
                    }));
                }
            }
            continue;
        }

        if let Some(name_node) = name_node {
            let text = text_for_node(source, name_node);
            if !text.is_empty() {
                name = text;
                name_loc = legacy::NameLocation {
                    start: source_location_from_point(
                        source,
                        name_node.start_position(),
                        name_node.start_byte(),
                    ),
                    end: source_location_from_point(
                        source,
                        name_node.end_position(),
                        name_node.end_byte(),
                    ),
                };
            }
        }

        let value = if has_expression && value_parts.len() == 1 {
            match value_parts.pop() {
                Some(modern::AttributeValue::ExpressionTag(tag)) => {
                    modern::AttributeValueList::ExpressionTag(tag)
                }
                Some(other) => modern::AttributeValueList::Values(vec![other].into_boxed_slice()),
                None => modern::AttributeValueList::Boolean(true),
            }
        } else if name.is_empty() && value_parts.is_empty() {
            let expr_pos = child
                .start_byte()
                .saturating_add(1)
                .min(child.end_byte().saturating_sub(1));
            let (line, column) = line_column_at_offset(source, expr_pos);
            modern::AttributeValueList::ExpressionTag(modern::ExpressionTag {
                r#type: modern::ExpressionTagType::ExpressionTag,
                start: expr_pos,
                end: expr_pos,
                expression: modern_identifier_expression_with_loc(
                    Arc::from(""),
                    expr_pos,
                    expr_pos,
                    Some(modern::Loc {
                        start: modern::Position {
                            line,
                            column,
                            character: Some(expr_pos),
                        },
                        end: modern::Position {
                            line,
                            column,
                            character: Some(expr_pos),
                        },
                    }),
                ),
            })
        } else if !value_parts.is_empty() {
            modern::AttributeValueList::Values(value_parts.into_boxed_slice())
        } else {
            modern::AttributeValueList::Boolean(true)
        };

        out.push(modern::Attribute::Attribute(modern::NamedAttribute {
            start: attribute_start,
            end: child.end_byte(),
            name,
            name_loc,
            value,
            value_syntax: attribute_value_syntax,
            error,
        }));
    }

    out
}

fn merge_trailing_expression_brace(
    source: &str,
    error_node: Node<'_>,
    out: &mut [modern::Attribute],
) -> bool {
    let raw = text_for_node(source, error_node);
    if raw.as_ref() != "}" {
        return false;
    }

    let Some(modern::Attribute::Attribute(attribute)) = out.last_mut() else {
        return false;
    };
    let modern::AttributeValueList::ExpressionTag(tag) = &attribute.value else {
        return false;
    };

    attribute.value = modern::AttributeValueList::Values(
        vec![
            modern::AttributeValue::ExpressionTag(tag.clone()),
            modern::AttributeValue::Text(modern::Text {
                start: error_node.start_byte(),
                end: error_node.end_byte(),
                raw: Arc::from("}"),
                data: Arc::from("}"),
            }),
        ]
        .into_boxed_slice(),
    );
    attribute.value_syntax = modern::AttributeValueSyntax::Unquoted;
    attribute.end = error_node.end_byte();
    true
}

fn missing_attribute_value_start(
    source: &str,
    tag_node: Node<'_>,
    attribute_node: Node<'_>,
) -> Option<usize> {
    let mut found_attribute = false;
    let mut saw_equals = false;

    for index in 0..tag_node.child_count() {
        let child = tag_node.child(index as u32)?;

        if !found_attribute {
            if child.id() == attribute_node.id() {
                found_attribute = true;
            }
            continue;
        }

        match child.kind() {
            "=" if !saw_equals => saw_equals = true,
            "ERROR"
                if !saw_equals
                    && child.utf8_text(source.as_bytes()).ok().unwrap_or_default() == "=" =>
            {
                saw_equals = true;
            }
            ">" | "/>" if saw_equals => return Some(child.start_byte()),
            _ if child.is_named() => return None,
            _ => {}
        }
    }

    None
}

fn recover_modern_invalid_attribute_from_error(
    source: &str,
    error_node: Node<'_>,
) -> Option<(Arc<str>, usize, usize)> {
    let raw = text_for_node(source, error_node);
    let raw_ref = raw.as_ref();
    let trimmed = raw_ref.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    let leading = raw_ref.len().saturating_sub(trimmed.len());
    let token_len = trimmed
        .chars()
        .take_while(|ch| !ch.is_ascii_whitespace() && !matches!(ch, '=' | '>' | '/'))
        .map(char::len_utf8)
        .sum::<usize>();
    if token_len == 0 {
        return None;
    }

    let token = trimmed.get(..token_len).unwrap_or_default();
    if token.is_empty() {
        return None;
    }
    if !looks_like_invalid_attribute_candidate(token) {
        return None;
    }

    let start = error_node.start_byte().saturating_add(leading);
    let end = start.saturating_add(token_len);
    Some((Arc::from(token), start, end))
}

fn looks_like_invalid_attribute_candidate(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|ch| {
            !ch.is_ascii_whitespace() && !matches!(ch, '<' | '>' | '"' | '\'' | '=' | '/')
        })
}

fn child_has_attribute_value(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|child| {
        matches!(
            child.kind(),
            "quoted_attribute_value"
                | "unquoted_attribute_value"
                | "attribute_value"
                | "expression"
        )
    })
}

fn detect_attr_error_in_value(source: &str, node: Node<'_>) -> Option<modern::AttrError> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        if kind == "html_tag" {
            return Some(modern::AttrError {
                kind: modern::AttrErrorKind::HtmlTag,
                start: child.start_byte(),
                end: child.start_byte(),
            });
        }
        if super::modern::is_typed_block_kind(kind) {
            let block_name = Arc::from(match kind {
                "if_block" => "if",
                "each_block" => "each",
                "await_block" => "await",
                "key_block" => "key",
                "snippet_block" => "snippet",
                _ => "if",
            });
            return Some(modern::AttrError {
                kind: modern::AttrErrorKind::Block(block_name),
                start: child.start_byte(),
                end: child.start_byte(),
            });
        }
        if kind == "expression"
            && let Some(error) = detect_attr_error_in_expression(source, child)
        {
            return Some(error);
        }
    }
    None
}

fn detect_attr_error_in_expression(_source: &str, _node: Node<'_>) -> Option<modern::AttrError> {
    None
}

fn parse_legacy_text(source: &str, node: Node<'_>) -> legacy::Text {
    let raw = text_for_node(source, node);
    let data = decode_html_entities(raw.as_ref());

    legacy::Text {
        start: node.start_byte(),
        end: node.end_byte(),
        raw: Some(raw),
        data,
    }
}

fn parse_modern_text(source: &str, node: Node<'_>) -> modern::Text {
    let raw = text_for_node(source, node);
    let data = decode_html_entities(raw.as_ref());

    modern::Text {
        start: node.start_byte(),
        end: node.end_byte(),
        raw,
        data,
    }
}

fn push_modern_attribute_value_part(
    source: &str,
    node: Node<'_>,
    value_parts: &mut Vec<modern::AttributeValue>,
    loose: bool,
) {
    match node.kind() {
        "attribute_value" | "entity" => {
            value_parts.push(modern::AttributeValue::Text(parse_modern_text(
                source, node,
            )));
        }
        "expression" => {
            if loose {
                value_parts.push(modern::AttributeValue::ExpressionTag(
                    crate::api::modern::parse_modern_expression_tag_loose(source, node),
                ));
            } else if let Some(tag) = parse_modern_expression_tag(source, node) {
                value_parts.push(modern::AttributeValue::ExpressionTag(tag));
            }
        }
        _ => {}
    }
}

fn collect_modern_attribute_value_parts(
    source: &str,
    node: Node<'_>,
    loose: bool,
) -> Vec<modern::AttributeValue> {
    let mut value_parts = Vec::new();

    match node.kind() {
        "quoted_attribute_value" | "unquoted_attribute_value" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                push_modern_attribute_value_part(source, child, &mut value_parts, loose);
            }
        }
        _ => push_modern_attribute_value_part(source, node, &mut value_parts, loose),
    }

    value_parts
}

fn first_modern_attribute_value_start(values: &[modern::AttributeValue]) -> Option<usize> {
    match values.first()? {
        modern::AttributeValue::Text(text) => Some(text.start),
        modern::AttributeValue::ExpressionTag(tag) => Some(tag.start),
    }
}

fn attribute_value_is_single_expression(values: &[modern::AttributeValue]) -> bool {
    matches!(values, [modern::AttributeValue::ExpressionTag(_)])
}

fn extend_modern_expression_values(
    values: &[modern::AttributeValue],
    expression_values: &mut Vec<modern::ExpressionTag>,
    has_expression: &mut bool,
) {
    for value in values {
        if let modern::AttributeValue::ExpressionTag(tag) = value {
            *has_expression = true;
            expression_values.push(tag.clone());
        }
    }
}

fn parse_legacy_comment(source: &str, node: Node<'_>) -> legacy::Comment {
    let raw = text_for_node(source, node);
    let data_text = raw
        .strip_prefix("<!--")
        .and_then(|inner| inner.strip_suffix("-->"))
        .unwrap_or(raw.as_ref());

    let ignores = parse_svelte_ignores(data_text);

    legacy::Comment {
        start: node.start_byte(),
        end: node.end_byte(),
        data: Arc::from(data_text),
        ignores,
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectiveKind {
    On,
    Bind,
    Class,
    Let,
    Style,
    Transition,
    In,
    Out,
    Animate,
    Use,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectiveHead {
    prefix: Arc<str>,
    kind: Option<DirectiveKind>,
    name: Arc<str>,
    modifiers: Box<[Arc<str>]>,
}

impl DirectiveKind {
    fn is_transition(self) -> bool {
        matches!(self, Self::Transition | Self::In | Self::Out)
    }

    fn is_intro(self) -> bool {
        !matches!(self, Self::Out)
    }

    fn is_outro(self) -> bool {
        !matches!(self, Self::In)
    }
}

impl std::str::FromStr for DirectiveKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "on" => Ok(Self::On),
            "bind" => Ok(Self::Bind),
            "class" => Ok(Self::Class),
            "let" => Ok(Self::Let),
            "style" => Ok(Self::Style),
            "transition" => Ok(Self::Transition),
            "in" => Ok(Self::In),
            "out" => Ok(Self::Out),
            "animate" => Ok(Self::Animate),
            "use" => Ok(Self::Use),
            _ => Err(()),
        }
    }
}

fn collect_named_descendants<'a>(node: Node<'a>, kind: &str, out: &mut Vec<Node<'a>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            out.push(child);
        }
        collect_named_descendants(child, kind, out);
    }
}

fn first_named_descendant<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut out = Vec::new();
    collect_named_descendants(node, kind, &mut out);
    out.into_iter().next()
}

fn trimmed_node_text(source: &str, node: Node<'_>) -> Option<(Arc<str>, usize)> {
    let raw = text_for_node(source, node);
    let raw = raw.as_ref();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let leading = raw.find(trimmed).unwrap_or(0);
    Some((Arc::from(trimmed), node.start_byte() + leading))
}

fn spread_attribute_expression_text(source: &str, node: Node<'_>) -> Option<(Arc<str>, usize)> {
    if let Some(content) = node.child_by_field_name("content") {
        return trimmed_node_text(source, content);
    }

    let raw = text_for_node(source, node);
    let raw = raw.as_ref();
    let inner = raw.strip_prefix("{...")?.strip_suffix('}')?;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    let leading = inner.find(trimmed).unwrap_or(0);
    Some((Arc::from(trimmed), node.start_byte() + 4 + leading))
}

fn parse_directive_head(source: &str, name_node: Node<'_>) -> Option<DirectiveHead> {
    let prefix = first_named_descendant(name_node, "attribute_directive");
    let name = first_named_descendant(name_node, "attribute_identifier");
    if let (Some(prefix), Some(name)) = (prefix, name) {
        let prefix = text_for_node(source, prefix);
        let name = text_for_node(source, name);
        let mut modifier_nodes = Vec::new();
        collect_named_descendants(name_node, "attribute_modifier", &mut modifier_nodes);
        let modifiers = modifier_nodes
            .into_iter()
            .map(|node| text_for_node(source, node))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        return Some(DirectiveHead {
            kind: prefix.as_ref().parse().ok(),
            prefix,
            name,
            modifiers,
        });
    }

    parse_directive_head_from_text(text_for_node(source, name_node).as_ref())
}

fn parse_directive_head_from_text(value: &str) -> Option<DirectiveHead> {
    let mut parts = value.split('|');
    let first = parts.next()?;
    let (prefix, name) = first.split_once(':')?;
    let modifiers = parts.map(Arc::from).collect::<Vec<_>>().into_boxed_slice();

    Some(DirectiveHead {
        prefix: Arc::from(prefix),
        kind: prefix.parse().ok(),
        name: Arc::from(name),
        modifiers,
    })
}

fn shorthand_directive_identifier_expression(
    source: &str,
    name_node: Option<Node<'_>>,
    head: &DirectiveHead,
    fallback_loc: &legacy::NameLocation,
    fallback_start: usize,
    fallback_end: usize,
) -> Option<modern::Expression> {
    if head.name.is_empty() {
        return None;
    }

    if let Some(name_node) = name_node {
        if let Some(identifier_node) = first_named_descendant(name_node, "attribute_identifier")
            && let Some((binding_name, name_abs)) = trimmed_node_text(source, identifier_node)
        {
            let (line, column) = line_column_at_offset(source, name_abs);
            if let Some(parsed) =
                parse_modern_expression_from_text(binding_name.as_ref(), name_abs, line, column)
            {
                return Some(parsed);
            }
        }

        let raw = text_for_node(source, name_node);
        let raw = raw.as_ref();
        if let Some(colon_idx) = raw.find(':') {
            let after_colon = &raw[(colon_idx + 1)..];
            let binding_name = after_colon.split('|').next().unwrap_or(after_colon).trim();
            if !binding_name.is_empty() {
                let name_rel = colon_idx + 1 + after_colon.find(binding_name).unwrap_or(0);
                let name_abs = name_node.start_byte() + name_rel;
                let (line, column) = line_column_at_offset(source, name_abs);
                if let Some(parsed) =
                    parse_modern_expression_from_text(binding_name, name_abs, line, column)
                {
                    return Some(parsed);
                }
            }
        }
    }

    Some(modern_identifier_expression_with_loc(
        head.name.clone(),
        fallback_start,
        fallback_end,
        Some(modern::Loc {
            start: modern::Position {
                line: fallback_loc.start.line,
                column: fallback_loc.start.column,
                character: None,
            },
            end: modern::Position {
                line: fallback_loc.end.line,
                column: fallback_loc.end.column,
                character: None,
            },
        }),
    ))
}

fn legacy_empty_identifier_expression_for_node(
    source: &str,
    node: Node<'_>,
    include_character: bool,
) -> legacy::Expression {
    let raw = node.utf8_text(source.as_bytes()).ok().unwrap_or_default();
    let trimmed = raw.trim();

    let (start, end) = if trimmed.is_empty() {
        // For zero-width nodes (empty expressions), use start_byte directly.
        // For non-zero-width nodes with only whitespace, end_byte - 1 gives the
        // position of the last character. Both are valid fallback positions.
        let pos = if node.start_byte() == node.end_byte() {
            node.start_byte()
        } else {
            node.end_byte().saturating_sub(1)
        };
        (pos, pos)
    } else {
        let leading = raw.find(trimmed).unwrap_or(0);
        let start = node.start_byte() + leading;
        (start, start + trimmed.len())
    };

    let loc = if include_character {
        let (start_line, start_column) = line_column_at_offset(source, start);
        let (end_line, end_column) = line_column_at_offset(source, end);
        Some(legacy::ExpressionLoc {
            start: legacy::ExpressionPoint {
                line: start_line,
                column: start_column,
                character: Some(start),
            },
            end: legacy::ExpressionPoint {
                line: end_line,
                column: end_column,
                character: Some(end),
            },
        })
    } else {
        None
    };

    legacy_empty_identifier_expression(start, end, loc)
}

fn parse_legacy_binding_field(
    source: &str,
    node: Node<'_>,
    include_character: bool,
) -> Option<legacy::Expression> {
    let raw = node.utf8_text(source.as_bytes()).ok()?;
    parse_legacy_pattern_from_text(
        raw,
        node.start_byte(),
        node.start_position().row + 1,
        node.start_position().column,
    )
    .or_else(|| {
        parse_legacy_expression_from_text(
            raw,
            node.start_byte(),
            node.start_position().row + 1,
            node.start_position().column,
            include_character,
        )
    })
}

fn parse_legacy_block(source: &str, block: Node<'_>) -> Option<legacy::Node> {
    let children = named_children_vec(block);

    match block.kind() {
        "if_block" => parse_legacy_if_block(source, block, &children).map(legacy::Node::IfBlock),
        "each_block" => {
            parse_legacy_each_block(source, block, &children).map(legacy::Node::EachBlock)
        }
        "key_block" => parse_legacy_key_block(source, block, &children).map(legacy::Node::KeyBlock),
        "await_block" => {
            parse_legacy_await_block(source, block, &children).map(legacy::Node::AwaitBlock)
        }
        "snippet_block" => {
            parse_legacy_snippet_block(source, block, &children).map(legacy::Node::SnippetBlock)
        }
        _ => None,
    }
}

fn parse_legacy_if_block(
    source: &str,
    block: Node<'_>,
    children: &[Node<'_>],
) -> Option<legacy::IfBlock> {
    let end_idx = children
        .iter()
        .rposition(|node| node.kind() == "block_end")
        .unwrap_or(children.len());
    let expression_node = block.child_by_field_name("expression");
    let expression = expression_node
        .map(|expr| {
            parse_legacy_expression(source, expr, false)
                .unwrap_or_else(|| legacy_empty_identifier_expression_for_node(source, expr, false))
        })
        .unwrap_or_else(|| {
            legacy_empty_identifier_expression(
                block.end_byte().saturating_sub(1),
                block.end_byte().saturating_sub(1),
                None,
            )
        });

    let body_start = super::modern::body_start_index(block, children, &["expression"]);

    // Find branch indices (else_if_clause, else_clause)
    let branch_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            matches!(node.kind(), "else_if_clause" | "else_clause").then_some(idx)
        })
        .collect();

    let (block_children, else_block) = if branch_indices.is_empty() {
        (
            trim_legacy_block_children(parse_legacy_nodes_slice(
                source,
                &children[body_start..end_idx],
            )),
            None,
        )
    } else {
        let consequent_end = branch_indices.first().copied().unwrap_or(end_idx);
        (
            trim_legacy_block_children(parse_legacy_nodes_slice(
                source,
                &children[body_start..consequent_end],
            )),
            parse_legacy_else_for_if_typed(source, children, &branch_indices, 0, end_idx),
        )
    };

    Some(legacy::IfBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        children: block_children.into_boxed_slice(),
        else_block,
        elseif: None,
    })
}

fn parse_legacy_if_from_branch_typed(
    source: &str,
    children: &[Node<'_>],
    branch_indices: &[usize],
    branch_index: usize,
    end_idx: usize,
) -> Option<legacy::IfBlock> {
    let branch_child_idx = *branch_indices.get(branch_index)?;
    let branch = *children.get(branch_child_idx)?;
    if branch.kind() != "else_if_clause" {
        return None;
    }

    // else_if_clause has expression field via expression_value child
    let expression = find_first_named_child(branch, "expression_value")
        .map(|expr| {
            parse_legacy_expression(source, expr, false)
                .unwrap_or_else(|| legacy_empty_identifier_expression_for_node(source, expr, false))
        })
        .unwrap_or_else(|| {
            legacy_empty_identifier_expression(
                branch.end_byte().saturating_sub(1),
                branch.end_byte().saturating_sub(1),
                None,
            )
        });

    // Body nodes are children of the clause node
    let clause_children = named_children_vec(branch);
    let clause_body_start = clause_children
        .iter()
        .position(|c| c.kind() == "expression_value")
        .map(|i| i + 1)
        .unwrap_or(0);
    let block_children = trim_legacy_block_children(parse_legacy_nodes_slice(
        source,
        &clause_children[clause_body_start..],
    ));

    let else_block = if branch_index + 1 < branch_indices.len() {
        parse_legacy_else_for_if_typed(source, children, branch_indices, branch_index + 1, end_idx)
    } else {
        None
    };

    // For else_if blocks, start is after the clause header (the closing } of {:else if ...})
    let body_start_byte = clause_children
        .get(clause_body_start)
        .map(|n| n.start_byte())
        .unwrap_or(branch.end_byte());

    Some(legacy::IfBlock {
        start: body_start_byte,
        end: children
            .get(end_idx)
            .map(|n| n.end_byte())
            .unwrap_or(branch.end_byte()),
        expression,
        children: block_children.into_boxed_slice(),
        else_block,
        elseif: Some(true),
    })
}

fn parse_legacy_else_for_if_typed(
    source: &str,
    children: &[Node<'_>],
    branch_indices: &[usize],
    branch_index: usize,
    end_idx: usize,
) -> Option<legacy::ElseBlock> {
    let branch_child_idx = *branch_indices.get(branch_index)?;
    let branch = *children.get(branch_child_idx)?;
    let boundary_node = *children.get(end_idx)?;

    let mut else_end = boundary_node.start_byte();
    let branch_children = match branch.kind() {
        "else_if_clause" => {
            let nested = parse_legacy_if_from_branch_typed(
                source,
                children,
                branch_indices,
                branch_index,
                end_idx,
            )?;
            if let Some(nested_else) = nested.else_block.as_ref() {
                else_end = nested_else.end;
            }
            vec![legacy::Node::IfBlock(nested)]
        }
        "else_clause" => {
            // Body nodes are children of the else_clause
            let clause_children = named_children_vec(branch);
            trim_legacy_block_children(parse_legacy_nodes_slice(source, &clause_children))
        }
        _ => return None,
    };

    // For else clauses, start is after the {:else} or {:else if ...} header
    let else_start = match branch.kind() {
        "else_if_clause" => {
            // Body starts after the {:else if ...} header
            let clause_children = named_children_vec(branch);
            let body_idx = clause_children
                .iter()
                .position(|c| c.kind() == "expression_value")
                .map(|i| i + 1)
                .unwrap_or(0);
            clause_children
                .get(body_idx)
                .map(|n| n.start_byte())
                .unwrap_or(branch.end_byte())
        }
        "else_clause" => {
            // Body starts after the {:else} header — use first body child or clause end
            let clause_children = named_children_vec(branch);
            clause_children
                .first()
                .map(|n| n.start_byte())
                .unwrap_or(branch.end_byte())
        }
        _ => branch.end_byte(),
    };

    Some(legacy::ElseBlock {
        r#type: legacy::ElseBlockType::ElseBlock,
        start: else_start,
        end: else_end,
        children: branch_children.into_boxed_slice(),
    })
}

fn fill_legacy_range_gaps(
    source: &str,
    start: usize,
    end: usize,
    mut nodes: Vec<legacy::Node>,
) -> Vec<legacy::Node> {
    nodes.sort_by_key(legacy_node_start);
    let mut out = Vec::new();
    let mut consumed = start;

    for node in nodes {
        let node_start = legacy_node_start(&node);
        let node_end = legacy_node_end(&node);

        if node_start > consumed {
            let raw = source.get(consumed..node_start).unwrap_or_default();
            if !raw.is_empty() {
                push_legacy_text_node(
                    &mut out,
                    legacy::Text {
                        start: consumed,
                        end: node_start,
                        raw: Some(Arc::from(raw)),
                        data: decode_html_entities(raw),
                    },
                );
            }
        }

        if node_end <= consumed {
            continue;
        }
        consumed = consumed.max(node_end);
        out.push(node);
    }

    if consumed < end {
        let raw = source.get(consumed..end).unwrap_or_default();
        if !raw.is_empty() {
            push_legacy_text_node(
                &mut out,
                legacy::Text {
                    start: consumed,
                    end,
                    raw: Some(Arc::from(raw)),
                    data: decode_html_entities(raw),
                },
            );
        }
    }

    out
}

fn parse_legacy_each_block(
    source: &str,
    block: Node<'_>,
    children: &[Node<'_>],
) -> Option<legacy::EachBlock> {
    let end_idx = children
        .iter()
        .rposition(|node| node.kind() == "block_end")
        .unwrap_or(children.len());

    let expression = block
        .child_by_field_name("expression")
        .map(|expr_node| {
            parse_legacy_expression(source, expr_node, false).unwrap_or_else(|| {
                legacy_empty_identifier_expression_for_node(source, expr_node, false)
            })
        })
        .unwrap_or_else(|| {
            legacy_empty_identifier_expression(
                block.end_byte().saturating_sub(1),
                block.end_byte().saturating_sub(1),
                None,
            )
        });

    let context = block
        .child_by_field_name("binding")
        .and_then(|binding_node| parse_legacy_binding_field(source, binding_node, true));

    let key = block.child_by_field_name("key").map(|key_node| {
        parse_legacy_expression(source, key_node, false)
            .unwrap_or_else(|| legacy_empty_identifier_expression_for_node(source, key_node, false))
    });

    let index = block
        .child_by_field_name("index")
        .map(|index_node| Arc::from(text_for_node(source, index_node).trim()));

    let body_start = super::modern::body_start_index(
        block,
        children,
        &["expression", "binding", "index", "key"],
    );

    // Find else_clause indices
    let else_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| (node.kind() == "else_clause").then_some(idx))
        .collect();

    let consequent_end = else_indices.first().copied().unwrap_or(end_idx);
    let block_children = trim_legacy_block_children(parse_legacy_nodes_slice(
        source,
        &children[body_start..consequent_end],
    ));

    let else_block = if let Some(&else_idx) = else_indices.first() {
        let else_node = children[else_idx];
        let boundary_node = children.get(end_idx)?;
        let clause_children = named_children_vec(else_node);
        let else_start = clause_children
            .first()
            .map(|n| n.start_byte())
            .unwrap_or(else_node.end_byte());
        Some(legacy::ElseBlock {
            r#type: legacy::ElseBlockType::ElseBlock,
            start: else_start,
            end: boundary_node.start_byte(),
            children: trim_legacy_block_children(parse_legacy_nodes_slice(
                source,
                &clause_children,
            ))
            .into_boxed_slice(),
        })
    } else {
        None
    };

    Some(legacy::EachBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        children: block_children.into_boxed_slice(),
        context,
        expression,
        index,
        key,
        else_block,
    })
}

fn parse_legacy_key_block(
    source: &str,
    block: Node<'_>,
    children: &[Node<'_>],
) -> Option<legacy::KeyBlock> {
    let end_idx = children
        .iter()
        .rposition(|node| node.kind() == "block_end")
        .unwrap_or(children.len());
    let expression_node = block.child_by_field_name("expression");
    let expression = expression_node
        .map(|expr| {
            parse_legacy_expression(source, expr, false)
                .unwrap_or_else(|| legacy_empty_identifier_expression_for_node(source, expr, false))
        })
        .unwrap_or_else(|| {
            legacy_empty_identifier_expression(
                block.end_byte().saturating_sub(1),
                block.end_byte().saturating_sub(1),
                None,
            )
        });

    let body_start = super::modern::body_start_index(block, children, &["expression"]);
    let parsed_children = trim_legacy_block_children(parse_legacy_nodes_slice(
        source,
        &children[body_start..end_idx],
    ));

    Some(legacy::KeyBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        children: parsed_children.into_boxed_slice(),
    })
}

fn parse_legacy_await_block(
    source: &str,
    block: Node<'_>,
    children: &[Node<'_>],
) -> Option<legacy::AwaitBlock> {
    let end_idx = children
        .iter()
        .rposition(|node| node.kind() == "block_end")
        .unwrap_or(children.len());

    let expression_node = block.child_by_field_name("expression");
    let parsed_expression =
        expression_node.and_then(|expr| parse_legacy_expression(source, expr, false));
    let expression_needs_recovery = expression_node.is_some() && parsed_expression.is_none();
    let expression = match (expression_node, parsed_expression) {
        (Some(_expr), Some(expression)) => expression,
        (Some(expr), None) => legacy_empty_identifier_expression_for_node(source, expr, false),
        (None, None) => legacy_empty_identifier_expression(
            block.end_byte().saturating_sub(1),
            block.end_byte().saturating_sub(1),
            None,
        ),
        (None, Some(_)) => unreachable!("expression parse without an expression node"),
    };
    let expression_end_byte = expression_node
        .map(|node| node.end_byte())
        .unwrap_or_else(|| block.end_byte().saturating_sub(1));

    // Check for shorthand form: {#await expr then/catch binding}
    let inline_kind = find_first_named_child(block, "shorthand_kind")
        .and_then(|node| text_for_node(source, node).parse::<BlockBranchKind>().ok())
        .filter(|kind| matches!(kind, BlockBranchKind::Then | BlockBranchKind::Catch));
    let inline_binding = block
        .child_by_field_name("binding")
        .and_then(|node| parse_legacy_binding_field(source, node, true));

    let mut value = matches!(inline_kind, Some(BlockBranchKind::Then))
        .then_some(inline_binding.clone())
        .flatten();
    let mut error = matches!(inline_kind, Some(BlockBranchKind::Catch))
        .then_some(inline_binding)
        .flatten();

    let mut pending = legacy::PendingBlock {
        r#type: legacy::PendingBlockType::PendingBlock,
        start: None,
        end: None,
        children: Box::new([]),
        skip: true,
    };
    let mut then = legacy::ThenBlock {
        r#type: legacy::ThenBlockType::ThenBlock,
        start: None,
        end: None,
        children: Box::new([]),
        skip: true,
    };
    let mut catch = legacy::CatchBlock {
        r#type: legacy::CatchBlockType::CatchBlock,
        start: None,
        end: None,
        children: Box::new([]),
        skip: true,
    };

    let parse_await_children = |node: Node<'_>| parse_legacy_children(source, node, false, 0);

    // Find typed branch indices (await_branch nodes)
    let typed_branch_indices: Vec<usize> = children
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| (node.kind() == "await_branch").then_some(idx))
        .collect();

    let first_branch_idx = typed_branch_indices.first().copied().unwrap_or(end_idx);

    if let Some(kind) = inline_kind {
        // Shorthand form: {#await expr then v}...{/await}
        let shorthand_children = block
            .child_by_field_name("shorthand_children")
            .filter(|node| node.kind() == "await_branch_children");
        let mut inline_children = shorthand_children
            .map(parse_await_children)
            .unwrap_or_default();

        let body_start =
            super::modern::body_start_index(block, children, &["expression", "binding"]);
        if inline_children.is_empty() && first_branch_idx > body_start {
            inline_children =
                parse_legacy_nodes_slice(source, &children[body_start..first_branch_idx]);
        }

        let inline_start = shorthand_children
            .map(|node| node.start_byte())
            .or_else(|| children.get(body_start).map(|node| node.start_byte()))
            .or_else(|| inline_children.first().map(legacy_node_start));
        let recovery_inline_end = (inline_children.is_empty() && expression_needs_recovery)
            .then(|| {
                source[..expression_end_byte]
                    .rfind('}')
                    .map(|index| index + 1)
            })
            .flatten();
        let inline_end = inline_children
            .last()
            .map(legacy_node_end)
            .or(recovery_inline_end)
            .or_else(|| shorthand_children.map(|node| node.end_byte()))
            .or_else(|| children.get(first_branch_idx).map(|node| node.start_byte()))
            .or(inline_start);

        match kind {
            BlockBranchKind::Then => {
                then.start = inline_start;
                then.end = inline_end;
                then.children = inline_children.into_boxed_slice();
                then.skip = false;
            }
            BlockBranchKind::Catch => {
                catch.start = inline_start;
                catch.end = inline_end;
                catch.children = inline_children.into_boxed_slice();
                catch.skip = false;
            }
            _ => {}
        }
    } else {
        // Non-shorthand form: pending content is in the await_pending node
        if let Some(pending_node) = children.iter().find(|n| n.kind() == "await_pending") {
            let pending_children = parse_await_children(*pending_node);
            let p_start = pending_node.start_byte();
            let p_end = pending_node.end_byte();
            pending.start = Some(p_start);
            pending.end = Some(p_end);
            pending.children =
                fill_legacy_range_gaps(source, p_start, p_end, pending_children).into_boxed_slice();
            pending.skip = false;
        } else if typed_branch_indices.is_empty() {
            // No await_pending and no branches: the whole body is pending
            let body_start =
                super::modern::body_start_index(block, children, &["expression", "binding"]);
            let pending_nodes: Vec<legacy::Node> = children
                .iter()
                .take(first_branch_idx)
                .skip(body_start)
                .flat_map(|node| parse_legacy_nodes_slice(source, std::slice::from_ref(node)))
                .collect();
            let p_start = children.get(body_start).map(|n| n.start_byte());
            let p_end = children.get(first_branch_idx).map(|n| n.start_byte());
            if !pending_nodes.is_empty() {
                pending.start = p_start;
                pending.end = p_end;
                pending.children = match (p_start, p_end) {
                    (Some(start), Some(end)) => {
                        fill_legacy_range_gaps(source, start, end, pending_nodes).into_boxed_slice()
                    }
                    _ => pending_nodes.into_boxed_slice(),
                };
                pending.skip = false;
            } else if p_start.is_some() {
                // Empty pending body — still mark as non-skipped with zero-width span
                let pos = p_start.unwrap();
                pending.start = Some(pos);
                pending.end = Some(pos);
                pending.skip = false;
            }
        }
    }

    // Process await_branch nodes ({:then ...} and {:catch ...})
    for branch_child_idx in typed_branch_indices.iter().copied() {
        let branch = *children.get(branch_child_idx)?;

        // Get branch kind from branch_kind node
        let kind = find_first_named_child(branch, "branch_kind")
            .and_then(|n| text_for_node(source, n).parse::<BlockBranchKind>().ok());
        let Some(kind) = kind else {
            continue;
        };

        let binding_node = branch
            .child_by_field_name("binding")
            .or_else(|| branch.child_by_field_name("expression"))
            .or_else(|| branch.child_by_field_name("expression_value"))
            .or_else(|| branch.child_by_field_name("value"));
        let binding_expr =
            binding_node.and_then(|node| parse_legacy_binding_field(source, node, true));

        let branch_children = find_first_named_child(branch, "await_branch_children")
            .map(parse_await_children)
            .unwrap_or_default()
            .into_boxed_slice();
        let next_boundary = typed_branch_indices
            .iter()
            .copied()
            .find(|idx| *idx > branch_child_idx)
            .unwrap_or(end_idx);
        let branch_start = Some(branch.start_byte());
        let branch_end = children.get(next_boundary).map(|node| node.start_byte());

        match kind {
            BlockBranchKind::Then => {
                if value.is_none() {
                    value = binding_expr;
                }
                then.start = branch_start;
                then.end = branch_end;
                then.children = branch_children;
                then.skip = false;
            }
            BlockBranchKind::Catch => {
                if error.is_none() {
                    error = binding_expr;
                }
                catch.start = branch_start;
                catch.end = branch_end;
                catch.children = branch_children;
                catch.skip = false;
            }
            _ => {}
        }
    }

    Some(legacy::AwaitBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        value,
        error,
        pending,
        then,
        catch,
    })
}

fn parse_legacy_snippet_block(
    source: &str,
    block: Node<'_>,
    children: &[Node<'_>],
) -> Option<legacy::SnippetBlock> {
    let end_idx = children
        .iter()
        .rposition(|node| node.kind() == "block_end")
        .unwrap_or(children.len());
    let name_node = block
        .child_by_field_name("name")
        .or_else(|| block.child_by_field_name("expression"));
    let type_params_node = block.child_by_field_name("type_parameters");
    let params_node = block.child_by_field_name("parameters");

    let expression = super::modern::parse_snippet_name(source, block, name_node);
    let expression = legacy_expression_from_raw_node(expression.0, true)?;
    let type_params = super::modern::parse_snippet_type_params(source, type_params_node);
    let parameters = super::modern::parse_snippet_params(source, params_node)
        .into_iter()
        .filter_map(|modern::Expression(raw, _)| legacy_expression_from_raw_node(raw, false))
        .collect::<Vec<_>>();

    let body_start = super::modern::body_start_index(
        block,
        children,
        &["name", "type_parameters", "parameters"],
    );
    let children = trim_legacy_block_children(parse_legacy_nodes_slice(
        source,
        &children[body_start..end_idx],
    ));

    Some(legacy::SnippetBlock {
        start: block.start_byte(),
        end: block.end_byte(),
        expression,
        type_params,
        parameters: parameters.into_boxed_slice(),
        children: children.into_boxed_slice(),
        header_error: None,
    })
}

fn parse_legacy_nodes_slice(source: &str, nodes: &[Node<'_>]) -> Vec<legacy::Node> {
    parse_legacy_nodes_slice_with_depth(source, nodes, 0)
}

fn parse_legacy_nodes_slice_with_depth(
    source: &str,
    nodes: &[Node<'_>],
    recovery_depth: usize,
) -> Vec<legacy::Node> {
    let mut out = Vec::new();
    let mut consumed_until = nodes.first().map(|node| node.start_byte()).unwrap_or(0);
    let mut index = 0usize;

    while index < nodes.len() {
        let node = nodes[index];
        if node.start_byte() < consumed_until {
            index += 1;
            continue;
        }

        match node.kind() {
            "element" => out.push(legacy_node_from_element_with_source(
                source,
                parse_legacy_element(source, node),
            )),
            "doctype" => {
                if let Some(node) = parse_legacy_doctype(source, node) {
                    out.push(node);
                }
            }
            "text" | "entity" => push_legacy_text_node(&mut out, parse_legacy_text(source, node)),
            "raw_text" => push_legacy_text_node(&mut out, parse_legacy_text(source, node)),
            "expression" => {
                if let Some(tag) = parse_mustache_tag(source, node) {
                    out.push(legacy::Node::MustacheTag(tag));
                }
            }
            kind if super::modern::is_typed_tag_kind(kind) => {
                if let Some(tag) = parse_legacy_tag(source, node) {
                    out.push(tag);
                }
            }
            "comment" => out.push(legacy::Node::Comment(parse_legacy_comment(source, node))),
            kind if super::modern::is_typed_block_kind(kind) => {
                if let Some(block) = parse_legacy_block(source, node) {
                    out.push(block);
                } else if let Some(block) = parse_legacy_loose_block_from_node(
                    source,
                    node,
                    recovery_depth.saturating_add(1),
                ) {
                    out.push(block);
                }
            }
            "start_tag" => {
                if let Some((element_node, next_index, end)) =
                    parse_legacy_loose_element_from_start_tag(
                        source,
                        nodes,
                        index,
                        recovery_depth.saturating_add(1),
                    )
                {
                    out.push(element_node);
                    consumed_until = consumed_until.max(end);
                    index = next_index;
                    continue;
                }
            }
            "self_closing_tag" => {
                if let Some((element_node, next_index, end)) =
                    parse_legacy_loose_element_from_self_closing_tag(source, nodes, index)
                {
                    out.push(element_node);
                    consumed_until = consumed_until.max(end);
                    index = next_index;
                    continue;
                }
            }
            "ERROR" if recovery_depth < 16 => {
                let mut nested = parse_legacy_children(source, node, false, recovery_depth + 1);
                let modern_nodes = recover_modern_error_nodes(source, node, false);
                let modern_as_legacy = legacy_nodes_from_modern_error_recovery(modern_nodes);
                if !modern_as_legacy.is_empty() {
                    let nested_has_structure = legacy_nodes_have_structural_content(&nested);
                    let modern_has_structure =
                        legacy_nodes_have_structural_content(&modern_as_legacy);
                    if nested.is_empty() || (modern_has_structure && !nested_has_structure) {
                        nested = modern_as_legacy;
                    }
                }
                out.append(&mut nested);
            }
            "tag_name" => {
                let name = text_for_node(source, node);
                if !name.is_empty() {
                    let mut start = node.start_byte();
                    if start > 0
                        && source
                            .as_bytes()
                            .get(start.saturating_sub(1))
                            .is_some_and(|byte| *byte == b'<')
                    {
                        start = start.saturating_sub(1);
                    }
                    let element = legacy::Element {
                        start,
                        end: node.end_byte(),
                        name,
                        tag: None,
                        attributes: Box::new([]),
                        children: Box::new([]),
                    };
                    out.push(legacy_node_from_element_with_source(source, element));
                }
            }
            "end_tag"
            | "block_end"
            | "else_if_clause"
            | "else_clause"
            | "await_branch"
            | "erroneous_end_tag_name" => {}
            _ => {}
        }

        consumed_until = consumed_until.max(node.end_byte());
        index += 1;
    }

    out
}

fn parse_legacy_loose_block_from_node(
    source: &str,
    node: Node<'_>,
    _recovery_depth: usize,
) -> Option<legacy::Node> {
    // With typed nodes, if the normal parse failed, try basic recovery
    // by extracting the expression and body from the typed block node.
    let block_kind = BlockKind::from_node_kind(node.kind())?;
    let expression = node
        .child_by_field_name("expression")
        .and_then(|expr| parse_legacy_expression(source, expr, false))
        .unwrap_or_else(|| {
            legacy_empty_identifier_expression(node.end_byte(), node.end_byte(), None)
        });

    match block_kind {
        BlockKind::If => Some(legacy::Node::IfBlock(legacy::IfBlock {
            start: node.start_byte(),
            end: node.end_byte(),
            expression,
            children: Box::new([]),
            else_block: None,
            elseif: None,
        })),
        BlockKind::Key => Some(legacy::Node::KeyBlock(legacy::KeyBlock {
            start: node.start_byte(),
            end: node.end_byte(),
            expression,
            children: Box::new([]),
        })),
        _ => None,
    }
}

fn parse_legacy_loose_element_from_self_closing_tag(
    source: &str,
    nodes: &[Node<'_>],
    index: usize,
) -> Option<(legacy::Node, usize, usize)> {
    let tag_node = *nodes.get(index)?;
    let name = legacy_tag_name_from_tag_node(source, tag_node);
    let attributes = parse_legacy_attributes(source, tag_node);
    let element = legacy::Element {
        start: tag_node.start_byte(),
        end: tag_node.end_byte(),
        name,
        tag: None,
        attributes: attributes.into_boxed_slice(),
        children: Box::new([]),
    };
    let end = element.end;
    Some((
        legacy_node_from_element_with_source(source, element),
        index + 1,
        end,
    ))
}

fn parse_legacy_loose_element_from_start_tag(
    source: &str,
    nodes: &[Node<'_>],
    index: usize,
    recovery_depth: usize,
) -> Option<(legacy::Node, usize, usize)> {
    let start_tag = *nodes.get(index)?;
    let name = legacy_tag_name_from_tag_node(source, start_tag);
    if name.is_empty() {
        return None;
    }

    let attributes = parse_legacy_attributes(source, start_tag);
    if is_legacy_void_element(name.as_ref()) {
        let element = legacy::Element {
            start: start_tag.start_byte(),
            end: start_tag.end_byte(),
            name,
            tag: None,
            attributes: attributes.into_boxed_slice(),
            children: Box::new([]),
        };
        let end = element.end;
        return Some((
            legacy_node_from_element_with_source(source, element),
            index + 1,
            end,
        ));
    }

    let mut matching_close_idx: Option<usize> = None;
    let mut boundary_idx = nodes.len();
    for (candidate_idx, candidate) in nodes.iter().enumerate().skip(index + 1) {
        if candidate.kind() == "end_tag" {
            let close_name = legacy_tag_name_from_tag_node(source, *candidate);
            if close_name == name {
                matching_close_idx = Some(candidate_idx);
                boundary_idx = candidate_idx;
                break;
            }
            boundary_idx = candidate_idx;
            break;
        }
        if candidate.kind() == "ERROR" {
            let raw = text_for_node(source, *candidate);
            let trimmed = raw.trim_start();
            if let Some(rest) = trimmed.strip_prefix("</")
                && let Some(tail) = rest.strip_prefix(name.as_ref())
            {
                let next = tail.chars().next();
                if next.is_none_or(|ch| ch.is_whitespace() || ch == '>' || ch == '/') {
                    matching_close_idx = Some(candidate_idx);
                    boundary_idx = candidate_idx;
                    break;
                }
            }
            if trimmed.starts_with("</") {
                boundary_idx = candidate_idx;
                break;
            }
        }
        if candidate.kind() == "block_end" {
            boundary_idx = candidate_idx;
            break;
        }
    }

    let mut children = if boundary_idx > index + 1 {
        parse_legacy_nodes_slice_with_depth(
            source,
            &nodes[(index + 1)..boundary_idx],
            recovery_depth.saturating_add(1),
        )
    } else {
        Vec::new()
    };

    let (end, next_index) = if let Some(close_idx) = matching_close_idx {
        (nodes[close_idx].end_byte(), close_idx + 1)
    } else if matches!(classify_element_name(name.as_ref()), ElementKind::Textarea) {
        let content_start = start_tag.end_byte();
        let mut sequence_end = nodes
            .last()
            .map(|node| node.end_byte())
            .unwrap_or(start_tag.end_byte());
        let scan_end = source.len().min(sequence_end.saturating_add(32));
        if let Some(rel) = source
            .get(sequence_end..scan_end)
            .and_then(|tail| tail.find('>'))
        {
            sequence_end = sequence_end + rel + 1;
        }
        let close_start =
            find_valid_legacy_closing_tag_start(source, content_start, sequence_end, "textarea")
                .unwrap_or(sequence_end);
        children = parse_legacy_textarea_children(source, content_start, close_start);
        (sequence_end, nodes.len())
    } else if boundary_idx < nodes.len() {
        (nodes[boundary_idx].start_byte(), boundary_idx)
    } else {
        (
            children
                .last()
                .map(legacy_node_end)
                .unwrap_or(start_tag.end_byte()),
            boundary_idx,
        )
    };

    let element = legacy::Element {
        start: start_tag.start_byte(),
        end,
        name,
        tag: None,
        attributes: attributes.into_boxed_slice(),
        children: children.into_boxed_slice(),
    };
    Some((
        legacy_node_from_element_with_source(source, element),
        next_index.max(index + 1),
        end,
    ))
}

fn trim_legacy_block_children(mut nodes: Vec<legacy::Node>) -> Vec<legacy::Node> {
    while matches!(
        nodes.first(),
        Some(legacy::Node::Text(legacy::Text { data, .. })) if data.chars().all(char::is_whitespace)
    ) {
        nodes.remove(0);
    }

    while matches!(
        nodes.last(),
        Some(legacy::Node::Text(legacy::Text { data, .. })) if data.chars().all(char::is_whitespace)
    ) {
        nodes.pop();
    }

    nodes
}

fn legacy_node_from_element_with_source(
    source: &str,
    mut element: legacy::Element,
) -> legacy::Node {
    let (this_tag, attributes) = split_legacy_this_attribute(source, element.attributes.into_vec());
    element.attributes = attributes.into_boxed_slice();

    match classify_element_name(element.name.as_ref()) {
        ElementKind::Svelte(SvelteElementKind::Component) => {
            let expression = match this_tag {
                Some(legacy::ElementTag::Expression(expr)) => Some(expr),
                _ => None,
            };
            return legacy::Node::InlineComponent(legacy::InlineComponent {
                start: element.start,
                end: element.end,
                name: element.name,
                expression,
                attributes: element.attributes,
                children: element.children,
            });
        }
        ElementKind::Svelte(SvelteElementKind::SelfTag) => {
            return legacy::Node::InlineComponent(legacy::InlineComponent {
                start: element.start,
                end: element.end,
                name: element.name,
                expression: None,
                attributes: element.attributes,
                children: element.children,
            });
        }
        ElementKind::Svelte(SvelteElementKind::Head) => {
            return legacy::Node::Head(legacy::Head {
                start: element.start,
                end: element.end,
                name: element.name,
                attributes: element.attributes,
                children: element.children,
            });
        }
        ElementKind::Svelte(SvelteElementKind::Element) => {
            element.tag = this_tag;
            return legacy::Node::Element(element);
        }
        _ => {}
    }

    let is_inline = is_component_name(element.name.as_ref());
    if is_inline {
        legacy::Node::InlineComponent(legacy::InlineComponent {
            start: element.start,
            end: element.end,
            name: element.name,
            expression: None,
            attributes: element.attributes,
            children: element.children,
        })
    } else {
        legacy::Node::Element(element)
    }
}

fn split_legacy_this_attribute(
    source: &str,
    attributes: Vec<legacy::Attribute>,
) -> (Option<legacy::ElementTag>, Vec<legacy::Attribute>) {
    let mut out = Vec::new();
    let mut this_tag = None;

    for attribute in attributes {
        match attribute {
            legacy::Attribute::Attribute(attr)
                if classify_attribute_name(attr.name.as_ref()) == AttributeKind::This
                    && this_tag.is_none() =>
            {
                this_tag = legacy_element_tag_from_attribute_value(source, &attr.value);
            }
            other => out.push(other),
        }
    }

    (this_tag, out)
}

fn legacy_element_tag_from_attribute_value(
    source: &str,
    value: &legacy::AttributeValueList,
) -> Option<legacy::ElementTag> {
    let legacy::AttributeValueList::Values(values) = value else {
        return None;
    };
    let first = values.first()?;

    match first {
        legacy::AttributeValue::Text(text) => {
            let raw = text.raw.as_deref().unwrap_or_default();
            if let Some(inner) = raw.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                let trimmed = inner.trim();
                let leading = inner.find(trimmed).unwrap_or(0);
                let start = text.start + 1 + leading;
                let (line, column) = line_column_at_offset(source, start);
                if let Some(expression) =
                    parse_legacy_expression_from_text(trimmed, start, line, column, false)
                {
                    return Some(legacy::ElementTag::Expression(expression));
                }
            }

            Some(legacy::ElementTag::String(text.data.clone()))
        }
        legacy::AttributeValue::MustacheTag(tag) => {
            Some(legacy::ElementTag::Expression(tag.expression.clone()))
        }
        legacy::AttributeValue::AttributeShorthand(shorthand) => {
            Some(legacy::ElementTag::Expression(shorthand.expression.clone()))
        }
    }
}

// moved from api.rs during api cleanup
fn parse_mustache_tag(source: &str, node: Node<'_>) -> Option<legacy::MustacheTag> {
    let expression = parse_legacy_expression(source, node, false).or_else(|| {
        let raw = text_for_node(source, node);
        let raw_ref = raw.as_ref();
        if !(raw_ref.starts_with('{') && raw_ref.ends_with('}')) {
            return None;
        }
        let inner = &raw_ref[1..raw_ref.len().saturating_sub(1)];
        let trimmed = inner.trim();
        let leading = inner.find(trimmed).unwrap_or(0);
        let start = node.start_byte() + 1 + leading;
        let end = start + trimmed.len();
        Some(legacy_empty_identifier_expression(start, end, None))
    })?;

    Some(legacy::MustacheTag {
        start: node.start_byte(),
        end: node.end_byte(),
        expression,
    })
}

fn parse_legacy_tag(source: &str, tag: Node<'_>) -> Option<legacy::Node> {
    match tag.kind() {
        "html_tag" => Some(legacy::Node::RawMustacheTag(legacy::RawMustacheTag {
            start: tag.start_byte(),
            end: tag.end_byte(),
            expression: parse_legacy_special_tag_expression_or_empty(source, tag),
        })),
        "debug_tag" => {
            let arguments = parse_legacy_debug_tag_arguments(source, tag);
            let identifiers = legacy_debug_tag_identifiers(&arguments);
            Some(legacy::Node::DebugTag(legacy::DebugTag {
                start: tag.start_byte(),
                end: tag.end_byte(),
                arguments,
                identifiers,
            }))
        }
        _ => None,
    }
}

fn parse_legacy_special_tag_expression_or_empty(source: &str, tag: Node<'_>) -> legacy::Expression {
    let expression = find_first_named_child(tag, "expression_value")
        .or_else(|| find_first_named_child(tag, "expression"))
        .and_then(|node| super::modern::parse_modern_expression_field(source, node))
        .map(legacy_expression_from_modern_or_empty);

    expression.unwrap_or_else(|| {
        let end = tag.end_byte().saturating_sub(1);
        legacy_empty_identifier_expression(end, end, None)
    })
}

fn parse_legacy_debug_tag_arguments(source: &str, tag: Node<'_>) -> Box<[legacy::Expression]> {
    let expr_node = find_first_named_child(tag, "expression_value")
        .or_else(|| find_first_named_child(tag, "expression"));
    let Some(expr_node) = expr_node else {
        return Box::new([]);
    };

    super::modern::parse_modern_expression_field(source, expr_node)
        .map(super::modern::split_debug_tag_arguments)
        .map(|arguments| {
            arguments
                .into_vec()
                .into_iter()
                .map(legacy_expression_from_modern_or_empty)
                .collect::<Vec<_>>()
                .into_boxed_slice()
        })
        .unwrap_or_default()
}

fn legacy_debug_tag_identifiers(
    arguments: &[legacy::Expression],
) -> Box<[legacy::IdentifierExpression]> {
    arguments
        .iter()
        .filter_map(|argument| match argument {
            legacy::Expression::Identifier(identifier) => Some(identifier.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn parse_attribute_shorthand(source: &str, node: Node<'_>) -> Option<legacy::AttributeShorthand> {
    let raw = text_for_node(source, node);
    let expression = if raw.starts_with('{') && raw.ends_with('}') {
        let inner = &raw[1..raw.len().saturating_sub(1)];
        let trimmed = inner.trim();
        let leading = inner.find(trimmed).unwrap_or(0);
        let start = node.start_byte() + 1 + leading;
        let end = start + trimmed.len();
        let (line, column) = line_column_at_offset(source, start);
        parse_legacy_expression_from_text(inner, start, line, column, true).or_else(|| {
            let loc = legacy_loc(
                source,
                line,
                column,
                column + trimmed.len(),
                start,
                end,
                true,
            );
            Some(legacy_empty_identifier_expression(start, end, Some(loc)))
        })
    } else {
        parse_legacy_expression(source, node, true)
    }?;
    let (start, end) = legacy_expression_span(&expression)?;

    Some(legacy::AttributeShorthand {
        start,
        end,
        expression,
    })
}

fn legacy_expression_span(expression: &legacy::Expression) -> Option<(usize, usize)> {
    match expression {
        legacy::Expression::Identifier(expr) => Some((expr.start, expr.end)),
        legacy::Expression::Literal(expr) => Some((expr.start, expr.end)),
        legacy::Expression::CallExpression(expr) => Some((expr.start, expr.end)),
        legacy::Expression::BinaryExpression(expr) => Some((expr.start, expr.end)),
        legacy::Expression::ArrowFunctionExpression { fields }
        | legacy::Expression::AssignmentExpression { fields }
        | legacy::Expression::UnaryExpression { fields }
        | legacy::Expression::MemberExpression { fields }
        | legacy::Expression::LogicalExpression { fields }
        | legacy::Expression::ConditionalExpression { fields }
        | legacy::Expression::ArrayPattern { fields }
        | legacy::Expression::ObjectPattern { fields }
        | legacy::Expression::RestElement { fields }
        | legacy::Expression::ArrayExpression { fields }
        | legacy::Expression::ObjectExpression { fields }
        | legacy::Expression::Property { fields }
        | legacy::Expression::FunctionExpression { fields }
        | legacy::Expression::TemplateLiteral { fields }
        | legacy::Expression::TaggedTemplateExpression { fields }
        | legacy::Expression::SequenceExpression { fields }
        | legacy::Expression::UpdateExpression { fields }
        | legacy::Expression::ThisExpression { fields }
        | legacy::Expression::NewExpression { fields } => {
            let start = estree_value_to_usize(fields.get("start"))?;
            let end = estree_value_to_usize(fields.get("end"))?;
            Some((start, end))
        }
    }
}

fn parse_legacy_expression(
    source: &str,
    node: Node<'_>,
    include_character: bool,
) -> Option<legacy::Expression> {
    let expr_node =
        find_first_named_child(node, "js").or_else(|| find_first_named_child(node, "ts"));
    let js_node = expr_node.unwrap_or(node);
    let text = text_for_node(source, js_node);
    let text_ref = text.as_ref();

    if expr_node.is_none()
        && node.kind() == "expression"
        && text_ref.len() >= 2
        && text_ref.starts_with('{')
        && text_ref.ends_with('}')
    {
        let inner = &text_ref[1..text_ref.len().saturating_sub(1)];
        return parse_legacy_expression_from_text(
            inner,
            node.start_byte() + 1,
            node.start_position().row + 1,
            node.start_position().column + 1,
            include_character,
        );
    }

    parse_legacy_expression_from_text(
        text_ref,
        js_node.start_byte(),
        js_node.start_position().row + 1,
        js_node.start_position().column,
        include_character,
    )
}

fn parse_legacy_expression_from_text(
    text: &str,
    start_byte: usize,
    line: usize,
    column: usize,
    include_character: bool,
) -> Option<legacy::Expression> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading_ws = text.find(trimmed).unwrap_or(0);
    let start = start_byte + leading_ws;
    let start_col = column + leading_ws;

    if let Some(mut modern_expression) =
        crate::compiler::phases::parse::parse_modern_expression_with_oxc(
            trimmed, start, line, start_col,
        )
    {
        attach_leading_comments_to_expression(&mut modern_expression, trimmed, start);
        attach_trailing_comments_to_expression(&mut modern_expression, trimmed, start);
        if let modern::Expression(raw, _) = modern_expression
            && let Some(expr) = legacy_expression_from_raw_node(raw, include_character)
        {
            return Some(expr);
        }
    }

    None
}

fn legacy_empty_identifier_expression(
    start: usize,
    end: usize,
    loc: Option<legacy::ExpressionLoc>,
) -> legacy::Expression {
    legacy::Expression::Identifier(legacy::IdentifierExpression {
        name: Arc::from(""),
        start,
        end,
        loc,
        fields: BTreeMap::new(),
    })
}

fn parse_legacy_pattern_from_text(
    text: &str,
    start_byte: usize,
    line: usize,
    column: usize,
) -> Option<legacy::Expression> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }

    let leading_ws = text.find(trimmed).unwrap_or(0);
    let start = start_byte + leading_ws;
    let start_col = column + leading_ws + 1;

    let modern::Expression(mut raw, _) =
        crate::compiler::phases::parse::parse_modern_expression_with_oxc(
            trimmed, start, line, start_col,
        )?;

    rewrite_raw_expression_for_pattern(&mut raw);
    legacy_expression_from_raw_node(raw, false)
}

fn rewrite_raw_expression_for_pattern(node: &mut modern::EstreeNode) {
    if let Some(modern::EstreeValue::String(kind)) = estree_node_field_mut(node, RawField::Type) {
        match kind.as_ref() {
            "ArrayExpression" => *kind = Arc::from("ArrayPattern"),
            "ObjectExpression" => *kind = Arc::from("ObjectPattern"),
            "SpreadElement" => *kind = Arc::from("RestElement"),
            _ => {}
        }
    }

    for value in node.fields.values_mut() {
        rewrite_raw_expression_for_pattern_value(value);
    }
}

fn rewrite_raw_expression_for_pattern_value(value: &mut modern::EstreeValue) {
    match value {
        modern::EstreeValue::Object(node) => rewrite_raw_expression_for_pattern(node),
        modern::EstreeValue::Array(items) => {
            for item in items.iter_mut() {
                rewrite_raw_expression_for_pattern_value(item);
            }
        }
        modern::EstreeValue::String(_)
        | modern::EstreeValue::Int(_)
        | modern::EstreeValue::UInt(_)
        | modern::EstreeValue::Number(_)
        | modern::EstreeValue::Bool(_)
        | modern::EstreeValue::Null => {}
    }
}

pub(crate) fn legacy_expression_from_raw_node(
    mut node: modern::EstreeNode,
    include_character: bool,
) -> Option<legacy::Expression> {
    let kind = match estree_node_field(&node, RawField::Type) {
        Some(modern::EstreeValue::String(kind)) => kind.to_string(),
        _ => return None,
    };

    if include_character && (kind == "Identifier" || kind == "Literal") {
        maybe_set_raw_loc_character(&mut node);
    }

    normalize_pattern_template_elements(&mut node);

    match kind.as_str() {
        "ArrowFunctionExpression"
        | "AssignmentExpression"
        | "UnaryExpression"
        | "MemberExpression"
        | "LogicalExpression"
        | "ConditionalExpression"
        | "ArrayPattern"
        | "ObjectPattern"
        | "RestElement"
        | "ArrayExpression"
        | "ObjectExpression"
        | "Property"
        | "FunctionExpression"
        | "TemplateLiteral"
        | "TaggedTemplateExpression"
        | "SequenceExpression"
        | "UpdateExpression"
        | "ThisExpression"
        | "NewExpression" => {
            let mut fields = node.fields;
            fields.remove("type");
            return Some(match kind.as_str() {
                "ArrowFunctionExpression" => legacy::Expression::ArrowFunctionExpression { fields },
                "AssignmentExpression" => legacy::Expression::AssignmentExpression { fields },
                "UnaryExpression" => legacy::Expression::UnaryExpression { fields },
                "MemberExpression" => legacy::Expression::MemberExpression { fields },
                "LogicalExpression" => legacy::Expression::LogicalExpression { fields },
                "ConditionalExpression" => legacy::Expression::ConditionalExpression { fields },
                "ArrayPattern" => legacy::Expression::ArrayPattern { fields },
                "ObjectPattern" => legacy::Expression::ObjectPattern { fields },
                "RestElement" => legacy::Expression::RestElement { fields },
                "ArrayExpression" => legacy::Expression::ArrayExpression { fields },
                "ObjectExpression" => legacy::Expression::ObjectExpression { fields },
                "Property" => legacy::Expression::Property { fields },
                "FunctionExpression" => legacy::Expression::FunctionExpression { fields },
                "TemplateLiteral" => legacy::Expression::TemplateLiteral { fields },
                "TaggedTemplateExpression" => {
                    legacy::Expression::TaggedTemplateExpression { fields }
                }
                "SequenceExpression" => legacy::Expression::SequenceExpression { fields },
                "UpdateExpression" => legacy::Expression::UpdateExpression { fields },
                "ThisExpression" => legacy::Expression::ThisExpression { fields },
                "NewExpression" => legacy::Expression::NewExpression { fields },
                _ => unreachable!(),
            });
        }
        _ => {}
    }

    let value = serde_json::to_value(&node).ok()?;
    serde_json::from_value::<legacy::Expression>(value).ok()
}

fn maybe_set_raw_loc_character(node: &mut modern::EstreeNode) {
    let Some(start) = estree_value_to_usize(estree_node_field(node, RawField::Start)) else {
        return;
    };
    let Some(end) = estree_value_to_usize(estree_node_field(node, RawField::End)) else {
        return;
    };
    let Some(modern::EstreeValue::Object(loc)) = estree_node_field_mut(node, RawField::Loc) else {
        return;
    };
    let Some(modern::EstreeValue::Object(start_loc)) = estree_node_field_mut(loc, RawField::Start)
    else {
        return;
    };
    start_loc.fields.insert(
        "character".to_string(),
        modern::EstreeValue::UInt(start as u64),
    );

    let Some(modern::EstreeValue::Object(end_loc)) = estree_node_field_mut(loc, RawField::End)
    else {
        return;
    };
    end_loc.fields.insert(
        "character".to_string(),
        modern::EstreeValue::UInt(end as u64),
    );
}

pub(crate) fn parse_identifier_name(text: &str) -> Option<Arc<str>> {
    if text.is_empty() {
        return None;
    }

    let mut chars = text.chars();
    let first = chars.next()?;
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return None;
    }

    if chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()) {
        return Some(Arc::from(text));
    }

    None
}

fn legacy_loc(
    source: &str,
    line: usize,
    start_col: usize,
    end_col: usize,
    start: usize,
    end: usize,
    include_character: bool,
) -> legacy::ExpressionLoc {
    legacy::ExpressionLoc {
        start: legacy::ExpressionPoint {
            line,
            column: start_col,
            character: include_character.then_some(source_utf16_offset(source, start)),
        },
        end: legacy::ExpressionPoint {
            line,
            column: end_col,
            character: include_character.then_some(source_utf16_offset(source, end)),
        },
    }
}

pub(crate) fn find_first_named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

pub(crate) fn text_for_node(source: &str, node: Node<'_>) -> Arc<str> {
    Arc::from(node.utf8_text(source.as_bytes()).unwrap_or_default())
}

fn source_utf16_offset(source: &str, offset: usize) -> usize {
    SourceText::new(SourceId::new(0), source, None).utf16_offset(offset)
}

fn source_location_at_offset(source: &str, offset: usize) -> SourceLocation {
    SourceText::new(SourceId::new(0), source, None).location_at_offset(offset)
}

pub(crate) fn source_location_from_point(
    source: &str,
    _point: Point,
    offset: usize,
) -> SourceLocation {
    source_location_at_offset(source, offset)
}

fn decode_html_entities(raw: &str) -> Arc<str> {
    if !raw.contains('&') {
        return Arc::from(raw);
    }

    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'&' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }

        let mut j = i + 1;
        while j < bytes.len() {
            let b = bytes[j];
            if b.is_ascii_alphanumeric() || b == b'#' || b == b'x' || b == b'X' {
                j += 1;
            } else {
                break;
            }
        }

        if j < bytes.len() && bytes[j] == b';' && j > i + 1 {
            let entity = &raw[(i + 1)..j];
            if let Some(decoded) = decode_html_entity_piece(entity) {
                out.push_str(&decoded);
                i = j + 1;
                continue;
            }
            out.push_str(&raw[i..=j]);
            i = j + 1;
            continue;
        }

        if j > i + 1 {
            let entity = &raw[(i + 1)..j];
            let next = bytes.get(j).copied();
            if next.is_none_or(|b| !b.is_ascii_alphanumeric())
                && entity.bytes().all(|b| b.is_ascii_alphabetic())
                && let Some(decoded) = decode_html_entity_piece(entity)
            {
                out.push_str(&decoded);
                i = j;
                continue;
            }
        }

        out.push('&');
        i += 1;
    }

    Arc::from(out)
}

fn decode_html_entity_piece(entity: &str) -> Option<String> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
        && let Ok(value) = u32::from_str_radix(hex, 16)
        && let Some(ch) = char::from_u32(value)
    {
        return Some(ch.to_string());
    }

    if let Some(dec) = entity.strip_prefix('#')
        && let Ok(value) = dec.parse::<u32>()
        && let Some(ch) = char::from_u32(value)
    {
        return Some(ch.to_string());
    }

    let probe = format!("&{entity};");
    let decoded = decode_html_entities_cow(&probe);
    if decoded.as_ref() == probe {
        return None;
    }
    Some(decoded.into_owned())
}
fn collect_legacy_document_comments(
    source: &str,
    html_children: &[legacy::Node],
    instance: Option<&legacy::Script>,
    module: Option<&legacy::Script>,
) -> Option<Box<[legacy::ProgramComment]>> {
    let mut comments = Vec::new();
    let mut seen: HashSet<(usize, usize, u8)> = HashSet::new();

    if let Some(script) = instance {
        collect_comments_from_raw_node(source, &script.content, &mut comments, &mut seen);
        collect_comments_from_script_content_range(
            source,
            &script.content,
            &mut comments,
            &mut seen,
        );
    }
    if let Some(script) = module {
        collect_comments_from_raw_node(source, &script.content, &mut comments, &mut seen);
        collect_comments_from_script_content_range(
            source,
            &script.content,
            &mut comments,
            &mut seen,
        );
    }

    collect_comments_from_legacy_nodes(source, html_children, &mut comments, &mut seen);

    comments.sort_by_key(|comment| (comment.start, comment.end));
    (!comments.is_empty()).then(|| comments.into_boxed_slice())
}

fn collect_comments_from_script_content_range(
    source: &str,
    program: &modern::EstreeNode,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    let Some(start) = estree_value_to_usize(estree_node_field(program, RawField::Start)) else {
        return;
    };
    let Some(end) = estree_value_to_usize(estree_node_field(program, RawField::End)) else {
        return;
    };
    if start >= end || end > source.len() {
        return;
    }

    let snippet = source.get(start..end).unwrap_or_default();
    for comment in parse_all_comment_nodes(snippet, start) {
        maybe_push_legacy_program_comment(source, &comment.fields, comments, seen);
    }
}

fn collect_comments_from_legacy_nodes(
    source: &str,
    nodes: &[legacy::Node],
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    for node in nodes {
        collect_comments_from_legacy_node(source, node, comments, seen);
    }
}

fn collect_comments_from_legacy_node(
    source: &str,
    node: &legacy::Node,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    match node {
        legacy::Node::Element(element) => {
            for attribute in element.attributes.iter() {
                collect_comments_from_legacy_attribute(source, attribute, comments, seen);
            }
            collect_comments_from_legacy_nodes(source, &element.children, comments, seen);
        }
        legacy::Node::Head(head) => {
            for attribute in head.attributes.iter() {
                collect_comments_from_legacy_attribute(source, attribute, comments, seen);
            }
            collect_comments_from_legacy_nodes(source, &head.children, comments, seen);
        }
        legacy::Node::InlineComponent(component) => {
            if let Some(expression) = &component.expression {
                collect_comments_from_legacy_expression(source, expression, comments, seen);
            }
            for attribute in component.attributes.iter() {
                collect_comments_from_legacy_attribute(source, attribute, comments, seen);
            }
            collect_comments_from_legacy_nodes(source, &component.children, comments, seen);
        }
        legacy::Node::MustacheTag(tag) => {
            collect_comments_from_legacy_expression(source, &tag.expression, comments, seen);
        }
        legacy::Node::RawMustacheTag(tag) => {
            collect_comments_from_legacy_expression(source, &tag.expression, comments, seen);
        }
        legacy::Node::DebugTag(tag) => {
            for identifier in tag.identifiers.iter() {
                for value in identifier.fields.values() {
                    collect_comments_from_raw_value(source, value, comments, seen);
                }
            }
        }
        legacy::Node::IfBlock(block) => {
            collect_comments_from_legacy_expression(source, &block.expression, comments, seen);
            collect_comments_from_legacy_nodes(source, &block.children, comments, seen);
            if let Some(else_block) = &block.else_block {
                collect_comments_from_legacy_nodes(source, &else_block.children, comments, seen);
            }
        }
        legacy::Node::EachBlock(block) => {
            if let Some(context) = &block.context {
                collect_comments_from_legacy_expression(source, context, comments, seen);
            }
            collect_comments_from_legacy_expression(source, &block.expression, comments, seen);
            if let Some(key) = &block.key {
                collect_comments_from_legacy_expression(source, key, comments, seen);
            }
            collect_comments_from_legacy_nodes(source, &block.children, comments, seen);
            if let Some(else_block) = &block.else_block {
                collect_comments_from_legacy_nodes(source, &else_block.children, comments, seen);
            }
        }
        legacy::Node::KeyBlock(block) => {
            collect_comments_from_legacy_expression(source, &block.expression, comments, seen);
            collect_comments_from_legacy_nodes(source, &block.children, comments, seen);
        }
        legacy::Node::AwaitBlock(block) => {
            collect_comments_from_legacy_expression(source, &block.expression, comments, seen);
            if let Some(value) = &block.value {
                collect_comments_from_legacy_expression(source, value, comments, seen);
            }
            if let Some(error) = &block.error {
                collect_comments_from_legacy_expression(source, error, comments, seen);
            }
            collect_comments_from_legacy_nodes(source, &block.pending.children, comments, seen);
            collect_comments_from_legacy_nodes(source, &block.then.children, comments, seen);
            collect_comments_from_legacy_nodes(source, &block.catch.children, comments, seen);
        }
        legacy::Node::SnippetBlock(block) => {
            collect_comments_from_legacy_expression(source, &block.expression, comments, seen);
            for parameter in block.parameters.iter() {
                collect_comments_from_legacy_expression(source, parameter, comments, seen);
            }
            collect_comments_from_legacy_nodes(source, &block.children, comments, seen);
        }
        legacy::Node::Text(_) | legacy::Node::Comment(_) => {}
    }
}

fn collect_comments_from_legacy_attribute(
    source: &str,
    attribute: &legacy::Attribute,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    match attribute {
        legacy::Attribute::Spread(spread) => {
            collect_comments_from_legacy_expression(source, &spread.expression, comments, seen);
        }
        legacy::Attribute::Transition(directive) => {
            if let Some(expression) = &directive.expression {
                collect_comments_from_legacy_expression(source, expression, comments, seen);
            }
        }
        legacy::Attribute::Attribute(named) => {
            if let legacy::AttributeValueList::Values(values) = &named.value {
                for value in values.iter() {
                    match value {
                        legacy::AttributeValue::MustacheTag(tag) => {
                            collect_comments_from_legacy_expression(
                                source,
                                &tag.expression,
                                comments,
                                seen,
                            );
                        }
                        legacy::AttributeValue::AttributeShorthand(shorthand) => {
                            collect_comments_from_legacy_expression(
                                source,
                                &shorthand.expression,
                                comments,
                                seen,
                            );
                        }
                        legacy::AttributeValue::Text(_) => {}
                    }
                }
            }
        }
        legacy::Attribute::StyleDirective(directive) => {
            if let legacy::AttributeValueList::Values(values) = &directive.value {
                for value in values.iter() {
                    if let legacy::AttributeValue::MustacheTag(tag) = value {
                        collect_comments_from_legacy_expression(
                            source,
                            &tag.expression,
                            comments,
                            seen,
                        );
                    }
                }
            }
        }
        legacy::Attribute::Let(directive)
        | legacy::Attribute::Action(directive)
        | legacy::Attribute::Binding(directive)
        | legacy::Attribute::Class(directive)
        | legacy::Attribute::Animation(directive)
        | legacy::Attribute::EventHandler(directive) => {
            if let Some(expression) = &directive.expression {
                collect_comments_from_legacy_expression(source, expression, comments, seen);
            }
        }
    }
}

fn collect_comments_from_legacy_expression(
    source: &str,
    expression: &legacy::Expression,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    match expression {
        legacy::Expression::Identifier(identifier) => {
            for value in identifier.fields.values() {
                collect_comments_from_raw_value(source, value, comments, seen);
            }
        }
        legacy::Expression::Literal(literal) => {
            for value in literal.fields.values() {
                collect_comments_from_raw_value(source, value, comments, seen);
            }
        }
        legacy::Expression::ThisExpression { fields } => {
            for value in fields.values() {
                collect_comments_from_raw_value(source, value, comments, seen);
            }
        }
        legacy::Expression::CallExpression(call) => {
            collect_comments_from_legacy_expression(source, &call.callee, comments, seen);
            for argument in call.arguments.iter() {
                collect_comments_from_legacy_expression(source, argument, comments, seen);
            }
            for value in call.fields.values() {
                collect_comments_from_raw_value(source, value, comments, seen);
            }
        }
        legacy::Expression::BinaryExpression(binary) => {
            collect_comments_from_legacy_expression(source, &binary.left, comments, seen);
            collect_comments_from_legacy_expression(source, &binary.right, comments, seen);
            for value in binary.fields.values() {
                collect_comments_from_raw_value(source, value, comments, seen);
            }
        }
        legacy::Expression::ArrowFunctionExpression { fields }
        | legacy::Expression::AssignmentExpression { fields }
        | legacy::Expression::UnaryExpression { fields }
        | legacy::Expression::MemberExpression { fields }
        | legacy::Expression::LogicalExpression { fields }
        | legacy::Expression::ConditionalExpression { fields }
        | legacy::Expression::ArrayPattern { fields }
        | legacy::Expression::ObjectPattern { fields }
        | legacy::Expression::RestElement { fields }
        | legacy::Expression::ArrayExpression { fields }
        | legacy::Expression::ObjectExpression { fields }
        | legacy::Expression::Property { fields }
        | legacy::Expression::FunctionExpression { fields }
        | legacy::Expression::TemplateLiteral { fields }
        | legacy::Expression::TaggedTemplateExpression { fields }
        | legacy::Expression::SequenceExpression { fields }
        | legacy::Expression::UpdateExpression { fields }
        | legacy::Expression::NewExpression { fields } => {
            for value in fields.values() {
                collect_comments_from_raw_value(source, value, comments, seen);
            }
        }
    }
}

fn collect_comments_from_raw_node(
    source: &str,
    node: &modern::EstreeNode,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    maybe_push_legacy_program_comment(source, &node.fields, comments, seen);
    for value in node.fields.values() {
        collect_comments_from_raw_value(source, value, comments, seen);
    }
}

fn collect_comments_from_raw_value(
    source: &str,
    value: &modern::EstreeValue,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    match value {
        modern::EstreeValue::Object(node) => {
            collect_comments_from_raw_node(source, node, comments, seen)
        }
        modern::EstreeValue::Array(items) => {
            for item in items.iter() {
                collect_comments_from_raw_value(source, item, comments, seen);
            }
        }
        modern::EstreeValue::String(_)
        | modern::EstreeValue::Int(_)
        | modern::EstreeValue::UInt(_)
        | modern::EstreeValue::Number(_)
        | modern::EstreeValue::Bool(_)
        | modern::EstreeValue::Null => {}
    }
}

fn maybe_push_legacy_program_comment(
    source: &str,
    fields: &BTreeMap<String, modern::EstreeValue>,
    comments: &mut Vec<legacy::ProgramComment>,
    seen: &mut HashSet<(usize, usize, u8)>,
) {
    let kind = match fields.get("type") {
        Some(modern::EstreeValue::String(kind)) if kind.as_ref() == "Line" => RootCommentType::Line,
        Some(modern::EstreeValue::String(kind)) if kind.as_ref() == "Block" => {
            RootCommentType::Block
        }
        _ => return,
    };

    let Some(modern::EstreeValue::String(value)) = fields.get("value") else {
        return;
    };
    let Some(start) = estree_value_to_usize(fields.get("start")) else {
        return;
    };
    let Some(end) = estree_value_to_usize(fields.get("end")) else {
        return;
    };

    let kind_id = match kind {
        RootCommentType::Line => 0,
        RootCommentType::Block => 1,
    };
    if !seen.insert((start, end, kind_id)) {
        return;
    }

    let (start_line, start_col) = line_column_at_offset(source, start);
    let (end_line, end_col) = line_column_at_offset(source, end);
    comments.push(legacy::ProgramComment {
        r#type: kind,
        value: value.clone(),
        start,
        end,
        loc: legacy::ExpressionLoc {
            start: legacy::ExpressionPoint {
                line: start_line,
                column: start_col,
                character: None,
            },
            end: legacy::ExpressionPoint {
                line: end_line,
                column: end_col,
                character: None,
            },
        },
    });
}

fn is_legacy_whitespace_text(node: &legacy::Node) -> bool {
    matches!(node, legacy::Node::Text(text) if text.data.chars().all(char::is_whitespace))
}

fn legacy_nodes_have_structural_content(nodes: &[legacy::Node]) -> bool {
    nodes
        .iter()
        .any(|node| !matches!(node, legacy::Node::Text(_) | legacy::Node::Comment(_)))
}

pub(crate) fn is_legacy_void_element(name: &str) -> bool {
    is_void_element_name(name)
}

pub(crate) fn legacy_node_start(node: &legacy::Node) -> usize {
    node.start()
}

pub(crate) fn legacy_node_end(node: &legacy::Node) -> usize {
    node.end()
}

pub(crate) fn merge_legacy_text_raw(current: &mut legacy::Text, next: &legacy::Text) {
    let merged_raw = format!(
        "{}{}",
        current.raw.as_deref().unwrap_or(current.data.as_ref()),
        next.raw.as_deref().unwrap_or(next.data.as_ref())
    );
    let merged_data = format!("{}{}", current.data, next.data);

    current.end = next.end;
    current.raw = Some(Arc::from(merged_raw));
    current.data = Arc::from(merged_data);
}

pub(crate) fn push_legacy_text_node(nodes: &mut Vec<legacy::Node>, text: legacy::Text) {
    if let Some(legacy::Node::Text(last)) = nodes.last_mut()
        && last.end == text.start
    {
        merge_legacy_text_raw(last, &text);
        return;
    }

    nodes.push(legacy::Node::Text(text));
}

pub(crate) fn push_legacy_attribute_text(
    values: &mut Vec<legacy::AttributeValue>,
    text: legacy::Text,
) {
    if let Some(legacy::AttributeValue::Text(last)) = values.last_mut()
        && last.end == text.start
    {
        merge_legacy_text_raw(last, &text);
        return;
    }

    values.push(legacy::AttributeValue::Text(text));
}
