use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, Statement};
use oxc_parser::Parser;
use oxc_span::SourceType;
use svelte_syntax::ast::modern::{Attribute, Node, Search};
use svelte_syntax::{SourceId, SourceText, parse_modern_root, parse_svelte};

pub fn has_children(content: &str, is_svelte_5_plus: bool) -> bool {
    let Ok(root) = parse_modern_root(content) else {
        return false;
    };

    root.fragment
        .search(|entry, _| {
            let Some(node) = entry.as_node() else {
                return Search::Continue;
            };

            match node {
                Node::SlotElement(_) => Search::Found(true),
                Node::RenderTag(_) if is_svelte_5_plus => Search::Found(true),
                _ if is_svelte_5_plus
                    && node
                        .as_element()
                        .is_some_and(|element| has_children_attribute(element.attributes())) =>
                {
                    Search::Found(true)
                }
                _ => Search::Continue,
            }
        })
        .unwrap_or(false)
}

pub fn should_ignore(content: &str, match_index: usize) -> bool {
    should_ignore_in_javascript(content, match_index)
        || should_ignore_in_svelte_comments(content, match_index)
        || top_level_literal_contains_match(content, match_index)
}

fn has_children_attribute(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| match attribute {
        Attribute::Attribute(attribute) if attribute.name.as_ref() == "children" => {
            match &attribute.value {
                svelte_syntax::ast::modern::AttributeValueKind::ExpressionTag(expression) => {
                    expression.expression.identifier_name().as_deref()
                        == Some("children")
                }
                svelte_syntax::ast::modern::AttributeValueKind::Values(values) => {
                    values.iter().any(|value| match value {
                        svelte_syntax::ast::modern::AttributeValue::ExpressionTag(expression) => {
                            expression.expression.identifier_name().as_deref()
                                == Some("children")
                        }
                        _ => false,
                    })
                }
                svelte_syntax::ast::modern::AttributeValueKind::Boolean(_) => false,
            }
        }
        _ => false,
    })
}

fn should_ignore_in_javascript(content: &str, match_index: usize) -> bool {
    let allocator = Allocator::default();
    for source_type in [SourceType::mjs(), SourceType::default()] {
        let parsed = Parser::new(&allocator, content, source_type).parse();
        if !parsed.errors.is_empty() {
            continue;
        }

        let match_index = match_index as u32;
        if parsed
            .program
            .comments
            .iter()
            .any(|comment| comment.span.start <= match_index && match_index < comment.span.end)
        {
            return true;
        }

        if parsed
            .program
            .body
            .iter()
            .any(|statement| statement_contains_ignored_expression(content, statement, match_index))
        {
            return true;
        }
    }

    false
}

fn statement_contains_ignored_expression(
    source: &str,
    statement: &Statement<'_>,
    match_index: u32,
) -> bool {
    match statement {
        Statement::ExpressionStatement(statement) => {
            expression_contains_ignored_span(&statement.expression, match_index)
                || expression_statement_is_literal_source(
                    source,
                    statement.span.start,
                    statement.span.end,
                    match_index,
                )
        }
        _ => false,
    }
}

fn expression_statement_is_literal_source(
    source: &str,
    start: u32,
    end: u32,
    match_index: u32,
) -> bool {
    if !(start <= match_index && match_index < end) {
        return false;
    }

    source[start as usize..end as usize]
        .trim_start()
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '"' | '\'' | '`'))
}

fn expression_contains_ignored_span(expression: &Expression<'_>, match_index: u32) -> bool {
    match expression {
        Expression::StringLiteral(literal) => {
            literal.span.start <= match_index && match_index < literal.span.end
        }
        Expression::TemplateLiteral(literal) => {
            literal.span.start <= match_index && match_index < literal.span.end
        }
        Expression::ParenthesizedExpression(expression) => {
            expression_contains_ignored_span(&expression.expression, match_index)
        }
        Expression::TSAsExpression(expression) => {
            expression_contains_ignored_span(&expression.expression, match_index)
        }
        Expression::TSSatisfiesExpression(expression) => {
            expression_contains_ignored_span(&expression.expression, match_index)
        }
        Expression::TSNonNullExpression(expression) => {
            expression_contains_ignored_span(&expression.expression, match_index)
        }
        Expression::TSInstantiationExpression(expression) => {
            expression_contains_ignored_span(&expression.expression, match_index)
        }
        _ => false,
    }
}

fn should_ignore_in_svelte_comments(content: &str, match_index: usize) -> bool {
    let source = SourceText::new(SourceId::new(0), content, None);
    let Ok(document) = parse_svelte(source) else {
        return false;
    };

    let match_index = match_index;
    let mut stack = vec![document.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment"
            && node.start_byte() <= match_index
            && match_index < node.end_byte()
        {
            return true;
        }

        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }

    false
}

fn top_level_literal_contains_match(content: &str, match_index: usize) -> bool {
    let trimmed = content.trim();
    if trimmed.contains(';') {
        return false;
    }

    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    let Some(last) = trimmed.chars().last() else {
        return false;
    };

    if !matches!(first, '"' | '\'' | '`') || first != last {
        return false;
    }

    let start = content.len().saturating_sub(content.trim_start().len());
    let end = content.trim_end().len();
    start < match_index && match_index < end.saturating_sub(1)
}
