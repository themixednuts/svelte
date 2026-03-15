use super::*;
use crate::api::validation::scope::extend_name_set_with_oxc_pattern_bindings;
use crate::ast::modern::{
    Attribute, AttributeValue, AttributeValueKind, EachBlock, Expression, Fragment, Node, Search,
};
use crate::names::NameStack;
use crate::source::{NamedSpan, SourceSpan};
use oxc_ast::ast::{
    AssignmentTarget, BindingPattern, BlockStatement, CallExpression, CatchClause, ChainElement,
    ClassElement, Declaration, Expression as OxcExpression, ImportDeclarationSpecifier,
    MethodDefinitionKind, ModuleExportName, Program, Statement, VariableDeclaration,
    VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use oxc_syntax::scope::ScopeFlags;
use std::collections::HashMap;
use svelte_syntax::JsProgram;

impl ComponentValidator<'_> {
    pub(super) fn runes_mode_invalid_import(&self) -> Option<CompileError> {
        let script = self.root.instance.as_ref()?;
        let offset = script.content_start;
        if let Some(span) = find_before_update_import_in_program(&script.content) {
            return Some(span.offset(offset).to_compile_error(
                self.source,
                DiagnosticKind::RunesModeInvalidImportBeforeUpdate,
            ));
        }
        None
    }

    pub(super) fn legacy_export_invalid(&self) -> Option<CompileError> {
        let script = self.root.instance.as_ref()?;
        let offset = script.content_start;
        if let Some(span) = find_legacy_export_let_in_program(&script.content) {
            return Some(span.offset(offset).to_compile_error(
                self.source,
                DiagnosticKind::LegacyExportInvalid,
            ));
        }
        None
    }

    pub(super) fn dollar_prefix_invalid(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_dollar_prefix_invalid_in_program(&script.content) {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::DollarPrefixInvalid,
                ));
            }
        }
        None
    }

    pub(super) fn store_invalid_scoped_subscription(&self) -> Option<CompileError> {
        if let Some(instance) = self.root.instance.as_ref()
            && let Some(span) =
                find_store_invalid_scoped_subscription_in_program(&instance.content)
        {
            let offset = instance.content_start;
            return Some(span.offset(offset).to_compile_error(
                self.source,
                DiagnosticKind::StoreInvalidScopedSubscription,
            ));
        }

        let span =
            find_store_invalid_scoped_subscription(&self.root.fragment, &mut AliasStack::default())?;
        Some(span.to_compile_error(
            self.source,
            DiagnosticKind::StoreInvalidScopedSubscription,
        ))
    }

    pub(super) fn store_invalid_subscription(&self) -> Option<CompileError> {
        let module = self.root.module.as_ref()?;
        let offset = module.content_start;
        let span = find_store_invalid_subscription(&module.content)?;
        Some(span.offset(offset).to_compile_error(
            self.source,
            DiagnosticKind::StoreInvalidSubscription,
        ))
    }

    pub(super) fn dollar_binding_error(&self, options: &CompileOptions) -> Option<CompileError> {
        let runes_mode = is_runes_mode(options, self.root);
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) =
                find_dollar_binding_invalid_declaration(&script.content, runes_mode)
            {
                return Some(SourceSpan::new(span.start + offset, span.start + offset + 1).to_compile_error(
                    self.source,
                    DiagnosticKind::DollarBindingInvalid,
                ));
            }
        }
        None
    }

    pub(super) fn global_reference_invalid_markup(&self, runes_mode: bool) -> Option<CompileError> {
        // Collect declarations from both module and instance scripts
        let mut all_declared = NameSet::default();
        for script in [&self.root.module, &self.root.instance] {
            if let Some(script) = script.as_ref() {
                all_declared.extend(collect_declared_names_in_program(&script.content));
            }
        }
        let named = find_invalid_global_reference_in_fragment_with_declared(
            &self.root.fragment,
            runes_mode,
            &all_declared,
        )?;
        Some(named.span.to_compile_error(
            self.source,
            DiagnosticKind::GlobalReferenceInvalid { ident: named.name },
        ))
    }

    pub(super) fn state_in_each_header(&self) -> Option<CompileError> {
        let span = find_state_in_each_header_fragment(&self.root.fragment)?;
        Some(span.to_compile_error(
            self.source,
            DiagnosticKind::StateInvalidPlacement,
        ))
    }

    pub(super) fn rune_missing_parentheses(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            let Some(span) = find_rune_missing_parentheses_in_program(&script.content) else {
                continue;
            };
            return Some(span.offset(offset).to_compile_error(
                self.source,
                DiagnosticKind::RuneMissingParentheses,
            ));
        }
        None
    }

    pub(super) fn each_item_invalid_assignment(&self) -> Option<CompileError> {
        let mut scope = ScopeStack::default();
        let span = find_each_item_invalid_assignment(&self.root.fragment, &mut scope)?;
        Some(span.to_compile_error(
            self.source,
            DiagnosticKind::EachItemInvalidAssignment,
        ))
    }

    pub(super) fn render_tag_errors(&self) -> Option<CompileError> {
        let error = find_render_tag_error_in_fragment(&self.root.fragment)?;
        Some(error.span.to_compile_error(self.source, error.kind))
    }

    pub(super) fn class_state_field_error(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            let Some(error) = find_class_state_field_error_oxc(&script.content) else {
                continue;
            };
            return Some(error.span.offset(offset).to_compile_error(self.source, error.kind));
        }
        None
    }

    pub(super) fn rune_argument_count_errors(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            let Some((kind, span)) = find_invalid_rune_argument_count(&script.content) else {
                continue;
            };
            return Some(span.offset(offset).to_compile_error(self.source, kind));
        }
        None
    }

    pub(super) fn rune_invalid_spread(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            let Some(named) = find_rune_invalid_spread(&script.content) else {
                continue;
            };
            return Some(compile_error_custom(
                self.source,
                "rune_invalid_spread",
                format!("`{}` cannot be called with a spread argument", named.name),
                named.span.start + offset,
                named.span.end + offset,
            ));
        }

        None
    }

    pub(super) fn props_duplicate(&self) -> Option<CompileError> {
        let mut count = 0usize;
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_first_call_span_by_name(&script.content, "$props") {
                count += count_calls_by_name(&script.content, "$props");
                if count > 1 {
                    return Some(span.offset(offset).to_compile_error(
                        self.source,
                        DiagnosticKind::PropsDuplicate,
                    ));
                }
            }
        }
        None
    }

    pub(super) fn props_illegal_name(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_props_illegal_name(&script.content) {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::PropsIllegalName,
                ));
            }
        }
        None
    }

    pub(super) fn bindable_invalid_arguments(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) =
                find_invalid_call_arg_count(&script.content, "$bindable", |c| c <= 1)
            {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::RuneInvalidArgumentsLengthBindable,
                ));
            }
        }
        None
    }

    pub(super) fn props_invalid_arguments(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) =
                find_invalid_call_arg_count(&script.content, "$props", |c| c == 0)
            {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::RuneInvalidArgumentsProps,
                ));
            }
        }
        None
    }

    pub(super) fn props_invalid_placement(&self) -> Option<CompileError> {
        let instance = self.root.instance.as_ref()?;
        let offset = instance.content_start;
        let span = find_props_invalid_placement_component(&instance.content)?;
        Some(span.offset(offset).to_compile_error(
            self.source,
            DiagnosticKind::PropsInvalidPlacement,
        ))
    }

    pub(super) fn bindable_invalid_location(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_bindable_invalid_location(&script.content) {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::BindableInvalidLocation,
                ));
            }
        }
        None
    }

    pub(super) fn derived_invalid_placement(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_invalid_initializer_placement(&script.content, "$derived")
            {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::StateInvalidPlacementDerived,
                ));
            }
        }
        None
    }

    pub(super) fn effect_invalid_placement(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_effect_invalid_placement(&script.content) {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::EffectInvalidPlacement,
                ));
            }
        }
        None
    }

    pub(super) fn host_invalid_placement(&self) -> Option<CompileError> {
        if self.root
            .options
            .as_ref()
            .and_then(|options| options.custom_element.as_ref())
            .is_some()
        {
            return None;
        }
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_first_call_span_by_name(&script.content, "$host") {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::HostInvalidPlacement,
                ));
            }
        }
        None
    }

    pub(super) fn state_invalid_placement_general(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_invalid_initializer_placement(&script.content, "$state") {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::StateInvalidPlacement,
                ));
            }
        }
        None
    }

    pub(super) fn state_invalid_placement(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_static_state_call(&script.content) {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::StateInvalidPlacement,
                ));
            }
        }
        None
    }

    pub(super) fn invalid_rune_name(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(named) = find_invalid_rune_name(&script.content) {
                return Some(named.span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::RuneInvalidName { name: named.name },
                ));
            }
        }
        None
    }

    pub(super) fn constant_assignment(&self) -> Option<CompileError> {
        let mut immutables = NameSet::default();
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            collect_script_immutable_bindings(&script.content, &mut immutables);
        }

        let mut found = None;
        self.root.fragment.walk(
            &mut found,
            |entry, found| {
                if found.is_some() {
                    return Search::Found(());
                }
                let Some(node) = entry.as_node() else {
                    return Search::Continue;
                };
                let find_with_offset = |expr: &Expression| {
                    find_constant_assignment_in_expression(expr, &immutables)
                        .map(|s| s.offset(expr.start))
                };
                let span = match node {
                    Node::ExpressionTag(tag) => find_with_offset(&tag.expression),
                    Node::RenderTag(tag) => find_with_offset(&tag.expression),
                    Node::HtmlTag(tag) => find_with_offset(&tag.expression),
                    Node::ConstTag(tag) => find_with_offset(&tag.declaration),
                    Node::EachBlock(block) => find_with_offset(&block.expression)
                        .or_else(|| {
                            block
                                .key
                                .as_ref()
                                .and_then(&find_with_offset)
                        })
                        .or_else(|| {
                            block
                                .context
                                .as_ref()
                                .and_then(&find_with_offset)
                        }),
                    Node::AwaitBlock(block) => find_with_offset(&block.expression)
                        .or_else(|| {
                            block
                                .value
                                .as_ref()
                                .and_then(&find_with_offset)
                        })
                        .or_else(|| {
                            block
                                .error
                                .as_ref()
                                .and_then(&find_with_offset)
                        }),
                    Node::SnippetBlock(block) => block
                        .parameters
                        .iter()
                        .find_map(&find_with_offset),
                    Node::KeyBlock(block) => find_with_offset(&block.expression),
                    _ => None,
                };
                if let Some(span) = span {
                    *found = Some(span);
                    Search::Found(())
                } else {
                    Search::Continue
                }
            },
            |_, _| {},
        );

        let span = found?;
        Some(span.to_compile_error(
            self.source,
            DiagnosticKind::ConstantAssignment,
        ))
    }

    pub(super) fn constant_assignment_in_scripts(&self) -> Option<CompileError> {
        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(span) = find_constant_assignment_in_program(&script.content) {
                return Some(span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::ConstantAssignment,
                ));
            }
        }
        None
    }

    pub(super) fn global_reference_invalid_in_scripts(&self, check_store_refs: bool) -> Option<CompileError> {
        // Collect all top-level declarations across both scripts so that
        // store subscriptions like `$foo` in the instance can find `foo`
        // declared/imported in the module script (and vice versa).
        let mut all_declared = NameSet::default();
        for script in [&self.root.module, &self.root.instance] {
            if let Some(script) = script.as_ref() {
                let names = collect_declared_names_in_program(&script.content);
                all_declared.extend(names);
            }
        }
        // In legacy mode, `$:` reactive labels create store subscriptions
        // (e.g. `$: $foo;` declares `$foo`). Add these to the declared set
        // so they aren't flagged as invalid global references.
        if let Some(instance) = self.root.instance.as_ref() {
            collect_dollar_label_store_subscriptions(&mut all_declared, instance.oxc_program());
        }

        for script in [&self.root.module, &self.root.instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            if let Some(named) =
                find_global_reference_invalid_in_program_with_extra_declared(
                    &script.content,
                    &all_declared,
                    check_store_refs,
                )
            {
                return Some(named.span.offset(offset).to_compile_error(
                    self.source,
                    DiagnosticKind::GlobalReferenceInvalid { ident: named.name },
                ));
            }
        }
        None
    }
}

fn find_each_item_invalid_assignment(
    fragment: &Fragment,
    scope: &mut ScopeStack,
) -> Option<SourceSpan> {
    fragment.walk(
        scope,
        |entry, scope| {
            if let Some(block) = entry.as_if_block() {
                return match assignment_to_each_scoped_name_in_expression(&block.test, scope) {
                    Some(span) => Search::Found(span),
                    None => Search::Continue,
                };
            }

            let Some(node) = entry.as_node() else {
                return Search::Continue;
            };

            let found = match node {
                Node::Text(_) | Node::Comment(_) | Node::DebugTag(_) => None,
                Node::ExpressionTag(tag) => {
                    assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                }
                Node::RenderTag(tag) => {
                    assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                }
                Node::HtmlTag(tag) => {
                    assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                }
                Node::ConstTag(tag) => {
                    assignment_to_each_scoped_name_in_expression(&tag.declaration, scope)
                }
                Node::IfBlock(_) => None,
                Node::EachBlock(block) => {
                    if let Some(span) =
                        assignment_to_each_scoped_name_in_expression(&block.expression, scope)
                    {
                        return Search::Found(span);
                    }

                    if let Some(span) =
                        scope.with_frame(scope_frame_for_each_block(block), |scope| {
                            if let Some(key) = block.key.as_ref()
                                && let Some(span) =
                                    assignment_to_each_scoped_name_in_expression(key, scope)
                            {
                                return Some(span);
                            }

                            find_each_item_invalid_assignment(&block.body, scope)
                        })
                    {
                        return Search::Found(span);
                    }

                    if let Some(fallback) = block.fallback.as_ref()
                        && let Some(span) = find_each_item_invalid_assignment(fallback, scope)
                    {
                        return Search::Found(span);
                    }

                    return Search::Skip;
                }
                Node::AwaitBlock(block) => {
                    assignment_to_each_scoped_name_in_expression(&block.expression, scope)
                        .or_else(|| {
                            block.value.as_ref().and_then(|value| {
                                assignment_to_each_scoped_name_in_expression(value, scope)
                            })
                        })
                        .or_else(|| {
                            block.error.as_ref().and_then(|error| {
                                assignment_to_each_scoped_name_in_expression(error, scope)
                            })
                        })
                }
                Node::SnippetBlock(_) => None,
                Node::KeyBlock(block) => {
                    assignment_to_each_scoped_name_in_expression(&block.expression, scope)
                }
                _ => node.as_element().and_then(|element| {
                    assignment_to_each_scoped_name_in_attributes(element.attributes(), scope)
                }),
            };

            match found {
                Some(span) => Search::Found(span),
                None => Search::Continue,
            }
        },
        |_, _| {},
    )
}

fn assignment_to_each_scoped_name_in_attributes(
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
                                assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
                    if let Some(span) =
                        assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                    {
                        return Some(span);
                    }
                }
            },
            Attribute::BindDirective(attribute) => {
                if attribute
                    .expression
                    .identifier_name()
                    .is_some_and(|name| scope.contains(name.as_ref()))
                {
                    return Some(SourceSpan::new(attribute.start, attribute.end));
                }
                if let Some(span) =
                    assignment_to_each_scoped_name_in_expression(&attribute.expression, scope)
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
                    assignment_to_each_scoped_name_in_expression(&attribute.expression, scope)
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
                                assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
                    if let Some(span) =
                        assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                    {
                        return Some(span);
                    }
                }
            },
            Attribute::TransitionDirective(attribute) => {
                if let Some(span) =
                    assignment_to_each_scoped_name_in_expression(&attribute.expression, scope)
                {
                    return Some(span);
                }
            }
            Attribute::AttachTag(tag) => {
                if let Some(span) =
                    assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                {
                    return Some(span);
                }
            }
            Attribute::SpreadAttribute(spread) => {
                if let Some(span) =
                    assignment_to_each_scoped_name_in_expression(&spread.expression, scope)
                {
                    return Some(span);
                }
            }
        }
    }
    None
}

fn assignment_to_each_scoped_name_in_expression(
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
                self.found = Some(span_range(it.span));
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
                self.found = Some(span_range(it.span));
                return;
            }
            walk::walk_update_expression(self, it);
        }
    }

    if let Some(oxc_expression) = expression.oxc_expression() {
        let mut visitor = Visitor { scope, found: None };
        visitor.visit_expression(oxc_expression);
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


impl ScriptValidator<'_> {
    pub(super) fn invalid_name(&self) -> Option<CompileError> {
        let named = find_invalid_rune_name(self.program())?;
        Some(named.span.to_compile_error(
            self.source(),
            DiagnosticKind::RuneInvalidName { name: named.name },
        ))
    }

    pub(super) fn renamed_effect_active(&self) -> Option<CompileError> {
        let span = find_renamed_effect_active_oxc(self.program())?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::RuneRenamedEffectActive,
        ))
    }

    pub(super) fn store_invalid_subscription_module(&self) -> Option<CompileError> {
        let span = find_store_invalid_subscription(self.program())?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::StoreInvalidSubscriptionModule,
        ))
    }

    pub(super) fn dollar_binding_error(&self, runes_mode: bool) -> Option<CompileError> {
        detect_dollar_binding_error_in_program(self.source(), self.program(), runes_mode)
    }

    pub(super) fn constant_assignment(&self) -> Option<CompileError> {
        let span = find_constant_assignment_in_program(self.program())?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::ConstantAssignment,
        ))
    }

    pub(super) fn bindable_invalid_location(&self) -> Option<CompileError> {
        let span = find_bindable_invalid_location(self.program())?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::BindableInvalidLocation,
        ))
    }

    pub(super) fn rune_argument_count(&self) -> Option<CompileError> {
        let (kind, span) = find_invalid_rune_argument_count(self.program())?;
        Some(span.to_compile_error(self.source(), kind))
    }

    pub(super) fn state_invalid_placement(&self) -> Option<CompileError> {
        detect_initializer_placement(
            self.source(),
            self.program(),
            "$state",
            DiagnosticKind::StateInvalidPlacement,
        )
    }

    pub(super) fn derived_invalid_placement(&self) -> Option<CompileError> {
        detect_initializer_placement(
            self.source(),
            self.program(),
            "$derived",
            DiagnosticKind::StateInvalidPlacementDerived,
        )
    }

    pub(super) fn effect_invalid_placement(&self) -> Option<CompileError> {
        let span = find_effect_invalid_placement(self.program())?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::EffectInvalidPlacement,
        ))
    }

    pub(super) fn host_invalid_placement(&self) -> Option<CompileError> {
        let span = find_first_call_span_by_name(self.program(), "$host")?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::HostInvalidPlacement,
        ))
    }

    pub(super) fn class_state_field_error(&self) -> Option<CompileError> {
        let error = find_class_state_field_error_oxc(self.program())?;
        Some(error.span.to_compile_error(self.source(), error.kind))
    }

    pub(super) fn props_invalid_placement_module(&self) -> Option<CompileError> {
        let span = find_first_call_span_by_name(self.program(), "$props")?;
        Some(span.to_compile_error(
            self.source(),
            DiagnosticKind::PropsInvalidPlacement,
        ))
    }

    pub(super) fn global_reference_invalid_module(&self) -> Option<CompileError> {
        let named = find_global_reference_invalid_in_program(self.program())?;
        Some(named.span.to_compile_error(
            self.source(),
            DiagnosticKind::GlobalReferenceInvalid { ident: named.name },
        ))
    }
}

fn detect_initializer_placement(
    source: &str,
    program: &JsProgram,
    call_name: &str,
    kind: DiagnosticKind,
) -> Option<CompileError> {
    let span = find_invalid_initializer_placement(program, call_name)?;
    Some(span.to_compile_error(source, kind))
}

fn find_before_update_import_in_program(program: &JsProgram) -> Option<SourceSpan> {
    for statement in &program.program().body {
        let Statement::ImportDeclaration(declaration) = statement else {
            continue;
        };
        if declaration.source.value.as_str() != "svelte" {
            continue;
        }
        let Some(specifiers) = declaration.specifiers.as_ref() else {
            continue;
        };
        for specifier in specifiers {
            let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier else {
                continue;
            };
            if module_export_name_as_str(&specifier.imported) != Some("beforeUpdate") {
                continue;
            }
            return Some(span_range(specifier.local.span));
        }
    }
    None
}

fn find_legacy_export_let_in_program(program: &JsProgram) -> Option<SourceSpan> {
    for statement in &program.program().body {
        let Statement::ExportNamedDeclaration(declaration) = statement else {
            continue;
        };
        if declaration.source.is_some() {
            continue;
        }
        let Some(Declaration::VariableDeclaration(variable)) = declaration.declaration.as_ref()
        else {
            continue;
        };
        if variable.kind != VariableDeclarationKind::Let {
            continue;
        }
        let decl_span = span_range(declaration.span);
        let end = (decl_span.start + "export let".len()).min(decl_span.end);
        return Some(SourceSpan::new(decl_span.start, end));
    }
    None
}

fn find_dollar_prefix_invalid_in_program(program: &JsProgram) -> Option<SourceSpan> {
    for statement in &program.program().body {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                if let Some(span) = find_invalid_dollar_in_variable_declaration(declaration) {
                    return Some(span);
                }
            }
            Statement::ImportDeclaration(declaration) => {
                let Some(specifiers) = declaration.specifiers.as_ref() else {
                    continue;
                };
                for specifier in specifiers {
                    let Some(name) = import_specifier_local_name(specifier) else {
                        continue;
                    };
                    if is_dollar_prefixed_invalid_identifier(name) {
                        return Some(span_range(specifier.span()));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn find_invalid_dollar_in_variable_declaration(
    declaration: &VariableDeclaration<'_>,
) -> Option<SourceSpan> {
    for declarator in &declaration.declarations {
        let Some(name) = declarator.id.get_binding_identifier() else {
            continue;
        };
        if is_dollar_prefixed_invalid_identifier(name.name.as_str()) {
            return Some(SourceSpan::new(name.span.start as usize, name.span.start as usize + 1));
        }
    }
    None
}

fn span_range(span: Span) -> SourceSpan {
    SourceSpan::from_oxc(span)
}

/// Check if any identifier in an assignment target refers to an immutable binding.
fn has_immutable_assignment_target(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    immutables: &NameSet,
    locals: &[NameSet],
) -> bool {
    let is_local = |name: &str| locals.iter().rev().any(|scope| scope.contains(name));
    match target {
        oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
            let name = id.name.as_str();
            !is_local(name) && immutables.contains(name)
        }
        oxc_ast::ast::AssignmentTarget::ArrayAssignmentTarget(arr) => {
            arr.elements.iter().any(|el| {
                el.as_ref().is_some_and(|maybe_default| {
                    has_immutable_maybe_default_target(maybe_default, immutables, locals)
                })
            }) || arr.rest.as_ref().is_some_and(|rest| {
                has_immutable_assignment_target(&rest.target, immutables, locals)
            })
        }
        oxc_ast::ast::AssignmentTarget::ObjectAssignmentTarget(obj) => {
            obj.properties.iter().any(|prop| match prop {
                oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                    let name = id.binding.name.as_str();
                    !is_local(name) && immutables.contains(name)
                }
                oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                    has_immutable_maybe_default_target(&prop.binding, immutables, locals)
                }
            }) || obj.rest.as_ref().is_some_and(|rest| {
                has_immutable_assignment_target(&rest.target, immutables, locals)
            })
        }
        _ => false,
    }
}

fn has_immutable_maybe_default_target(
    target: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
    immutables: &NameSet,
    locals: &[NameSet],
) -> bool {
    match target {
        oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
            has_immutable_assignment_target(&with_default.binding, immutables, locals)
        }
        _ => {
            // The @inherit AssignmentTarget variants are accessed via as_assignment_target()
            if let Some(inner) = target.as_assignment_target() {
                has_immutable_assignment_target(inner, immutables, locals)
            } else {
                false
            }
        }
    }
}

fn oxc_callee_name(callee: &OxcExpression<'_>) -> Option<String> {
    match callee.get_inner_expression() {
        OxcExpression::Identifier(reference) => Some(reference.name.to_string()),
        OxcExpression::StaticMemberExpression(member) => {
            let object = member.object.get_inner_expression();
            let OxcExpression::Identifier(object) = object else {
                return None;
            };
            Some(format!("{}.{}", object.name, member.property.name))
        }
        _ => None,
    }
}

fn import_specifier_local_name<'a>(
    specifier: &'a ImportDeclarationSpecifier<'a>,
) -> Option<&'a str> {
    match specifier {
        ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
    }
}

fn module_export_name_as_str<'a>(name: &'a ModuleExportName<'a>) -> Option<&'a str> {
    match name {
        ModuleExportName::IdentifierName(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::StringLiteral(_) => None,
    }
}

#[derive(Debug)]
struct ClassStateFieldError {
    kind: DiagnosticKind,
    span: SourceSpan,
}

fn is_dollar_prefixed_invalid_identifier(name: &str) -> bool {
    if name.len() <= 1 || !name.starts_with('$') {
        return false;
    }
    let second = name.as_bytes()[1];
    second == b'_' || second.is_ascii_alphabetic()
}

fn find_rune_invalid_spread(program: &JsProgram) -> Option<NamedSpan> {
    struct Visitor {
        found: Option<NamedSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            let Some(name) = oxc_callee_name(&it.callee) else {
                walk::walk_call_expression(self, it);
                return;
            };
            if !matches!(
                name.as_str(),
                "$derived" | "$derived.by" | "$state" | "$state.raw"
            ) {
                walk::walk_call_expression(self, it);
                return;
            }
            if it.arguments.iter().any(|argument| argument.is_spread()) {
                self.found = Some(NamedSpan::new(Arc::from(name), span_range(it.span)));
                return;
            }
            walk::walk_call_expression(self, it);
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_first_call_span_by_name(program: &JsProgram, name: &str) -> Option<SourceSpan> {
    struct Visitor<'n> {
        name: &'n str,
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if oxc_callee_name(&it.callee).as_deref() == Some(self.name) {
                self.found = Some(span_range(it.span));
                return;
            }
            walk::walk_call_expression(self, it);
        }
    }

    let mut visitor = Visitor { name, found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn count_calls_by_name(program: &JsProgram, name: &str) -> usize {
    struct Visitor<'n> {
        name: &'n str,
        count: usize,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if oxc_callee_name(&it.callee).as_deref() == Some(self.name) {
                self.count += 1;
            }
            walk::walk_call_expression(self, it);
        }
    }

    let mut visitor = Visitor { name, count: 0 };
    visitor.visit_program(program.program());
    visitor.count
}

fn find_props_illegal_name(program: &JsProgram) -> Option<SourceSpan> {
    let mut props_rest_bindings = NameSet::default();
    let mut found = None::<SourceSpan>;
    for statement in &program.program().body {
        let Statement::VariableDeclaration(declaration) = statement else {
            continue;
        };
        for declarator in &declaration.declarations {
            let Some(init) = declarator.init.as_ref() else {
                continue;
            };
            let OxcExpression::CallExpression(call) = init.get_inner_expression() else {
                continue;
            };
            if oxc_callee_name(&call.callee).as_deref() != Some("$props") {
                continue;
            }
            match &declarator.id {
                BindingPattern::BindingIdentifier(identifier) => {
                    props_rest_bindings.insert(Arc::from(identifier.name.as_str()));
                }
                BindingPattern::ObjectPattern(pattern) => {
                    for property in &pattern.properties {
                        if let Some(name) = property.key.static_name()
                            && name.starts_with("$$")
                        {
                            found = Some(span_range(property.key.span()));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                    if let Some(rest) = pattern.rest.as_ref()
                        && let BindingPattern::BindingIdentifier(identifier) = &rest.argument
                    {
                        props_rest_bindings.insert(Arc::from(identifier.name.as_str()));
                    }
                }
                _ => {}
            }
        }
        if found.is_some() {
            break;
        }
    }

    if found.is_some() {
        return found;
    }

    struct Visitor {
        props_rest_bindings: NameSet,
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_member_expression(&mut self, it: &oxc_ast::ast::MemberExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            let Some(object) = it.object().get_identifier_reference() else {
                walk::walk_member_expression(self, it);
                return;
            };
            if !self.props_rest_bindings.contains(object.name.as_str()) {
                walk::walk_member_expression(self, it);
                return;
            }
            let Some((span, property_name)) = it.static_property_info() else {
                walk::walk_member_expression(self, it);
                return;
            };
            if property_name.starts_with("$$") {
                self.found = Some(span_range(span));
                return;
            }
            walk::walk_member_expression(self, it);
        }
    }

    let mut visitor = Visitor {
        props_rest_bindings,
        found: None,
    };
    visitor.visit_program(program.program());
    found = visitor.found;

    found
}

fn find_invalid_call_arg_count(
    program: &JsProgram,
    name: &str,
    is_valid: impl Fn(usize) -> bool,
) -> Option<SourceSpan> {
    struct Visitor<'n, F> {
        name: &'n str,
        is_valid: F,
        found: Option<SourceSpan>,
    }

    impl<'a, F: Fn(usize) -> bool> Visit<'a> for Visitor<'_, F> {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if oxc_callee_name(&it.callee).as_deref() == Some(self.name)
                && !(self.is_valid)(it.arguments.len())
            {
                self.found = Some(span_range(it.span));
                return;
            }
            walk::walk_call_expression(self, it);
        }
    }

    let mut visitor = Visitor {
        name,
        is_valid,
        found: None,
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_invalid_rune_argument_count(
    program: &JsProgram,
) -> Option<(DiagnosticKind, SourceSpan)> {
    struct Visitor {
        found: Option<(DiagnosticKind, SourceSpan)>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            let Some(name) = oxc_callee_name(&it.callee) else {
                walk::walk_call_expression(self, it);
                return;
            };
            let Some(kind) = invalid_rune_argument_kind(name.as_str(), it.arguments.len()) else {
                walk::walk_call_expression(self, it);
                return;
            };
            self.found = Some((kind, span_range(it.span)));
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn invalid_rune_argument_kind(name: &str, arg_count: usize) -> Option<DiagnosticKind> {
    match name {
        "$derived" if arg_count != 1 => {
            Some(DiagnosticKind::RuneInvalidArgumentsLengthDerived)
        }
        "$effect" if arg_count != 1 => {
            Some(DiagnosticKind::RuneInvalidArgumentsLengthEffect)
        }
        "$state.raw" if arg_count > 1 => {
            Some(DiagnosticKind::RuneInvalidArgumentsLengthStateRaw)
        }
        "$state.snapshot" if arg_count != 1 => {
            Some(DiagnosticKind::RuneInvalidArgumentsLengthStateSnapshot)
        }
        "$state" if arg_count > 1 => Some(DiagnosticKind::RuneInvalidArgumentsLengthState),
        _ => None,
    }
}

fn collect_top_level_initializer_call_spans(
    program: &JsProgram,
    call_name: &str,
) -> std::collections::BTreeSet<SourceSpan> {
    let mut spans = std::collections::BTreeSet::new();
    for statement in &program.program().body {
        let variable = match statement {
            Statement::VariableDeclaration(declaration) => declaration.as_ref(),
            Statement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
                Some(Declaration::VariableDeclaration(declaration)) => declaration.as_ref(),
                _ => continue,
            },
            _ => continue,
        };
        for declarator in &variable.declarations {
            let Some(init) = declarator.init.as_ref() else {
                continue;
            };
            let OxcExpression::CallExpression(call) = init.get_inner_expression() else {
                continue;
            };
            if oxc_callee_name(&call.callee).as_deref() == Some(call_name) {
                spans.insert(span_range(call.span));
            }
        }
    }
    spans
}

fn collect_all_expression_statement_call_spans(
    program: &JsProgram,
    call_name: &str,
) -> std::collections::BTreeSet<SourceSpan> {
    struct Visitor<'a> {
        call_name: &'a str,
        spans: std::collections::BTreeSet<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_expression_statement(
            &mut self,
            it: &oxc_ast::ast::ExpressionStatement<'a>,
        ) {
            if let OxcExpression::CallExpression(call) = it.expression.get_inner_expression()
                && oxc_callee_name(&call.callee).as_deref() == Some(self.call_name)
            {
                self.spans.insert(span_range(call.span));
            }
            walk::walk_expression_statement(self, it);
        }
    }

    let mut visitor = Visitor {
        call_name,
        spans: std::collections::BTreeSet::new(),
    };
    visitor.visit_program(program.program());
    visitor.spans
}

fn collect_allowed_initializer_call_spans(
    program: &JsProgram,
    call_name: &str,
) -> std::collections::BTreeSet<SourceSpan> {
    let mut spans = collect_top_level_initializer_call_spans(program, call_name);

    struct Visitor<'a> {
        call_name: &'a str,
        spans: &'a mut std::collections::BTreeSet<SourceSpan>,
    }

    impl<'a, 'b> Visit<'a> for Visitor<'b> {
        fn visit_property_definition(&mut self, it: &oxc_ast::ast::PropertyDefinition<'a>) {
            if let Some(value) = it.value.as_ref()
                && let OxcExpression::CallExpression(call) = value.get_inner_expression()
                && oxc_callee_name(&call.callee).as_deref() == Some(self.call_name)
            {
                self.spans.insert(span_range(call.span));
            }
            walk::walk_property_definition(self, it);
        }

        fn visit_method_definition(&mut self, it: &oxc_ast::ast::MethodDefinition<'a>) {
            // For constructors, only allow `this.x = $state()` as DIRECT body statements
            if it.kind == oxc_ast::ast::MethodDefinitionKind::Constructor
                && let Some(body) = it.value.body.as_ref()
            {
                for statement in &body.statements {
                    if let Statement::ExpressionStatement(stmt) = statement
                        && let OxcExpression::AssignmentExpression(assign) =
                            &stmt.expression
                        && matches!(
                            assign.left,
                            oxc_ast::ast::AssignmentTarget::StaticMemberExpression(_)
                                | oxc_ast::ast::AssignmentTarget::PrivateFieldExpression(_)
                                | oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(_)
                        )
                        && let Some(member) = assign.left.as_member_expression()
                        && matches!(member.object(), OxcExpression::ThisExpression(_))
                        && let OxcExpression::CallExpression(call) =
                            assign.right.get_inner_expression()
                        && oxc_callee_name(&call.callee).as_deref() == Some(self.call_name)
                    {
                        self.spans.insert(span_range(call.span));
                    }
                }
            }
            walk::walk_method_definition(self, it);
        }
    }

    let mut visitor = Visitor {
        call_name,
        spans: &mut spans,
    };
    visitor.visit_program(program.program());
    spans
}

fn collect_allowed_bindable_call_spans(
    program: &JsProgram,
) -> std::collections::BTreeSet<SourceSpan> {
    let mut spans = std::collections::BTreeSet::new();
    for statement in &program.program().body {
        let Statement::VariableDeclaration(declaration) = statement else {
            continue;
        };
        for declarator in &declaration.declarations {
            let Some(init) = declarator.init.as_ref() else {
                continue;
            };
            let OxcExpression::CallExpression(call) = init.get_inner_expression() else {
                continue;
            };
            if oxc_callee_name(&call.callee).as_deref() != Some("$props") {
                continue;
            }
            let BindingPattern::ObjectPattern(pattern) = &declarator.id else {
                continue;
            };
            collect_bindable_spans_from_object_pattern(pattern, &mut spans);
        }
    }
    spans
}

fn collect_bindable_spans_from_object_pattern(
    pattern: &oxc_ast::ast::ObjectPattern<'_>,
    spans: &mut std::collections::BTreeSet<SourceSpan>,
) {
    for property in &pattern.properties {
        collect_bindable_spans_from_pattern(&property.value, spans);
    }
    if let Some(rest) = pattern.rest.as_ref() {
        collect_bindable_spans_from_pattern(&rest.argument, spans);
    }
}

fn collect_bindable_spans_from_pattern(
    pattern: &BindingPattern<'_>,
    spans: &mut std::collections::BTreeSet<SourceSpan>,
) {
    match pattern {
        BindingPattern::AssignmentPattern(pattern) => {
            if let OxcExpression::CallExpression(call) = pattern.right.get_inner_expression()
                && oxc_callee_name(&call.callee).as_deref() == Some("$bindable")
            {
                spans.insert(span_range(call.span));
            }
            collect_bindable_spans_from_pattern(&pattern.left, spans);
        }
        BindingPattern::ObjectPattern(pattern) => {
            collect_bindable_spans_from_object_pattern(pattern, spans)
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_bindable_spans_from_pattern(element, spans);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_bindable_spans_from_pattern(&rest.argument, spans);
            }
        }
        BindingPattern::BindingIdentifier(_) => {}
    }
}

fn find_props_invalid_placement_component(program: &JsProgram) -> Option<SourceSpan> {
    let allowed = collect_top_level_initializer_call_spans(program, "$props");
    find_first_call_span_by_name(program, "$props").filter(|span| !allowed.contains(span))
}

fn find_bindable_invalid_location(program: &JsProgram) -> Option<SourceSpan> {
    let allowed = collect_allowed_bindable_call_spans(program);
    find_first_call_span_by_name(program, "$bindable").filter(|span| !allowed.contains(span))
}

fn find_invalid_initializer_placement(
    program: &JsProgram,
    call_name: &str,
) -> Option<SourceSpan> {
    let allowed = collect_allowed_initializer_call_spans(program, call_name);

    struct Visitor<'n> {
        name: &'n str,
        allowed: &'n std::collections::BTreeSet<SourceSpan>,
        found: Option<SourceSpan>,
        function_depth: usize,
        in_constructor: bool,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if (self.function_depth == 0 || self.in_constructor)
                && oxc_callee_name(&it.callee).as_deref() == Some(self.name)
            {
                let span = span_range(it.span);
                if !self.allowed.contains(&span) {
                    self.found = Some(span);
                    return;
                }
            }
            walk::walk_call_expression(self, it);
        }

        fn visit_method_definition(&mut self, it: &oxc_ast::ast::MethodDefinition<'a>) {
            if it.kind == oxc_ast::ast::MethodDefinitionKind::Constructor {
                let prev = self.in_constructor;
                self.in_constructor = true;
                // Walk the constructor function body directly without incrementing function_depth
                if let Some(body) = it.value.body.as_ref() {
                    self.visit_function_body(body);
                }
                self.in_constructor = prev;
            } else {
                walk::walk_method_definition(self, it);
            }
        }

        fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            let prev_constructor = self.in_constructor;
            self.in_constructor = false;
            self.function_depth += 1;
            walk::walk_function(self, it, flags);
            self.function_depth -= 1;
            self.in_constructor = prev_constructor;
        }

        fn visit_arrow_function_expression(
            &mut self,
            it: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            let prev_constructor = self.in_constructor;
            self.in_constructor = false;
            self.function_depth += 1;
            walk::walk_arrow_function_expression(self, it);
            self.function_depth -= 1;
            self.in_constructor = prev_constructor;
        }
    }

    let mut visitor = Visitor {
        name: call_name,
        allowed: &allowed,
        found: None,
        function_depth: 0,
        in_constructor: false,
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_effect_invalid_placement(program: &JsProgram) -> Option<SourceSpan> {
    // Collect all $effect() calls that are direct expression statements (allowed at any depth).
    let allowed = collect_all_expression_statement_call_spans(program, "$effect");

    struct Visitor<'a> {
        allowed: &'a std::collections::BTreeSet<SourceSpan>,
        found: Option<SourceSpan>,
        scopes: Vec<NameSet>,
    }

    impl Visitor<'_> {
        fn is_shadowed(&self, name: &str) -> bool {
            self.scopes.iter().rev().any(|scope| scope.contains(name))
        }
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if oxc_callee_name(&it.callee).as_deref() == Some("$effect")
                && !self.is_shadowed("$effect")
            {
                let span = span_range(it.span);
                if !self.allowed.contains(&span) {
                    self.found = Some(span);
                    return;
                }
            }
            walk::walk_call_expression(self, it);
        }

        fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            let mut scope = NameSet::default();
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.scopes.push(scope);
            walk::walk_function(self, it, flags);
            self.scopes.pop();
        }

        fn visit_arrow_function_expression(
            &mut self,
            it: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            let mut scope = NameSet::default();
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.scopes.push(scope);
            walk::walk_arrow_function_expression(self, it);
            self.scopes.pop();
        }

        fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
            if let Some(scope) = self.scopes.last_mut() {
                extend_name_set_with_oxc_pattern_bindings(scope, &it.id);
            }
            walk::walk_variable_declarator(self, it);
        }
    }

    let mut visitor = Visitor {
        allowed: &allowed,
        found: None,
        scopes: Vec::new(),
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_static_state_call(program: &JsProgram) -> Option<SourceSpan> {
    for statement in &program.program().body {
        let Statement::ClassDeclaration(class) = statement else {
            continue;
        };
        for element in &class.body.body {
            let oxc_ast::ast::ClassElement::PropertyDefinition(property) = element else {
                continue;
            };
            if !property.r#static {
                continue;
            }
            let Some(value) = property.value.as_ref() else {
                continue;
            };
            let OxcExpression::CallExpression(call) = value.get_inner_expression() else {
                continue;
            };
            if oxc_callee_name(&call.callee).as_deref() == Some("$state") {
                return Some(span_range(call.span));
            }
        }
    }
    None
}

fn find_rune_missing_parentheses_in_program(program: &JsProgram) -> Option<SourceSpan> {
    struct Visitor {
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            for argument in &it.arguments {
                self.visit_argument(argument);
                if self.found.is_some() {
                    return;
                }
            }
        }

        fn visit_member_expression(&mut self, it: &oxc_ast::ast::MemberExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            match it {
                oxc_ast::ast::MemberExpression::ComputedMemberExpression(expr) => {
                    self.visit_expression(&expr.object);
                    self.visit_expression(&expr.expression);
                }
                oxc_ast::ast::MemberExpression::StaticMemberExpression(expr) => {
                    if expr
                        .object
                        .get_identifier_reference()
                        .is_none_or(|ident| !matches!(ident.name.as_str(), "$bindable" | "$props"))
                    {
                        self.visit_expression(&expr.object);
                    }
                }
                oxc_ast::ast::MemberExpression::PrivateFieldExpression(expr) => {
                    self.visit_expression(&expr.object);
                }
            }
        }

        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if self.found.is_none() && matches!(it.name.as_str(), "$bindable" | "$props") {
                self.found = Some(span_range(it.span));
            }
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_invalid_rune_name(program: &JsProgram) -> Option<NamedSpan> {
    struct Visitor {
        found: Option<NamedSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_member_expression(&mut self, it: &oxc_ast::ast::MemberExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            let Some(object) = it.object().get_identifier_reference() else {
                walk::walk_member_expression(self, it);
                return;
            };
            let Some((span, property_name)) = it.static_property_info() else {
                walk::walk_member_expression(self, it);
                return;
            };
            if object.name.as_str() == "$state" && !matches!(property_name, "raw" | "snapshot") {
                self.found = Some(NamedSpan::new(
                    Arc::from(format!("{}.{}", object.name, property_name)),
                    span_range(it.span()),
                ));
                return;
            }
            if object.name.as_str() == "$effect"
                && !matches!(property_name, "active" | "pre" | "tracking" | "root")
            {
                self.found = Some(NamedSpan::new(
                    Arc::from(format!("{}.{}", object.name, property_name)),
                    span_range(it.span()),
                ));
                return;
            }
            let _ = span;
            walk::walk_member_expression(self, it);
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_renamed_effect_active_oxc(program: &JsProgram) -> Option<SourceSpan> {
    struct Visitor {
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_member_expression(&mut self, it: &oxc_ast::ast::MemberExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if let Some(object) = it.object().get_identifier_reference()
                && object.name.as_str() == "$effect"
                && let Some((_, property_name)) = it.static_property_info()
                && property_name == "active"
            {
                self.found = Some(span_range(it.span()));
                return;
            }
            walk::walk_member_expression(self, it);
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn collect_script_immutable_bindings(program: &JsProgram, out: &mut NameSet) {
    for statement in &program.program().body {
        match statement {
            Statement::ImportDeclaration(declaration) => {
                if let Some(specifiers) = declaration.specifiers.as_ref() {
                    for specifier in specifiers {
                        let name = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                                specifier.local.name.as_str()
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                                specifier.local.name.as_str()
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                                specifier.local.name.as_str()
                            }
                        };
                        out.insert(Arc::from(name));
                    }
                }
            }
            Statement::VariableDeclaration(declaration)
                if declaration.kind == VariableDeclarationKind::Const =>
            {
                for declarator in &declaration.declarations {
                    extend_name_set_with_oxc_pattern_bindings(out, &declarator.id);
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(Declaration::VariableDeclaration(variable)) =
                    declaration.declaration.as_ref()
                    && variable.kind == VariableDeclarationKind::Const
                {
                    for declarator in &variable.declarations {
                        extend_name_set_with_oxc_pattern_bindings(out, &declarator.id);
                    }
                }
            }
            _ => {}
        }
    }
}

fn find_constant_assignment_in_program(program: &JsProgram) -> Option<SourceSpan> {
    let mut immutables = NameSet::default();
    collect_script_immutable_bindings(program, &mut immutables);

    struct Visitor<'a> {
        immutables: &'a NameSet,
        locals: Vec<NameSet>,
        found: Option<SourceSpan>,
    }

    impl<'a> Visitor<'a> {
        fn is_immutable(&self, name: &str) -> bool {
            if self.locals.iter().rev().any(|scope| scope.contains(name)) {
                return false;
            }
            self.immutables.contains(name)
        }

        fn push_scope(&mut self) {
            self.locals.push(NameSet::default());
        }

        fn pop_scope(&mut self) {
            let _ = self.locals.pop();
        }

        fn declare_pattern(&mut self, pattern: &BindingPattern<'_>) {
            if let Some(scope) = self.locals.last_mut() {
                extend_name_set_with_oxc_pattern_bindings(scope, pattern);
            }
        }
    }

    impl<'a, 'b> Visit<'a> for Visitor<'b> {
        fn visit_function(&mut self, func: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            self.push_scope();
            for param in &func.params.items {
                self.declare_pattern(&param.pattern);
            }
            if let Some(rest) = func.params.rest.as_ref() {
                self.declare_pattern(&rest.rest.argument);
            }
            walk::walk_function(self, func, flags);
            self.pop_scope();
        }

        fn visit_arrow_function_expression(
            &mut self,
            expr: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.push_scope();
            for param in &expr.params.items {
                self.declare_pattern(&param.pattern);
            }
            if let Some(rest) = expr.params.rest.as_ref() {
                self.declare_pattern(&rest.rest.argument);
            }
            walk::walk_arrow_function_expression(self, expr);
            self.pop_scope();
        }

        fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
            // Only track declarations in nested scopes (inside functions/arrows).
            // Top-level const bindings are already in `immutables`.
            if self.locals.len() > 1
                && let Some(scope) = self.locals.last_mut()
            {
                for declarator in &declaration.declarations {
                    extend_name_set_with_oxc_pattern_bindings(scope, &declarator.id);
                }
            }
            walk::walk_variable_declaration(self, declaration);
        }

        fn visit_assignment_expression(
            &mut self,
            expr: &oxc_ast::ast::AssignmentExpression<'a>,
        ) {
            if self.found.is_some() {
                return;
            }
            if has_immutable_assignment_target(&expr.left, self.immutables, &self.locals) {
                self.found = Some(span_range(expr.span));
                return;
            }
            walk::walk_assignment_expression(self, expr);
        }

        fn visit_update_expression(&mut self, expr: &oxc_ast::ast::UpdateExpression<'a>) {
            if self.found.is_none()
                && let Some(name) = expr.argument.get_identifier_name()
                && self.is_immutable(name)
            {
                self.found = Some(span_range(expr.span));
                return;
            }
            walk::walk_update_expression(self, expr);
        }
    }

    let mut visitor = Visitor {
        immutables: &immutables,
        locals: vec![NameSet::default()],
        found: None,
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_class_state_field_error_oxc(program: &JsProgram) -> Option<ClassStateFieldError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FieldOrigin {
        /// Field declaration with no initializer (e.g., `count;`)
        Plain,
        /// Field declaration with a non-rune initializer (e.g., `count = -1;`)
        Initialized,
        State,
    }

    #[derive(Clone, Copy)]
    struct RecordedField {
        _span: Span,
        origin: FieldOrigin,
    }

    #[derive(Default)]
    struct ClassInfo {
        fields: HashMap<Arc<str>, RecordedField>,
        pending_assignments: HashMap<Arc<str>, Span>,
    }

    fn state_field_assignment_name<'a>(
        assignment: &'a oxc_ast::ast::AssignmentExpression<'a>,
    ) -> Option<Arc<str>> {
        let target = assignment.left.as_simple_assignment_target()?;
        let member = target.to_member_expression();
        let OxcExpression::ThisExpression(_) = member.object().get_inner_expression() else {
            return None;
        };
        Some(Arc::<str>::from(member.static_property_name()?))
    }

    fn is_state_creation_expression(expression: &OxcExpression<'_>) -> bool {
        let OxcExpression::CallExpression(call) = expression.get_inner_expression() else {
            return false;
        };
        matches!(
            call.callee
                .get_member_expr()
                .and_then(|member| member.static_property_name()),
            Some("raw" | "by")
        ) && matches!(
            call.callee
                .get_member_expr()
                .map(|member| member.object().get_inner_expression()),
            Some(OxcExpression::Identifier(identifier)) if identifier.name.as_str() == "$state"
                || identifier.name.as_str() == "$derived"
        ) || matches!(
            call.callee.get_inner_expression(),
            OxcExpression::Identifier(identifier)
                if matches!(identifier.name.as_str(), "$state" | "$derived")
        )
    }

    fn record_constructor_statement(
        statement: &Statement<'_>,
        info: &mut ClassInfo,
    ) -> Option<ClassStateFieldError> {
        fn record_constructor_block(
            block: &oxc_ast::ast::BlockStatement<'_>,
            info: &mut ClassInfo,
        ) -> Option<ClassStateFieldError> {
            for statement in &block.body {
                if let Some(error) = record_constructor_statement(statement, info) {
                    return Some(error);
                }
            }
            None
        }

        match statement {
            Statement::BlockStatement(block) => record_constructor_block(block, info),
            Statement::IfStatement(statement) => {
                if let Some(error) = record_constructor_statement(&statement.consequent, info) {
                    return Some(error);
                }
                statement
                    .alternate
                    .as_ref()
                    .and_then(|statement| record_constructor_statement(statement, info))
            }
            Statement::LabeledStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::WithStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::WhileStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::DoWhileStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::ForStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::ForInStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::ForOfStatement(statement) => {
                record_constructor_statement(&statement.body, info)
            }
            Statement::SwitchStatement(statement) => {
                for case in &statement.cases {
                    for statement in &case.consequent {
                        if let Some(error) = record_constructor_statement(statement, info) {
                            return Some(error);
                        }
                    }
                }
                None
            }
            Statement::TryStatement(statement) => {
                if let Some(error) = record_constructor_block(&statement.block, info) {
                    return Some(error);
                }
                if let Some(handler) = statement.handler.as_ref()
                    && let Some(error) = record_constructor_block(&handler.body, info)
                {
                    return Some(error);
                }
                if let Some(finalizer) = statement.finalizer.as_ref()
                    && let Some(error) = record_constructor_block(finalizer, info)
                {
                    return Some(error);
                }
                None
            }
            Statement::ExpressionStatement(statement) => {
                let OxcExpression::AssignmentExpression(assignment) =
                    statement.expression.get_inner_expression()
                else {
                    return None;
                };
                let Some(name) = state_field_assignment_name(assignment) else {
                    // Computed member with non-literal key:
                    // this[variable] = $state(...) is invalid placement.
                    // this[0] or this["name"] are OK (statically resolvable).
                    if is_state_creation_expression(&assignment.right)
                        && let AssignmentTarget::ComputedMemberExpression(member) =
                            &assignment.left
                    {
                        let is_literal_key = matches!(
                            member.expression.get_inner_expression(),
                            OxcExpression::NumericLiteral(_)
                                | OxcExpression::StringLiteral(_)
                        );
                        if !is_literal_key
                            && matches!(
                                member.object.get_inner_expression(),
                                OxcExpression::ThisExpression(_)
                            )
                        {
                            return Some(ClassStateFieldError {
                                kind: DiagnosticKind::StateInvalidPlacement,
                                span: span_range(assignment.right.span()),
                            });
                        }
                    }
                    return None;
                };

                if is_state_creation_expression(&assignment.right) {
                    if let Some(existing) = info.fields.get(name.as_ref()) {
                        match existing.origin {
                            FieldOrigin::Plain => {
                                // A plain field (no initializer) followed by a constructor
                                // assignment with $state/$derived is the "first assignment
                                // to a class field" pattern — this is allowed.
                                // Update the field to be a StateField.
                                info.fields.insert(
                                    name,
                                    RecordedField {
                                        _span: assignment.span,
                                        origin: FieldOrigin::State,
                                    },
                                );
                                return None;
                            }
                            FieldOrigin::Initialized | FieldOrigin::State => {
                                let kind = if existing.origin == FieldOrigin::State {
                                    DiagnosticKind::StateFieldDuplicate { name }
                                } else {
                                    DiagnosticKind::DuplicateClassField { name }
                                };
                                return Some(ClassStateFieldError {
                                    kind,
                                    span: span_range(assignment.span),
                                });
                            }
                        }
                    }

                    if let Some(previous) = info.pending_assignments.remove(name.as_ref()) {
                        return Some(ClassStateFieldError {
                            kind: DiagnosticKind::StateFieldInvalidAssignment,
                            span: span_range(previous),
                        });
                    }

                    info.fields.insert(
                        name,
                        RecordedField {
                            _span: assignment.span,
                            origin: FieldOrigin::State,
                        },
                    );
                    return None;
                }

                info.pending_assignments
                    .entry(name)
                    .or_insert(assignment.span);
                None
            }
            _ => None,
        }
    }

    fn check_class(class: &oxc_ast::ast::Class<'_>) -> Option<ClassStateFieldError> {
        let mut info = ClassInfo::default();
        let mut constructor = None;

        for element in &class.body.body {
            match element {
                ClassElement::PropertyDefinition(property) => {
                    if property.computed || property.r#static {
                        continue;
                    }
                    let Some(name) = property.key.static_name() else {
                        continue;
                    };
                    let key = Arc::<str>::from(name);
                    if info.fields.contains_key(key.as_ref()) {
                        return Some(ClassStateFieldError {
                            kind: DiagnosticKind::DuplicateClassField { name: key },
                            span: span_range(property.span),
                        });
                    }
                    let origin = match &property.value {
                        Some(value) if is_state_creation_expression(value) => {
                            FieldOrigin::State
                        }
                        Some(_) => FieldOrigin::Initialized,
                        None => FieldOrigin::Plain,
                    };
                    info.fields.insert(
                        key,
                        RecordedField {
                            _span: property.span,
                            origin,
                        },
                    );
                }
                ClassElement::MethodDefinition(method)
                    if matches!(method.kind, MethodDefinitionKind::Constructor) =>
                {
                    constructor = Some(method);
                }
                ClassElement::AccessorProperty(property) => {
                    if property.computed || property.r#static {
                        continue;
                    }
                    if let Some(name) = property.key.static_name() {
                        info.fields.insert(
                            Arc::<str>::from(name),
                            RecordedField {
                                _span: property.span,
                                origin: FieldOrigin::Plain,
                            },
                        );
                    }
                }
                _ => {}
            }
        }

        let constructor = constructor?;
        let body = constructor.value.body.as_ref()?;

        for stmt in &body.statements {
            if let Some(error) = record_constructor_statement(stmt, &mut info) {
                return Some(error);
            }
        }

        None
    }

    for statement in &program.program().body {
        let class = match statement {
            Statement::ClassDeclaration(class) => Some(class.as_ref()),
            Statement::ExportNamedDeclaration(export) => {
                export
                    .declaration
                    .as_ref()
                    .and_then(|declaration| match declaration {
                        oxc_ast::ast::Declaration::ClassDeclaration(class) => Some(class.as_ref()),
                        _ => None,
                    })
            }
            _ => None,
        };

        if let Some(class) = class
            && let Some(error) = check_class(class)
        {
            return Some(error);
        }
    }

    None
}

pub(super) fn find_constant_assignment_in_expression(
    expression: &Expression,
    outer_immutables: &NameSet,
) -> Option<SourceSpan> {
    struct Visitor<'a> {
        outer_immutables: &'a NameSet,
        locals: Vec<NameSet>,
        found: Option<SourceSpan>,
    }

    impl<'a> Visitor<'a> {
        fn is_immutable(&self, name: &str) -> bool {
            if self.locals.iter().rev().any(|scope| scope.contains(name)) {
                return false;
            }
            self.outer_immutables.contains(name)
        }

        fn push_scope(&mut self) {
            self.locals.push(NameSet::default());
        }

        fn pop_scope(&mut self) {
            let _ = self.locals.pop();
        }

        fn declare_pattern(&mut self, pattern: &BindingPattern<'_>) {
            if let Some(scope) = self.locals.last_mut() {
                extend_name_set_with_oxc_pattern_bindings(scope, pattern);
            }
        }
    }

    impl<'a, 'b> Visit<'a> for Visitor<'b> {
        fn visit_function(&mut self, func: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            self.push_scope();
            for param in &func.params.items {
                self.declare_pattern(&param.pattern);
            }
            if let Some(rest) = func.params.rest.as_ref() {
                self.declare_pattern(&rest.rest.argument);
            }
            walk::walk_function(self, func, flags);
            self.pop_scope();
        }

        fn visit_arrow_function_expression(
            &mut self,
            expr: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.push_scope();
            for param in &expr.params.items {
                self.declare_pattern(&param.pattern);
            }
            if let Some(rest) = expr.params.rest.as_ref() {
                self.declare_pattern(&rest.rest.argument);
            }
            walk::walk_arrow_function_expression(self, expr);
            self.pop_scope();
        }

        fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
            if let Some(scope) = self.locals.last_mut() {
                for declarator in &declaration.declarations {
                    extend_name_set_with_oxc_pattern_bindings(scope, &declarator.id);
                }
            }
            walk::walk_variable_declaration(self, declaration);
        }

        fn visit_assignment_expression(
            &mut self,
            expr: &oxc_ast::ast::AssignmentExpression<'a>,
        ) {
            if self.found.is_some() {
                return;
            }
            if has_immutable_assignment_target(&expr.left, self.outer_immutables, &self.locals) {
                self.found = Some(span_range(expr.span));
                return;
            }
            walk::walk_assignment_expression(self, expr);
        }

        fn visit_update_expression(&mut self, expr: &oxc_ast::ast::UpdateExpression<'a>) {
            if self.found.is_none()
                && let Some(name) = expr.argument.get_identifier_name()
                && self.is_immutable(name)
            {
                self.found = Some(span_range(expr.span));
                return;
            }
            walk::walk_update_expression(self, expr);
        }
    }

    let mut visitor = Visitor {
        outer_immutables,
        locals: Vec::new(),
        found: None,
    };

    if let Some(declaration) = expression.oxc_variable_declaration() {
        visitor.visit_variable_declaration(declaration);
    } else if let Some(expr) = expression.oxc_expression() {
        visitor.visit_expression(expr);
    }

    visitor.found
}

#[derive(Default)]
struct AliasStack {
    aliases: NameStack,
}

impl AliasStack {
    fn push(&mut self, alias: Arc<str>) {
        self.aliases.push(alias);
    }

    fn pop(&mut self) {
        self.aliases.pop();
    }

    fn contains(&self, alias: &str) -> bool {
        self.aliases.contains(alias)
    }
}

// compile_error_custom is provided by super::compile_error_custom (validation.rs)

fn find_store_invalid_subscription(program: &JsProgram) -> Option<SourceSpan> {
    struct Visitor {
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if self.found.is_some() {
                return;
            }
            let name = it.name.as_str();
            if name.len() <= 1
                || !name.starts_with('$')
                || !name.as_bytes()[1].is_ascii_alphabetic()
            {
                return;
            }
            if is_allowed_rune_name(name) {
                return;
            }
            self.found = Some(span_range(it.span));
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_program(program.program());
    visitor.found
}

fn detect_dollar_binding_error_in_program(
    source: &str,
    program: &JsProgram,
    runes_mode: bool,
) -> Option<CompileError> {
    if let Some(span) = find_dollar_binding_invalid_declaration(program, runes_mode) {
        return Some(SourceSpan::new(span.start, span.start + 1).to_compile_error(
            source,
            DiagnosticKind::DollarBindingInvalid,
        ));
    }
    None
}

fn find_dollar_binding_invalid_declaration(
    program: &JsProgram,
    runes_mode: bool,
) -> Option<SourceSpan> {
    struct Visitor {
        runes_mode: bool,
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
            if self.found.is_some() {
                return;
            }
            for declarator in &declaration.declarations {
                let mut names = Vec::new();
                collect_binding_identifier_spans(&declarator.id, &mut names);
                if let Some((_, span)) = names.into_iter().find(|(name, _)| name.starts_with('$'))
                {
                    self.found = Some(span);
                    return;
                }
            }
            walk::walk_variable_declaration(self, declaration);
        }

        fn visit_import_declaration(
            &mut self,
            declaration: &oxc_ast::ast::ImportDeclaration<'a>,
        ) {
            if self.found.is_some() {
                return;
            }
            if let Some(specifiers) = declaration.specifiers.as_ref() {
                for specifier in specifiers {
                    let (name, span) = match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                            (specifier.local.name.as_str(), specifier.local.span)
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                            (specifier.local.name.as_str(), specifier.local.span)
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                            (specifier.local.name.as_str(), specifier.local.span)
                        }
                    };
                    if name.starts_with('$') {
                        self.found = Some(span_range(span));
                        return;
                    }
                }
            }
        }

        // In legacy mode, don't walk into function bodies - only check top-level declarations.
        fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            if self.runes_mode {
                walk::walk_function(self, it, flags);
            }
        }

        fn visit_arrow_function_expression(
            &mut self,
            it: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            if self.runes_mode {
                walk::walk_arrow_function_expression(self, it);
            }
        }
    }

    let mut visitor = Visitor {
        runes_mode,
        found: None,
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_global_reference_invalid_in_program(
    program: &JsProgram,
) -> Option<NamedSpan> {
    // Module files (.svelte.js) are always runes mode
    find_global_reference_invalid_in_program_with_extra_declared(program, &NameSet::default(), true)
}

fn find_global_reference_invalid_in_program_with_extra_declared(
    program: &JsProgram,
    extra_declared: &NameSet,
    runes_mode: bool,
) -> Option<NamedSpan> {
    let mut declared = collect_declared_names_in_program(program);
    declared.extend(extra_declared.iter().cloned());

    struct Visitor<'a> {
        declared: &'a NameSet,
        runes_mode: bool,
        scopes: Vec<NameSet>,
        found: Option<NamedSpan>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_program(&mut self, it: &Program<'a>) {
            self.scopes.push(NameSet::default());
            walk::walk_program(self, it);
            self.scopes.pop();
        }

        fn visit_block_statement(&mut self, it: &BlockStatement<'a>) {
            self.scopes.push(NameSet::default());
            walk::walk_block_statement(self, it);
            self.scopes.pop();
        }

        fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            let mut scope = NameSet::default();
            if let Some(id) = it.id.as_ref() {
                scope.insert(Arc::from(id.name.as_str()));
            }
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.scopes.push(scope);
            walk::walk_function(self, it, flags);
            self.scopes.pop();
        }

        fn visit_arrow_function_expression(
            &mut self,
            it: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            let mut scope = NameSet::default();
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.scopes.push(scope);
            walk::walk_arrow_function_expression(self, it);
            self.scopes.pop();
        }

        fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
            if let Some(scope) = self.scopes.last_mut() {
                extend_name_set_with_oxc_pattern_bindings(scope, &it.id);
            }
            walk::walk_variable_declarator(self, it);
        }

        fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
            let mut scope = NameSet::default();
            if let Some(param) = it.param.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &param.pattern);
            }
            self.scopes.push(scope);
            walk::walk_catch_clause(self, it);
            self.scopes.pop();
        }

        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if self.found.is_some() {
                return;
            }
            let name = it.name.as_str();
            if !name.starts_with('$') {
                return;
            }
            // Allow rune names
            if is_allowed_rune_name(name) {
                return;
            }
            // Allow `$$props`, `$$restProps`, `$$slots`
            if matches!(name, "$$props" | "$$restProps" | "$$slots") {
                return;
            }
            // Check if the name itself is declared (e.g. `$foo` declared as a variable)
            if self.declared.contains(name)
                || self.scopes.iter().rev().any(|scope| scope.contains(name))
            {
                return;
            }
            // In non-runes mode, only bare `$` and `$$*` are illegal.
            // `$foo` store subscriptions are allowed in legacy mode.
            if !self.runes_mode {
                let is_bare_dollar = name == "$";
                let is_double_dollar = name.starts_with("$$");
                if !is_bare_dollar && !is_double_dollar {
                    return;
                }
            }
            // For store subscriptions `$foo`, check if `foo` is declared
            let alias = &name[1..];
            if !alias.is_empty()
                && (self.declared.contains(alias)
                    || self.scopes.iter().rev().any(|scope| scope.contains(alias)))
            {
                return;
            }
            self.found = Some(NamedSpan::new(Arc::from(name), span_range(it.span)));
        }
    }

    let mut visitor = Visitor {
        declared: &declared,
        runes_mode,
        scopes: Vec::new(),
        found: None,
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn find_invalid_global_reference_in_expression(
    expression: &Expression,
    declared: &NameSet,
    runes_mode: bool,
) -> Option<NamedSpan> {
    let oxc_expr = expression.oxc_expression()?;
    let offset = expression.start;

    struct Visitor<'a> {
        declared: &'a NameSet,
        runes_mode: bool,
        found: Option<NamedSpan>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if self.found.is_some() {
                return;
            }
            let name = it.name.as_str();
            if !name.starts_with('$') {
                return;
            }
            if is_allowed_rune_name(name) {
                return;
            }
            if matches!(name, "$$props" | "$$restProps" | "$$slots") {
                return;
            }
            if self.declared.contains(name) {
                return;
            }
            // In non-runes mode, only bare `$` and `$$*` are illegal.
            // `$foo` store subscriptions are allowed in legacy mode.
            if !self.runes_mode {
                let is_bare_dollar = name == "$";
                let is_double_dollar = name.starts_with("$$");
                if !is_bare_dollar && !is_double_dollar {
                    return;
                }
            }
            let alias = &name[1..];
            if !alias.is_empty() && self.declared.contains(alias) {
                return;
            }
            self.found = Some(NamedSpan::new(Arc::from(name), span_range(it.span)));
        }
    }

    let mut visitor = Visitor {
        declared,
        runes_mode,
        found: None,
    };
    visitor.visit_expression(oxc_expr);
    visitor
        .found
        .map(|named| NamedSpan::new(named.name, named.span.offset(offset)))
}

fn find_invalid_global_reference_in_fragment_with_declared(
    fragment: &Fragment,
    runes_mode: bool,
    declared: &NameSet,
) -> Option<NamedSpan> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        match node {
            Node::ExpressionTag(tag) => {
                find_invalid_global_reference_in_expression(&tag.expression, declared, runes_mode)
            }
            Node::RenderTag(tag) => {
                find_invalid_global_reference_in_expression(&tag.expression, declared, runes_mode)
            }
            Node::HtmlTag(tag) => {
                find_invalid_global_reference_in_expression(&tag.expression, declared, runes_mode)
            }
            Node::ConstTag(tag) => {
                find_invalid_global_reference_in_expression(&tag.declaration, declared, runes_mode)
            }
            _ => None,
        }
    })
}

fn find_state_in_each_header_fragment(fragment: &Fragment) -> Option<SourceSpan> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let Node::EachBlock(block) = node else {
            return None;
        };
        find_state_call_in_expression(&block.expression)
            .or_else(|| find_state_call_in_each_binding_shape(block))
            .or_else(|| {
                block
                    .context
                    .as_ref()
                    .and_then(find_state_call_in_expression)
            })
            .or_else(|| block.key.as_ref().and_then(find_state_call_in_expression))
    })
}

fn find_state_call_in_expression(expression: &Expression) -> Option<SourceSpan> {
    let expression = expression.oxc_expression()?;

    struct Visitor {
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.found.is_none() && oxc_callee_name(&it.callee).as_deref() == Some("$state") {
                self.found = Some(span_range(it.span));
                return;
            }
            walk::walk_call_expression(self, it);
        }
    }

    let mut visitor = Visitor { found: None };
    visitor.visit_expression(expression);
    visitor.found
}

fn is_allowed_rune_name(name: &str) -> bool {
    matches!(
        name,
        "$state"
            | "$state.raw"
            | "$state.snapshot"
            | "$derived"
            | "$derived.by"
            | "$effect"
            | "$effect.active"
            | "$effect.pre"
            | "$effect.tracking"
            | "$effect.root"
            | "$bindable"
            | "$props"
            | "$props.id"
            | "$inspect"
            | "$inspect.trace"
            | "$host"
    )
}

fn find_render_tag_error_in_fragment(fragment: &Fragment) -> Option<RenderTagDiagnostic> {
    fragment.find_map(|entry| match entry.as_node()? {
        Node::RenderTag(tag) => validate_render_tag(tag),
        _ => None,
    })
}

fn find_store_invalid_scoped_subscription(
    fragment: &Fragment,
    scoped_aliases: &mut AliasStack,
) -> Option<SourceSpan> {
    fragment.walk(
        scoped_aliases,
        |entry, scoped_aliases| {
            if let Some(block) = entry.as_if_block() {
                return match find_store_invalid_scoped_subscription_in_expression(
                    &block.test,
                    scoped_aliases,
                ) {
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
                    scoped_store_identifier_span(
                        identifier.name.as_ref(),
                        identifier.start,
                        identifier.end,
                        scoped_aliases,
                    )
                }),
                Node::ExpressionTag(tag) => find_store_invalid_scoped_subscription_in_expression(
                    &tag.expression,
                    scoped_aliases,
                ),
                Node::RenderTag(tag) => find_store_invalid_scoped_subscription_in_expression(
                    &tag.expression,
                    scoped_aliases,
                ),
                Node::HtmlTag(tag) => find_store_invalid_scoped_subscription_in_expression(
                    &tag.expression,
                    scoped_aliases,
                ),
                Node::ConstTag(tag) => find_store_invalid_scoped_subscription_in_expression(
                    &tag.declaration,
                    scoped_aliases,
                ),
                Node::IfBlock(_) => None,
                Node::EachBlock(block) => {
                    if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                        &block.expression,
                        scoped_aliases,
                    ) {
                        return Search::Found(span);
                    }
                    if let Some(key) = block.key.as_ref()
                        && let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                            key,
                            scoped_aliases,
                        )
                    {
                        return Search::Found(span);
                    }

                    if let Some(context) = block.context.as_ref()
                        && let Some(identifier_name) = context.identifier_name()
                    {
                        scoped_aliases.push(identifier_name);
                    }
                    None
                }
                Node::AwaitBlock(block) => find_store_invalid_scoped_subscription_in_expression(
                    &block.expression,
                    scoped_aliases,
                )
                .or_else(|| {
                    block.value.as_ref().and_then(|value| {
                        find_store_invalid_scoped_subscription_in_expression(value, scoped_aliases)
                    })
                })
                .or_else(|| {
                    block.error.as_ref().and_then(|error| {
                        find_store_invalid_scoped_subscription_in_expression(error, scoped_aliases)
                    })
                }),
                Node::SnippetBlock(_) => None,
                Node::KeyBlock(block) => find_store_invalid_scoped_subscription_in_expression(
                    &block.expression,
                    scoped_aliases,
                ),
                _ => node.as_element().and_then(|element| {
                    find_store_invalid_scoped_subscription_in_attributes(
                        element.attributes(),
                        scoped_aliases,
                    )
                }),
            };

            match found {
                Some(span) => Search::Found(span),
                None => Search::Continue,
            }
        },
        |entry, scoped_aliases| {
            if let Some(Node::EachBlock(block)) = entry.as_node()
                && block
                    .context
                    .as_ref()
                    .is_some_and(|context| context.identifier_name().is_some())
            {
                scoped_aliases.pop();
            }
        },
    )
}

fn find_store_invalid_scoped_subscription_in_attributes(
    attributes: &[Attribute],
    scoped_aliases: &AliasStack,
) -> Option<SourceSpan> {
    for attribute in attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueKind::Boolean(_) => {}
                AttributeValueKind::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                                &tag.expression,
                                scoped_aliases,
                            )
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
                    if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                        &tag.expression,
                        scoped_aliases,
                    ) {
                        return Some(span);
                    }
                }
            },
            Attribute::BindDirective(attribute)
            | Attribute::OnDirective(attribute)
            | Attribute::ClassDirective(attribute)
            | Attribute::LetDirective(attribute)
            | Attribute::AnimateDirective(attribute)
            | Attribute::UseDirective(attribute) => {
                if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                    &attribute.expression,
                    scoped_aliases,
                ) {
                    return Some(span);
                }
            }
            Attribute::StyleDirective(attribute) => match &attribute.value {
                AttributeValueKind::Boolean(_) => {}
                AttributeValueKind::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                                &tag.expression,
                                scoped_aliases,
                            )
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
                    if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                        &tag.expression,
                        scoped_aliases,
                    ) {
                        return Some(span);
                    }
                }
            },
            Attribute::TransitionDirective(attribute) => {
                if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                    &attribute.expression,
                    scoped_aliases,
                ) {
                    return Some(span);
                }
            }
            Attribute::AttachTag(tag) => {
                if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                    &tag.expression,
                    scoped_aliases,
                ) {
                    return Some(span);
                }
            }
            Attribute::SpreadAttribute(spread) => {
                if let Some(span) = find_store_invalid_scoped_subscription_in_expression(
                    &spread.expression,
                    scoped_aliases,
                ) {
                    return Some(span);
                }
            }
        }
    }
    None
}

fn find_store_invalid_scoped_subscription_in_expression(
    expression: &Expression,
    scoped_aliases: &AliasStack,
) -> Option<SourceSpan> {
    let expression = expression.oxc_expression()?;

    struct Visitor<'a> {
        aliases: &'a AliasStack,
        js_scopes: Vec<NameSet>,
        found: Option<SourceSpan>,
    }

    impl Visitor<'_> {
        fn is_locally_scoped(&self, alias: &str) -> bool {
            // Check template-level scoped aliases (from each blocks)
            if self.aliases.contains(alias) {
                return true;
            }
            // Check JS-level scoped bindings (from arrow/function params)
            self.js_scopes.iter().rev().any(|scope| scope.contains(alias))
        }
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if self.found.is_some() {
                return;
            }
            let name = it.name.as_str();
            if is_allowed_rune_name(name) {
                return;
            }
            let Some(alias) = name.strip_prefix('$') else {
                return;
            };
            if alias.is_empty() || !self.is_locally_scoped(alias) {
                return;
            }
            self.found = Some(span_range(it.span));
        }

        fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            let mut scope = NameSet::default();
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.js_scopes.push(scope);
            walk::walk_function(self, it, flags);
            self.js_scopes.pop();
        }

        fn visit_arrow_function_expression(
            &mut self,
            it: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            let mut scope = NameSet::default();
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.js_scopes.push(scope);
            walk::walk_arrow_function_expression(self, it);
            self.js_scopes.pop();
        }

        fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
            if let Some(scope) = self.js_scopes.last_mut() {
                extend_name_set_with_oxc_pattern_bindings(scope, &it.id);
            }
            walk::walk_variable_declarator(self, it);
        }
    }

    let mut visitor = Visitor {
        aliases: scoped_aliases,
        js_scopes: Vec::new(),
        found: None,
    };
    visitor.visit_expression(expression);
    visitor.found
}

fn find_state_call_in_each_binding_shape(block: &EachBlock) -> Option<SourceSpan> {
    if !block.has_as_clause {
        return None;
    }
    let context = block.context.as_ref()?;
    let key = block.key.as_ref()?;
    if context.identifier_name().as_deref() != Some("$state") {
        return None;
    }
    Some(SourceSpan::new(context.start, key.end))
}

fn find_store_invalid_scoped_subscription_in_program(
    program: &JsProgram,
) -> Option<SourceSpan> {
    let declared = collect_declared_names_in_program(program);

    struct Visitor<'a> {
        declared: &'a NameSet,
        scopes: Vec<NameSet>,
        found: Option<SourceSpan>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_program(&mut self, it: &Program<'a>) {
            self.scopes.push(NameSet::default());
            walk::walk_program(self, it);
            self.scopes.pop();
        }

        fn visit_block_statement(&mut self, it: &BlockStatement<'a>) {
            self.scopes.push(NameSet::default());
            walk::walk_block_statement(self, it);
            self.scopes.pop();
        }

        fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            let mut scope = NameSet::default();
            if let Some(id) = it.id.as_ref() {
                scope.insert(Arc::from(id.name.as_str()));
            }
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.scopes.push(scope);
            walk::walk_function(self, it, flags);
            self.scopes.pop();
        }

        fn visit_arrow_function_expression(
            &mut self,
            it: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            let mut scope = NameSet::default();
            for item in &it.params.items {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &item.pattern);
            }
            if let Some(rest) = it.params.rest.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &rest.rest.argument);
            }
            self.scopes.push(scope);
            walk::walk_arrow_function_expression(self, it);
            self.scopes.pop();
        }

        fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
            if let Some(scope) = self.scopes.last_mut() {
                extend_name_set_with_oxc_pattern_bindings(scope, &it.id);
            }
            walk::walk_variable_declarator(self, it);
        }

        fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
            let mut scope = NameSet::default();
            if let Some(param) = it.param.as_ref() {
                extend_name_set_with_oxc_pattern_bindings(&mut scope, &param.pattern);
            }
            self.scopes.push(scope);
            walk::walk_catch_clause(self, it);
            self.scopes.pop();
        }

        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if self.found.is_some() {
                return;
            }
            let name = it.name.as_str();
            if is_allowed_rune_name(name) {
                return;
            }
            let Some(alias) = name.strip_prefix('$') else {
                return;
            };
            if alias.is_empty()
                || !self.declared.contains(alias)
                || self.scopes.iter().rev().any(|scope| scope.contains(name))
            {
                return;
            }
            // Only flag $store references where the base name is shadowed
            // by a local binding in a nested scope (function param, let/const, etc.),
            // skipping the outermost program scope (index 0).
            if !self.scopes.iter().skip(1).any(|scope| scope.contains(alias)) {
                return;
            }
            self.found = Some(span_range(it.span));
        }
    }

    let mut visitor = Visitor {
        declared: &declared,
        scopes: Vec::new(),
        found: None,
    };
    visitor.visit_program(program.program());
    visitor.found
}

/// Collect names assigned in a reactive label body (e.g., `$: z = expr` → `z`).
fn collect_reactive_label_names(names: &mut NameSet, body: &Statement<'_>) {
    match body {
        Statement::ExpressionStatement(stmt) => {
            if let OxcExpression::AssignmentExpression(assign) = stmt.expression.get_inner_expression() {
                collect_names_from_assignment_target(names, &assign.left);
            }
        }
        Statement::BlockStatement(block) => {
            for stmt in &block.body {
                collect_reactive_label_names(names, stmt);
            }
        }
        _ => {}
    }
}

/// Collect `$name` identifiers referenced inside `$:` labeled statements.
/// In legacy mode, `$: $foo;` creates an auto-subscription to store `foo`,
/// making `$foo` a valid declared binding.
fn collect_dollar_label_store_subscriptions(
    names: &mut NameSet,
    program: &oxc_ast::ast::Program<'_>,
) {
    for stmt in &program.body {
        if let Statement::LabeledStatement(labeled) = stmt
            && labeled.label.name.as_str() == "$"
        {
            collect_dollar_refs_from_statement(names, &labeled.body);
        }
    }
}

fn collect_dollar_refs_from_statement(names: &mut NameSet, stmt: &Statement<'_>) {
    struct DollarRefVisitor<'b> {
        names: &'b mut NameSet,
    }
    impl<'a> Visit<'a> for DollarRefVisitor<'_> {
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            let name = it.name.as_str();
            if name.starts_with('$') && name.len() > 1 && !name.starts_with("$$") {
                self.names.insert(Arc::from(name));
            }
        }
    }
    let mut visitor = DollarRefVisitor { names };
    visitor.visit_statement(stmt);
}

fn collect_names_from_assignment_target(names: &mut NameSet, target: &AssignmentTarget<'_>) {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            names.insert(Arc::from(id.name.as_str()));
        }
        AssignmentTarget::ObjectAssignmentTarget(obj) => {
            for prop in &obj.properties {
                match prop {
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                        names.insert(Arc::from(id.binding.name.as_str()));
                    }
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                        collect_names_from_assignment_target_maybe_default(names, &prop.binding);
                    }
                }
            }
            if let Some(rest) = &obj.rest {
                collect_names_from_assignment_target(names, &rest.target);
            }
        }
        AssignmentTarget::ArrayAssignmentTarget(arr) => {
            for el in &arr.elements {
                if let Some(maybe_default) = el.as_ref() {
                    collect_names_from_assignment_target_maybe_default(names, maybe_default);
                }
            }
            if let Some(rest) = &arr.rest {
                collect_names_from_assignment_target(names, &rest.target);
            }
        }
        _ => {}
    }
}

fn collect_names_from_assignment_target_maybe_default(
    names: &mut NameSet,
    target: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
) {
    match target {
        oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
            collect_names_from_assignment_target(names, &with_default.binding);
        }
        _ => {
            // The other variants are assignment targets directly
            if let Some(target) = target.as_assignment_target() {
                collect_names_from_assignment_target(names, target);
            }
        }
    }
}

fn collect_declared_names_in_program(program: &JsProgram) -> NameSet {
    let mut names = NameSet::default();
    collect_declared_names_in_statements(&mut names, &program.program().body);
    names
}

fn collect_declared_names_in_statements(names: &mut NameSet, statements: &[Statement<'_>]) {
    for statement in statements {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    extend_name_set_with_oxc_pattern_bindings(names, &declarator.id);
                }
            }
            Statement::FunctionDeclaration(declaration) => {
                if let Some(id) = declaration.id.as_ref() {
                    names.insert(Arc::from(id.name.as_str()));
                }
            }
            Statement::ClassDeclaration(declaration) => {
                if let Some(id) = declaration.id.as_ref() {
                    names.insert(Arc::from(id.name.as_str()));
                }
            }
            Statement::ImportDeclaration(declaration) => {
                if let Some(specifiers) = declaration.specifiers.as_ref() {
                    for specifier in specifiers {
                        if let Some(name) = import_specifier_local_name(specifier) {
                            names.insert(Arc::from(name));
                        }
                    }
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(declaration) = declaration.declaration.as_ref() {
                    match declaration {
                        Declaration::VariableDeclaration(declaration) => {
                            for declarator in &declaration.declarations {
                                extend_name_set_with_oxc_pattern_bindings(names, &declarator.id);
                            }
                        }
                        Declaration::FunctionDeclaration(declaration) => {
                            if let Some(id) = declaration.id.as_ref() {
                                names.insert(Arc::from(id.name.as_str()));
                            }
                        }
                        Declaration::ClassDeclaration(declaration) => {
                            if let Some(id) = declaration.id.as_ref() {
                                names.insert(Arc::from(id.name.as_str()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Reactive declarations: `$: z = expr` declares `z`
            Statement::LabeledStatement(labeled) if labeled.label.name.as_str() == "$" => {
                collect_reactive_label_names(names, &labeled.body);
            }
            _ => {}
        }
    }
}

fn collect_binding_identifier_spans(
    pattern: &BindingPattern<'_>,
    names: &mut Vec<(String, SourceSpan)>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            names.push((identifier.name.to_string(), span_range(identifier.span)));
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_binding_identifier_spans(&pattern.left, names);
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_binding_identifier_spans(&property.value, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_binding_identifier_spans(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_binding_identifier_spans(element, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_binding_identifier_spans(&rest.argument, names);
            }
        }
    }
}

fn scoped_store_identifier_span(
    identifier: &str,
    start: usize,
    end: usize,
    scoped_aliases: &AliasStack,
) -> Option<SourceSpan> {
    let alias = identifier.strip_prefix('$')?;
    if alias.is_empty() {
        return None;
    }
    if scoped_aliases.contains(alias) {
        return Some(SourceSpan::new(start, end));
    }
    None
}

struct RenderTagDiagnostic {
    kind: DiagnosticKind,
    span: SourceSpan,
}

fn validate_render_tag(tag: &crate::ast::modern::RenderTag) -> Option<RenderTagDiagnostic> {
    let (call, optional) = match tag.expression.oxc_expression() {
        Some(OxcExpression::CallExpression(call)) => (call, false),
        Some(OxcExpression::ChainExpression(chain)) => {
            let ChainElement::CallExpression(call) = &chain.expression else {
                return Some(RenderTagDiagnostic {
                    kind: DiagnosticKind::RenderTagInvalidExpression,
                    span: SourceSpan::new(tag.start, tag.end),
                });
            };
            (call, true)
        }
        _ => {
            return Some(RenderTagDiagnostic {
                kind: DiagnosticKind::RenderTagInvalidExpression,
                span: SourceSpan::new(tag.start, tag.end),
            });
        }
    };

    if call.arguments.iter().any(|argument| argument.is_spread()) {
        return Some(RenderTagDiagnostic {
            kind: DiagnosticKind::RenderTagInvalidSpreadArgument,
            span: span_range(call.span),
        });
    }

    if !optional
        && let OxcExpression::StaticMemberExpression(member) = call.callee.get_inner_expression()
        && matches!(member.property.name.as_str(), "apply" | "bind" | "call")
    {
        return Some(RenderTagDiagnostic {
            kind: DiagnosticKind::RenderTagInvalidCallExpression,
            span: SourceSpan::new(tag.start, tag.end),
        });
    }

    None
}
