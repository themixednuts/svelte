use super::*;
use crate::api::validation::scope::extend_name_set_with_oxc_pattern_bindings;
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueKind, Expression, Fragment, IfBlock, Node,
    Search, SnippetBlock, SnippetHeaderErrorKind,
};
use crate::source::{NamedSpan, SourceSpan};
use oxc_ast::ast::Statement;
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;
use std::sync::Arc;
use svelte_syntax::JsProgram;

impl ComponentValidator<'_> {
    pub(super) fn snippet_invalid_export(&self) -> Option<CompileError> {
        let module = self.root.module.as_ref()?;
        let offset = module.content_start;
        let named = first_exported_name(&module.content)?;
        let snippet = find_snippet_block_by_name(&self.root.fragment, named.name.as_ref())?;

        let mut instance_names = NameSet::default();
        if let Some(instance) = self.root.instance.as_ref() {
            collect_instance_binding_names(&instance.content, &mut instance_names);
        }
        if instance_names.is_empty() {
            return None;
        }

        if fragment_references_any_name(&snippet.body, &instance_names) {
            return Some(
                named
                    .span
                    .offset(offset)
                    .to_compile_error(self.source, DiagnosticKind::SnippetInvalidExport),
            );
        }

        None
    }

    pub(super) fn malformed_snippet_headers(&self) -> Option<CompileError> {
        let error = self.root.fragment.find_map(|entry| {
            let node = entry.as_node()?;
            let Node::SnippetBlock(block) = node else {
                return None;
            };
            block.header_error.as_ref()
        })?;

        let kind = match error.kind {
            SnippetHeaderErrorKind::ExpectedRightBrace => {
                DiagnosticKind::ExpectedTokenRightBrace
            }
            SnippetHeaderErrorKind::ExpectedRightParen => {
                DiagnosticKind::ExpectedTokenRightParen
            }
        };

        Some(compile_error_with_range(
            self.source,
            kind,
            error.start,
            error.end,
        ))
    }

    pub(super) fn snippet_parameter_assignment(&self) -> Option<CompileError> {
        let mut scope = ScopeStack::default();
        let span = find_snippet_parameter_assignment(&self.root.fragment, &mut scope)?;
        Some(span.to_compile_error(self.source, DiagnosticKind::SnippetParameterAssignment))
    }

    pub(super) fn snippet_invalid_rest_parameter(&self) -> Option<CompileError> {
        let span = find_invalid_rest_parameter(&self.root.fragment)?;
        Some(span.to_compile_error(self.source, DiagnosticKind::SnippetInvalidRestParameter))
    }

    pub(super) fn snippet_children_conflict(&self) -> Option<CompileError> {
        let span = find_children_snippet_conflict(&self.root.fragment)?;
        Some(span.to_compile_error(self.source, DiagnosticKind::SnippetConflict))
    }

    pub(super) fn slot_snippet_conflict(&self) -> Option<CompileError> {
        let span = find_slot_snippet_conflict(&self.root.fragment)?;
        Some(span.to_compile_error(self.source, DiagnosticKind::SlotSnippetConflict))
    }
}

fn find_invalid_rest_parameter(fragment: &Fragment) -> Option<SourceSpan> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let Node::SnippetBlock(block) = node else {
            return None;
        };
        block.parameters.iter().find_map(rest_parameter_span)
    })
}

fn rest_parameter_span(parameter: &Expression) -> Option<SourceSpan> {
    parameter
        .is_rest_parameter()
        .then_some(SourceSpan::new(parameter.start, parameter.end))
}

fn find_children_snippet_conflict(fragment: &Fragment) -> Option<SourceSpan> {
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

fn children_snippet_conflict(nodes: &[Node]) -> Option<SourceSpan> {
    let snippet = nodes.iter().find_map(children_snippet)?;
    has_implicit_children(nodes).then_some(SourceSpan::new(snippet.start, snippet.end))
}

fn children_snippet(node: &Node) -> Option<&SnippetBlock> {
    let Node::SnippetBlock(block) = node else {
        return None;
    };
    let name = block.expression.identifier_name()?;
    (name.as_ref() == "children").then_some(block)
}

fn has_implicit_children(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::SnippetBlock(_) | Node::Comment(_) => false,
        Node::Text(text) => !text.data.trim().is_empty(),
        _ => true,
    })
}

fn find_slot_snippet_conflict(fragment: &Fragment) -> Option<SourceSpan> {
    let has_render = fragment.find_map(|entry| match entry.as_node()? {
        Node::RenderTag(tag) => Some(SourceSpan::new(tag.start, tag.end)),
        _ => None,
    });
    let slot_span = fragment.find_map(|entry| match entry.as_node()? {
        Node::SlotElement(slot) => Some(SourceSpan::new(slot.start, slot.end)),
        _ => None,
    })?;

    has_render.map(|_| slot_span)
}

fn first_exported_name(program: &JsProgram) -> Option<NamedSpan> {
    for statement in &program.program().body {
        let Statement::ExportNamedDeclaration(declaration) = statement else {
            continue;
        };
        if declaration.source.is_some() {
            continue;
        }
        for specifier in &declaration.specifiers {
            let name = Arc::from(specifier.local.name().as_str());
            let span = SourceSpan::from_oxc(specifier.span());
            return Some(NamedSpan::new(name, span));
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
        let snippet_name = block.expression.identifier_name()?;
        (snippet_name.as_ref() == name).then_some(block)
    })
}

fn find_snippet_parameter_assignment(
    fragment: &Fragment,
    scope: &mut ScopeStack,
) -> Option<SourceSpan> {
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
                    scope
                        .contains(identifier.name.as_ref())
                        .then_some(SourceSpan::new(identifier.start, identifier.end))
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
                    scope.push(scope_frame_for_snippet_block(block));
                    None
                }
                Node::KeyBlock(block) => {
                    assignment_to_scoped_name_in_expression(&block.expression, scope)
                }
                _ => node.as_element().and_then(|element| {
                    assignment_to_scoped_name_in_attributes(element.attributes(), scope)
                }),
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
    scope: &ScopeStack,
) -> Option<SourceSpan> {
    for attribute in attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueKind::Boolean(_) => {}
                AttributeValueKind::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) =
                                assignment_to_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
                    if let Some(span) =
                        assignment_to_scoped_name_in_expression(&tag.expression, scope)
                    {
                        return Some(span);
                    }
                }
            },
            Attribute::BindDirective(attribute) => {
                if let Some(name) = attribute.expression.identifier_name()
                    && scope.contains(name.as_ref())
                {
                    return Some(SourceSpan::new(attribute.start, attribute.end));
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
                AttributeValueKind::Boolean(_) => {}
                AttributeValueKind::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) =
                                assignment_to_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
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
    scope: &ScopeStack,
) -> Option<SourceSpan> {
    struct Visitor<'a> {
        scope: &'a ScopeStack,
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_assignment_expression(&mut self, it: &oxc_ast::ast::AssignmentExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if let Some(ident) = it.left.get_identifier_name()
                && self.scope.contains(ident)
            {
                self.found = Some(SourceSpan::new(it.span.start as usize, it.span.end as usize));
                return;
            }
            walk::walk_assignment_expression(self, it);
        }

        fn visit_update_expression(&mut self, it: &oxc_ast::ast::UpdateExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                &it.argument
                && self.scope.contains(identifier.name.as_str())
            {
                self.found = Some(SourceSpan::new(it.span.start as usize, it.span.end as usize));
                return;
            }
            walk::walk_update_expression(self, it);
        }
    }

    if let Some(expression) = expression.oxc_expression() {
        let mut visitor = Visitor { scope, found: None };
        visitor.visit_expression(expression);
        if visitor.found.is_some() {
            return visitor.found;
        }
    }

    if let Some(declaration) = expression.oxc_variable_declaration() {
        for declarator in &declaration.declarations {
            if let Some(init) = declarator.init.as_ref() {
                let mut visitor = Visitor { scope, found: None };
                visitor.visit_expression(init);
                if visitor.found.is_some() {
                    return visitor.found;
                }
            }
        }
    }

    None
}

fn collect_instance_binding_names(program: &JsProgram, out: &mut NameSet) {
    for statement in &program.program().body {
        let Statement::VariableDeclaration(declaration) = statement else {
            continue;
        };
        for declarator in &declaration.declarations {
            extend_name_set_with_oxc_pattern_bindings(out, &declarator.id);
        }
    }
}

fn fragment_references_any_name(fragment: &Fragment, names: &NameSet) -> bool {
    fragment
        .nodes
        .iter()
        .any(|node| node_references_any_name(node, names))
}

fn node_references_any_name(node: &Node, names: &NameSet) -> bool {
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

fn if_block_references_any_name(block: &IfBlock, names: &NameSet) -> bool {
    expression_references_any_name(&block.test, names)
        || fragment_references_any_name(&block.consequent, names)
        || match block.alternate.as_deref() {
            Some(Alternate::Fragment(fragment)) => fragment_references_any_name(fragment, names),
            Some(Alternate::IfBlock(block)) => if_block_references_any_name(block, names),
            None => false,
        }
}

fn expression_references_any_name(expression: &Expression, names: &NameSet) -> bool {
    struct Visitor<'a> {
        names: &'a NameSet,
        found: bool,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if !self.found && self.names.contains(it.name.as_str()) {
                self.found = true;
            }
        }
    }

    if let Some(expression) = expression.oxc_expression() {
        let mut visitor = Visitor { names, found: false };
        visitor.visit_expression(expression);
        if visitor.found {
            return true;
        }
    }

    if let Some(declaration) = expression.oxc_variable_declaration() {
        for declarator in &declaration.declarations {
            if let Some(init) = declarator.init.as_ref() {
                let mut visitor = Visitor { names, found: false };
                visitor.visit_expression(init);
                if visitor.found {
                    return true;
                }
            }
        }
    }

    false
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

    #[test]
    fn allows_binding_to_member_expression_of_snippet_parameter() {
        let error = validate("{#snippet row(item)}<input bind:value={item.value} />{/snippet}");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }
}
