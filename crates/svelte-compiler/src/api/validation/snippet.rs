use super::*;
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueList, EstreeNode, EstreeValue, Expression,
    Fragment, IfBlock, Node, Search, SnippetBlock, SnippetHeaderErrorKind,
};
use std::collections::HashSet;
use std::sync::Arc;

pub(super) fn detect_snippet_invalid_export(source: &str, root: &Root) -> Option<CompileError> {
    let module = root.module.as_ref()?;
    let (exported_name, export_start, export_end) = first_exported_name(&module.content)?;
    let snippet = find_snippet_block_by_name(&root.fragment, exported_name.as_str())?;

    let mut instance_names = HashSet::<String>::new();
    if let Some(instance) = root.instance.as_ref() {
        collect_instance_binding_names(&instance.content, &mut instance_names);
    }
    if instance_names.is_empty() {
        return None;
    }

    if fragment_references_any_name(&snippet.body, &instance_names) {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::SnippetInvalidExport,
            export_start,
            export_end,
        ));
    }

    None
}

pub(super) fn detect_malformed_snippet_headers(source: &str, root: &Root) -> Option<CompileError> {
    let error = root.fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let Node::SnippetBlock(block) = node else {
            return None;
        };
        block.header_error.as_ref()
    })?;

    let kind = match error.kind {
        SnippetHeaderErrorKind::ExpectedRightBrace => {
            CompilerDiagnosticKind::ExpectedTokenRightBrace
        }
        SnippetHeaderErrorKind::ExpectedRightParen => {
            CompilerDiagnosticKind::ExpectedTokenRightParen
        }
    };

    Some(compile_error_with_range(
        source,
        kind,
        error.start,
        error.end,
    ))
}

pub(super) fn detect_snippet_parameter_assignment(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let mut scope = Vec::<HashSet<String>>::new();
    let (start, end) = find_snippet_parameter_assignment(&root.fragment, &mut scope)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::SnippetParameterAssignment,
        start,
        end,
    ))
}

pub(super) fn detect_snippet_invalid_rest_parameter(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let (rest_start, rest_end) = find_invalid_rest_parameter(&root.fragment)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::SnippetInvalidRestParameter,
        rest_start,
        rest_end,
    ))
}

pub(super) fn detect_snippet_children_conflict(source: &str, root: &Root) -> Option<CompileError> {
    let (block_start, block_end) = find_children_snippet_conflict(&root.fragment)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::SnippetConflict,
        block_start,
        block_end,
    ))
}

pub(super) fn detect_slot_snippet_conflict(source: &str, root: &Root) -> Option<CompileError> {
    let (slot_start, slot_end) = find_slot_snippet_conflict(&root.fragment)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::SlotSnippetConflict,
        slot_start,
        slot_end,
    ))
}

fn find_invalid_rest_parameter(fragment: &Fragment) -> Option<(usize, usize)> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let Node::SnippetBlock(block) = node else {
            return None;
        };
        block.parameters.iter().find_map(rest_parameter_span)
    })
}

fn rest_parameter_span(parameter: &Expression) -> Option<(usize, usize)> {
    (estree_node_type(&parameter.0) == Some("RestElement"))
        .then(|| estree_node_span(&parameter.0))
        .flatten()
}

fn find_children_snippet_conflict(fragment: &Fragment) -> Option<(usize, usize)> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        match node {
            Node::Component(component) => {
                children_snippet_conflict(component.fragment.nodes.as_ref())
            }
            Node::SvelteComponent(component) => {
                children_snippet_conflict(component.fragment.nodes.as_ref())
            }
            Node::SvelteSelf(component) => {
                children_snippet_conflict(component.fragment.nodes.as_ref())
            }
            _ => None,
        }
    })
}

fn children_snippet_conflict(nodes: &[Node]) -> Option<(usize, usize)> {
    let snippet = nodes.iter().find_map(children_snippet)?;
    has_implicit_children(nodes).then_some((snippet.start, snippet.end))
}

fn children_snippet(node: &Node) -> Option<&SnippetBlock> {
    let Node::SnippetBlock(block) = node else {
        return None;
    };
    let name = expression_identifier_name(&block.expression)?;
    (name.as_ref() == "children").then_some(block)
}

fn has_implicit_children(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::SnippetBlock(_) | Node::Comment(_) => false,
        Node::Text(text) => !text.data.trim().is_empty(),
        _ => true,
    })
}

fn find_slot_snippet_conflict(fragment: &Fragment) -> Option<(usize, usize)> {
    let has_render = fragment.find_map(|entry| match entry.as_node()? {
        Node::RenderTag(tag) => Some((tag.start, tag.end)),
        _ => None,
    });
    let (slot_start, slot_end) = fragment.find_map(|entry| match entry.as_node()? {
        Node::SlotElement(slot) => Some((slot.start, slot.end)),
        _ => None,
    })?;

    has_render.map(|_| (slot_start, slot_end))
}

fn first_exported_name(program: &EstreeNode) -> Option<(String, usize, usize)> {
    let body = estree_node_field_array(program, RawField::Body)?;
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ExportNamedDeclaration")
            || estree_node_field_object(statement, RawField::Source).is_some()
        {
            continue;
        }
        let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers) else {
            continue;
        };
        for specifier in specifiers {
            let EstreeValue::Object(specifier) = specifier else {
                continue;
            };
            let Some(local) = estree_node_field_object(specifier, RawField::Local) else {
                continue;
            };
            let Some(name) = raw_identifier_name(local) else {
                continue;
            };
            let (start, end) = estree_node_span(local).or_else(|| estree_node_span(specifier))?;
            return Some((name, start, end));
        }
    }
    None
}

fn find_snippet_block_by_name<'a>(fragment: &'a Fragment, name: &str) -> Option<&'a SnippetBlock> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let Node::SnippetBlock(block) = node else {
            return None;
        };
        let snippet_name = expression_identifier_name(&block.expression)?;
        (snippet_name.as_ref() == name).then_some(block)
    })
}

fn find_snippet_parameter_assignment(
    fragment: &Fragment,
    scope: &mut Vec<HashSet<String>>,
) -> Option<(usize, usize)> {
    fragment.walk(
        scope,
        |entry, scope| {
            if let Some(block) = entry.as_if_block() {
                return match assignment_to_scoped_name_in_expression(&block.test, scope) {
                    Some(span) => Search::Found(span),
                    None => Search::Continue,
                };
            }

            let Some(node) = entry.as_node() else {
                return Search::Continue;
            };

            let found = match node {
                Node::Text(_) | Node::Comment(_) => None,
                Node::DebugTag(tag) => tag.identifiers.iter().find_map(|identifier| {
                    scope_contains_name(scope, identifier.name.as_ref())
                        .then_some((identifier.start, identifier.end))
                }),
                Node::ExpressionTag(tag) => {
                    assignment_to_scoped_name_in_expression(&tag.expression, scope)
                }
                Node::RenderTag(tag) => {
                    assignment_to_scoped_name_in_expression(&tag.expression, scope)
                }
                Node::HtmlTag(tag) => {
                    assignment_to_scoped_name_in_expression(&tag.expression, scope)
                }
                Node::ConstTag(tag) => {
                    assignment_to_scoped_name_in_expression(&tag.declaration, scope)
                }
                Node::IfBlock(_) => None,
                Node::EachBlock(block) => {
                    assignment_to_scoped_name_in_expression(&block.expression, scope).or_else(
                        || {
                            block
                                .key
                                .as_ref()
                                .and_then(|key| assignment_to_scoped_name_in_expression(key, scope))
                        },
                    )
                }
                Node::AwaitBlock(block) => {
                    assignment_to_scoped_name_in_expression(&block.expression, scope)
                        .or_else(|| {
                            block.value.as_ref().and_then(|value| {
                                assignment_to_scoped_name_in_expression(value, scope)
                            })
                        })
                        .or_else(|| {
                            block.error.as_ref().and_then(|error| {
                                assignment_to_scoped_name_in_expression(error, scope)
                            })
                        })
                }
                Node::SnippetBlock(block) => {
                    let mut names = HashSet::new();
                    for parameter in block.parameters.iter() {
                        collect_binding_names(&parameter.0, &mut names);
                    }
                    scope.push(names);
                    None
                }
                Node::KeyBlock(block) => {
                    assignment_to_scoped_name_in_expression(&block.expression, scope)
                }
                _ => {
                    let Some(element) = node.as_element() else {
                        return Search::Continue;
                    };
                    assignment_to_scoped_name_in_attributes(element.attributes(), scope)
                }
            };

            match found {
                Some(span) => Search::Found(span),
                None => Search::Continue,
            }
        },
        |entry, scope| {
            if let Some(Node::SnippetBlock(_)) = entry.as_node() {
                scope.pop();
            }
        },
    )
}

fn assignment_to_scoped_name_in_attributes(
    attributes: &[Attribute],
    scope: &mut [HashSet<String>],
) -> Option<(usize, usize)> {
    for attribute in attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) =
                                assignment_to_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueList::ExpressionTag(tag) => {
                    if let Some(span) =
                        assignment_to_scoped_name_in_expression(&tag.expression, scope)
                    {
                        return Some(span);
                    }
                }
            },
            Attribute::BindDirective(attribute) => {
                if let Some(base) = raw_base_identifier_name(&attribute.expression.0)
                    && scope_contains_name(scope, base.as_ref())
                {
                    return Some((attribute.start, attribute.end));
                }
                if let Some(span) =
                    assignment_to_scoped_name_in_expression(&attribute.expression, scope)
                {
                    return Some(span);
                }
            }
            Attribute::OnDirective(attribute)
            | Attribute::ClassDirective(attribute)
            | Attribute::LetDirective(attribute)
            | Attribute::AnimateDirective(attribute)
            | Attribute::UseDirective(attribute) => {
                if let Some(span) =
                    assignment_to_scoped_name_in_expression(&attribute.expression, scope)
                {
                    return Some(span);
                }
            }
            Attribute::StyleDirective(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) =
                                assignment_to_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueList::ExpressionTag(tag) => {
                    if let Some(span) =
                        assignment_to_scoped_name_in_expression(&tag.expression, scope)
                    {
                        return Some(span);
                    }
                }
            },
            Attribute::TransitionDirective(attribute) => {
                if let Some(span) =
                    assignment_to_scoped_name_in_expression(&attribute.expression, scope)
                {
                    return Some(span);
                }
            }
            Attribute::AttachTag(tag) => {
                if let Some(span) = assignment_to_scoped_name_in_expression(&tag.expression, scope)
                {
                    return Some(span);
                }
            }
            Attribute::SpreadAttribute(spread) => {
                if let Some(span) =
                    assignment_to_scoped_name_in_expression(&spread.expression, scope)
                {
                    return Some(span);
                }
            }
        }
    }
    None
}

fn assignment_to_scoped_name_in_expression(
    expression: &Expression,
    scope: &[HashSet<String>],
) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node(&expression.0, &mut |node| {
        if found.is_some() {
            return;
        }
        match estree_node_type(node) {
            Some("AssignmentExpression") => {
                let Some(left) = estree_node_field_object(node, RawField::Left) else {
                    return;
                };
                let Some(name) = raw_identifier_name(left) else {
                    return;
                };
                if !scope_contains_name(scope, name.as_str()) {
                    return;
                }
                if let Some(span) = estree_node_span(left).or_else(|| estree_node_span(node)) {
                    found = Some(span);
                }
            }
            Some("UpdateExpression") => {
                let Some(argument) = estree_node_field_object(node, RawField::Argument) else {
                    return;
                };
                let Some(name) = raw_identifier_name(argument) else {
                    return;
                };
                if !scope_contains_name(scope, name.as_str()) {
                    return;
                }
                if let Some(span) = estree_node_span(argument).or_else(|| estree_node_span(node)) {
                    found = Some(span);
                }
            }
            _ => {}
        }
    });
    found
}

fn scope_contains_name(scope: &[HashSet<String>], name: &str) -> bool {
    scope.iter().rev().any(|frame| frame.contains(name))
}

fn collect_instance_binding_names(program: &EstreeNode, out: &mut HashSet<String>) {
    let Some(body) = estree_node_field_array(program, RawField::Body) else {
        return;
    };
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("VariableDeclaration") {
            continue;
        }
        let Some(declarations) = estree_node_field_array(statement, RawField::Declarations) else {
            continue;
        };
        for declarator in declarations {
            let EstreeValue::Object(declarator) = declarator else {
                continue;
            };
            let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
                continue;
            };
            collect_binding_names(id, out);
        }
    }
}

fn collect_binding_names(pattern: &EstreeNode, out: &mut HashSet<String>) {
    match estree_node_type(pattern) {
        Some("Identifier") => {
            if let Some(name) = estree_node_field_str(pattern, RawField::Name) {
                out.insert(name.to_string());
            }
        }
        Some("RestElement") => {
            if let Some(argument) = estree_node_field_object(pattern, RawField::Argument) {
                collect_binding_names(argument, out);
            }
        }
        Some("AssignmentPattern") => {
            if let Some(left) = estree_node_field_object(pattern, RawField::Left) {
                collect_binding_names(left, out);
            }
        }
        Some("ArrayPattern") => {
            if let Some(elements) = estree_node_field_array(pattern, RawField::Elements) {
                for element in elements {
                    let EstreeValue::Object(element) = element else {
                        continue;
                    };
                    collect_binding_names(element, out);
                }
            }
        }
        Some("ObjectPattern") => {
            if let Some(properties) = estree_node_field_array(pattern, RawField::Properties) {
                for property in properties {
                    let EstreeValue::Object(property) = property else {
                        continue;
                    };
                    match estree_node_type(property) {
                        Some("Property") => {
                            if let Some(value) = estree_node_field_object(property, RawField::Value)
                            {
                                collect_binding_names(value, out);
                            }
                        }
                        Some("RestElement") => {
                            if let Some(argument) =
                                estree_node_field_object(property, RawField::Argument)
                            {
                                collect_binding_names(argument, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

fn fragment_references_any_name(fragment: &Fragment, names: &HashSet<String>) -> bool {
    fragment
        .nodes
        .iter()
        .any(|node| node_references_any_name(node, names))
}

fn node_references_any_name(node: &Node, names: &HashSet<String>) -> bool {
    match node {
        Node::Text(_) | Node::Comment(_) => false,
        Node::DebugTag(tag) => tag
            .identifiers
            .iter()
            .any(|identifier| names.contains(identifier.name.as_ref())),
        Node::ExpressionTag(tag) => expression_references_any_name(&tag.expression, names),
        Node::RenderTag(tag) => expression_references_any_name(&tag.expression, names),
        Node::HtmlTag(tag) => expression_references_any_name(&tag.expression, names),
        Node::ConstTag(tag) => expression_references_any_name(&tag.declaration, names),
        Node::SnippetBlock(block) => fragment_references_any_name(&block.body, names),
        Node::IfBlock(block) => if_block_references_any_name(block, names),
        Node::EachBlock(block) => {
            expression_references_any_name(&block.expression, names)
                || block
                    .context
                    .as_ref()
                    .is_some_and(|context| expression_references_any_name(context, names))
                || block
                    .key
                    .as_ref()
                    .is_some_and(|key| expression_references_any_name(key, names))
                || fragment_references_any_name(&block.body, names)
                || block
                    .fallback
                    .as_ref()
                    .is_some_and(|fallback| fragment_references_any_name(fallback, names))
        }
        Node::AwaitBlock(block) => {
            expression_references_any_name(&block.expression, names)
                || block
                    .value
                    .as_ref()
                    .is_some_and(|value| expression_references_any_name(value, names))
                || block
                    .error
                    .as_ref()
                    .is_some_and(|error| expression_references_any_name(error, names))
                || [
                    block.pending.as_ref(),
                    block.then.as_ref(),
                    block.catch.as_ref(),
                ]
                .iter()
                .flatten()
                .any(|fragment| fragment_references_any_name(fragment, names))
        }
        Node::KeyBlock(block) => {
            expression_references_any_name(&block.expression, names)
                || fragment_references_any_name(&block.fragment, names)
        }
        _ => {
            let Some(el) = node.as_element() else {
                return false;
            };
            fragment_references_any_name(el.fragment(), names)
        }
    }
}

fn if_block_references_any_name(block: &IfBlock, names: &HashSet<String>) -> bool {
    expression_references_any_name(&block.test, names)
        || fragment_references_any_name(&block.consequent, names)
        || match block.alternate.as_deref() {
            Some(Alternate::Fragment(fragment)) => fragment_references_any_name(fragment, names),
            Some(Alternate::IfBlock(block)) => if_block_references_any_name(block, names),
            None => false,
        }
}

fn expression_references_any_name(expression: &Expression, names: &HashSet<String>) -> bool {
    raw_expression_references_any_name(&expression.0, names)
}

fn raw_expression_references_any_name(raw: &EstreeNode, names: &HashSet<String>) -> bool {
    let mut found = false;
    walk_estree_node(raw, &mut |node| {
        if found || estree_node_type(node) != Some("Identifier") {
            return;
        }
        if let Some(name) = estree_node_field_str(node, RawField::Name)
            && names.contains(name)
        {
            found = true;
        }
    });
    found
}

fn estree_node_span(node: &EstreeNode) -> Option<(usize, usize)> {
    Some((
        estree_value_to_usize(estree_node_field(node, RawField::Start))?,
        estree_value_to_usize(estree_node_field(node, RawField::End))?,
    ))
}

fn raw_identifier_name(node: &EstreeNode) -> Option<String> {
    if estree_node_type(node) != Some("Identifier") {
        return None;
    }
    estree_node_field_str(node, RawField::Name).map(ToString::to_string)
}

fn raw_base_identifier_name(node: &EstreeNode) -> Option<Arc<str>> {
    match estree_node_type(node) {
        Some("Identifier") => estree_node_field_str(node, RawField::Name).map(Arc::from),
        Some("MemberExpression") => {
            let object = estree_node_field_object(node, RawField::Object)?;
            raw_base_identifier_name(object)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::validate_component_snippets;
    use crate::ast::modern::{Node, SnippetHeaderErrorKind};
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn validate(source: &str) -> Option<crate::error::CompileError> {
        let parsed = parse_component_for_compile(source).expect("parse component");
        validate_component_snippets(source, parsed.root())
    }

    fn snippet_header_error_kind(source: &str) -> Option<SnippetHeaderErrorKind> {
        let parsed = parse_component_for_compile(source).expect("parse component");
        parsed.root().fragment.find_map(|entry| {
            let node = entry.as_node()?;
            let Node::SnippetBlock(block) = node else {
                return None;
            };
            block.header_error.as_ref().map(|error| error.kind)
        })
    }

    #[test]
    fn rejects_snippet_rest_parameter_from_ast() {
        let error =
            validate("{#snippet row(...items)}{/snippet}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "snippet_invalid_rest_parameter");
    }

    #[test]
    fn recovers_missing_right_brace_in_snippet_header_from_cst() {
        let kind = snippet_header_error_kind("{#snippet children()hi{/snippet}");
        assert_eq!(kind, Some(SnippetHeaderErrorKind::ExpectedRightBrace));
    }

    #[test]
    fn recovers_missing_right_paren_in_snippet_header_from_cst() {
        let kind = snippet_header_error_kind("{#snippet children(hi{/snippet}");
        assert_eq!(kind, Some(SnippetHeaderErrorKind::ExpectedRightParen));
    }

    #[test]
    fn rejects_missing_right_brace_in_snippet_header_from_ast() {
        let error =
            validate("{#snippet children()hi{/snippet}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_token");
        assert_eq!(error.message.as_ref(), "Expected token }");
    }

    #[test]
    fn rejects_missing_right_paren_in_snippet_header_from_ast() {
        let error = validate("{#snippet children(hi{/snippet}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_token");
        assert_eq!(error.message.as_ref(), "Expected token )");
    }

    #[test]
    fn rejects_children_snippet_with_implicit_children_from_ast() {
        let error = validate("<Widget>before{#snippet children()}{/snippet}</Widget>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "snippet_conflict");
    }

    #[test]
    fn allows_children_snippet_without_implicit_children_from_ast() {
        let error = validate("<Widget>{#snippet children()}{/snippet}</Widget>");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn ignores_nested_children_snippet_inside_component_content() {
        let error = validate("<Widget><div>{#snippet children()}{/snippet}</div></Widget>");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_slot_and_render_conflict_from_ast() {
        let error = validate("{#snippet foo()}{/snippet}<slot />{@render foo()}")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "slot_snippet_conflict");
    }
}
