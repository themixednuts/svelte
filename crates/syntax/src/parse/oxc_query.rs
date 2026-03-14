use std::sync::Arc;

use oxc_ast::ast::{BindingPattern, Expression as OxcExpression, ObjectPropertyKind};
use oxc_span::GetSpan;

use crate::ast::modern::{Expression, JsNodeHandle};

pub(crate) fn expression_identifier_name(expression: &Expression) -> Option<Arc<str>> {
    expression.identifier_name()
}

pub(crate) fn expression_literal_string(expression: &Expression) -> Option<Arc<str>> {
    match expression.oxc_expression()? {
        OxcExpression::StringLiteral(value) => Some(Arc::from(value.value.as_str())),
        _ => None,
    }
}

pub(crate) fn expression_literal_bool(expression: &Expression) -> Option<bool> {
    match expression.oxc_expression()? {
        OxcExpression::BooleanLiteral(value) => Some(value.value),
        _ => None,
    }
}

pub(crate) fn split_debug_tag_arguments(expression: Expression) -> Box<[Expression]> {
    if let Some(OxcExpression::SequenceExpression(sequence)) = expression.oxc_expression() {
        let root = match &expression.node {
            Some(JsNodeHandle::Expression(root)) => Some(root.clone()),
            Some(JsNodeHandle::SequenceItem { root, .. }) => Some(root.clone()),
            _ => None,
        };

        if let Some(root) = root {
            return sequence
                .expressions
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    let span = node.span();
                    Expression::from_sequence_item(
                        root.clone(),
                        index,
                        span.start as usize,
                        span.end as usize,
                    )
                })
                .collect::<Vec<_>>()
                .into_boxed_slice();
        }
    }

    vec![expression].into_boxed_slice()
}

pub(crate) fn invalid_binding_expression_error(
    expression: &Expression,
) -> Option<(usize, Arc<str>)> {
    if let Some(oxc) = expression.oxc_expression()
        && let Some(error) = invalid_binding_expression_error_oxc(oxc)
    {
        return Some(error);
    }

    invalid_binding_pattern_error(expression.oxc_pattern()?)
}

fn invalid_binding_expression_error_oxc(
    expression: &OxcExpression<'_>,
) -> Option<(usize, Arc<str>)> {
    match expression {
        OxcExpression::ObjectExpression(expression) => {
            for (index, property) in expression.properties.iter().enumerate() {
                if property.is_spread() && index + 1 < expression.properties.len() {
                    let start = property.span().end as usize;
                    return Some((
                        start,
                        Arc::from("Comma is not permitted after the rest element"),
                    ));
                }
                if let ObjectPropertyKind::ObjectProperty(property) = property
                    && let Some(error) = invalid_binding_expression_error_oxc(&property.value)
                {
                    return Some(error);
                }
            }
            None
        }
        OxcExpression::ArrayExpression(expression) => {
            for (index, element) in expression.elements.iter().enumerate() {
                if element.is_spread() && index + 1 < expression.elements.len() {
                    let start = element.span().end as usize;
                    return Some((
                        start,
                        Arc::from("Comma is not permitted after the rest element"),
                    ));
                }
            }
            None
        }
        _ => None,
    }
}

fn invalid_binding_pattern_error(pattern: &BindingPattern<'_>) -> Option<(usize, Arc<str>)> {
    match pattern {
        BindingPattern::ObjectPattern(pattern) => {
            for (index, property) in pattern.properties.iter().enumerate() {
                if let Some(error) = invalid_binding_pattern_error(&property.value) {
                    return Some(error);
                }
                if index + 1 < pattern.properties.len() && pattern.rest.is_some() {
                    let start = property.span.end as usize;
                    return Some((
                        start,
                        Arc::from("Comma is not permitted after the rest element"),
                    ));
                }
            }
            None
        }
        BindingPattern::ArrayPattern(pattern) => {
            if pattern.rest.is_some() && pattern.elements.iter().flatten().count() > 0 {
                let start = pattern
                    .elements
                    .iter()
                    .flatten()
                    .last()
                    .map(|element| element.span().end as usize)
                    .unwrap_or(pattern.span.start as usize);
                return Some((
                    start,
                    Arc::from("Comma is not permitted after the rest element"),
                ));
            }
            None
        }
        BindingPattern::AssignmentPattern(pattern) => invalid_binding_pattern_error(&pattern.left),
        BindingPattern::BindingIdentifier(_) => None,
    }
}
