use super::*;
use crate::ast::modern::{
    Attribute, AttributeValue, AttributeValueList, EachBlock, EstreeNode, EstreeValue, Expression,
    Fragment, Node, Search,
};
use crate::estree::{
    PathStep, class_key_name, path_has_function_scope, raw_base_identifier_name, raw_callee_name,
    raw_identifier_name, raw_literal_string, raw_member_property_name, this_member_name,
    walk_estree_node_with_path,
};
use crate::names::NameStack;
use crate::{SourceId, SourceText};
use std::collections::HashMap;

pub(super) fn detect_runes_mode_invalid_import(source: &str, root: &Root) -> Option<CompileError> {
    let script = root.instance.as_ref()?;
    if let Some((start, end)) = find_before_update_import_in_program(&script.content) {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::RunesModeInvalidImportBeforeUpdate,
            start,
            end,
        ));
    }
    None
}

pub(super) fn detect_legacy_export_invalid(source: &str, root: &Root) -> Option<CompileError> {
    let script = root.instance.as_ref()?;
    if let Some((start, end)) = find_legacy_export_let_in_program(&script.content) {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::LegacyExportInvalid,
            start,
            end,
        ));
    }
    None
}

pub(super) fn detect_dollar_prefix_invalid(source: &str, root: &Root) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_dollar_prefix_invalid_in_program(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::DollarPrefixInvalid,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_store_invalid_scoped_subscription(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    if let Some(instance) = root.instance.as_ref()
        && let Some((start, end)) =
            find_store_invalid_scoped_subscription_in_program(&instance.content)
    {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::StoreInvalidScopedSubscription,
            start,
            end,
        ));
    }

    let (start, end) =
        find_store_invalid_scoped_subscription(&root.fragment, &mut AliasStack::default())?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::StoreInvalidScopedSubscription,
        start,
        end,
    ))
}

pub(super) fn detect_store_invalid_subscription_component(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let module = root.module.as_ref()?;
    let (start, end) = find_store_invalid_subscription(&module.content)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::StoreInvalidSubscription,
        start,
        end,
    ))
}

pub(super) fn detect_dollar_binding_error_component(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Option<CompileError> {
    let runes_mode = is_runes_mode(options, root);
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some(error) =
            detect_dollar_binding_error_in_program(source, &script.content, runes_mode)
        {
            return Some(error);
        }
    }
    None
}

pub(super) fn detect_global_reference_invalid_markup(
    source: &str,
    root: &Root,
    runes_mode: bool,
) -> Option<CompileError> {
    let instance = root.instance.as_ref().map(|script| &script.content);
    let (ident, start, end) =
        find_invalid_global_reference_in_fragment(&root.fragment, runes_mode, instance)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::GlobalReferenceInvalid { ident },
        start,
        end,
    ))
}

pub(super) fn detect_state_in_each_header_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let (start, end) = find_state_in_each_header_fragment(&root.fragment)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::StateInvalidPlacement,
        start,
        end,
    ))
}

pub(super) fn detect_rune_missing_parentheses(source: &str, root: &Root) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        let Some((start, end)) = find_rune_missing_parentheses_in_program(&script.content) else {
            continue;
        };
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::RuneMissingParentheses,
            start,
            end,
        ));
    }
    None
}

pub(super) fn detect_each_item_invalid_assignment(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let mut scope = ScopeStack::default();
    let (start, end) = find_each_item_invalid_assignment(&root.fragment, &mut scope)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::EachItemInvalidAssignment,
        start,
        end,
    ))
}

fn find_each_item_invalid_assignment(
    fragment: &Fragment,
    scope: &mut ScopeStack,
) -> Option<(usize, usize)> {
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
) -> Option<(usize, usize)> {
    for attribute in attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) =
                                assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueList::ExpressionTag(tag) => {
                    if let Some(span) =
                        assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                    {
                        return Some(span);
                    }
                }
            },
            Attribute::BindDirective(attribute) => {
                if assignment_target_contains_each_binding(&attribute.expression.0, scope) {
                    return Some((attribute.start, attribute.end));
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
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value
                            && let Some(span) =
                                assignment_to_each_scoped_name_in_expression(&tag.expression, scope)
                        {
                            return Some(span);
                        }
                    }
                }
                AttributeValueList::ExpressionTag(tag) => {
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
                if !assignment_target_contains_each_binding(left, scope) {
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
                if !assignment_target_contains_each_binding(argument, scope) {
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

fn assignment_target_contains_each_binding(target: &EstreeNode, scope: &ScopeStack) -> bool {
    if estree_node_type(target) == Some("Identifier")
        && let Some(name) = raw_identifier_name(target)
        && scope.contains(name.as_ref())
    {
        return true;
    }

    let mut names = NameSet::default();
    extend_name_set_with_pattern_bindings(&mut names, target);
    names.iter().any(|name| scope.contains(name.as_ref()))
}

pub(super) fn detect_render_tag_errors_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let error = find_render_tag_error_in_fragment(&root.fragment)?;
    Some(compile_error_with_range(
        source,
        error.kind,
        error.start,
        error.end,
    ))
}

pub(super) fn detect_class_state_field_error_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        let Some(error) = detect_class_state_field_error(source, &script.content) else {
            continue;
        };
        return Some(error);
    }
    None
}

pub(super) fn detect_rune_argument_count_errors_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        let Some((kind, start, end)) = find_invalid_rune_argument_count(&script.content) else {
            continue;
        };
        return Some(compile_error_with_range(source, kind, start, end));
    }
    None
}

pub(super) fn detect_rune_invalid_spread_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        let Some((name, start, end)) = find_rune_invalid_spread(&script.content) else {
            continue;
        };
        return Some(compile_error_custom_runes(
            source,
            "rune_invalid_spread",
            format!("`{name}` cannot be called with a spread argument"),
            start,
            end,
        ));
    }

    None
}

pub(super) fn detect_props_duplicate_from_root(source: &str, root: &Root) -> Option<CompileError> {
    let mut count = 0usize;
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_first_call_span_by_name(&script.content, "$props") {
            count += count_calls_by_name(&script.content, "$props");
            if count > 1 {
                return Some(compile_error_with_range(
                    source,
                    CompilerDiagnosticKind::PropsDuplicate,
                    start,
                    end,
                ));
            }
        }
    }
    None
}

pub(super) fn detect_props_illegal_name_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_props_illegal_name(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::PropsIllegalName,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_bindable_invalid_arguments_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) =
            find_invalid_call_arg_count(&script.content, "$bindable", |c| c <= 1)
        {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::RuneInvalidArgumentsLengthBindable,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_props_invalid_arguments_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) =
            find_invalid_call_arg_count(&script.content, "$props", |c| c == 0)
        {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::RuneInvalidArgumentsProps,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_props_invalid_placement_component(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let instance = root.instance.as_ref()?;
    let (start, end) = find_props_invalid_placement_component(&instance.content)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::PropsInvalidPlacement,
        start,
        end,
    ))
}

pub(super) fn detect_bindable_invalid_location_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_bindable_invalid_location(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::BindableInvalidLocation,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_derived_invalid_placement_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_invalid_initializer_placement(&script.content, "$derived")
        {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidPlacementDerived,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_effect_invalid_placement_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_effect_invalid_placement(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::EffectInvalidPlacement,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_host_invalid_placement_component(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    if root
        .options
        .as_ref()
        .and_then(|options| options.custom_element.as_ref())
        .is_some()
    {
        return None;
    }
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_first_call_span_by_name(&script.content, "$host") {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::HostInvalidPlacement,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_state_invalid_placement_general_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_invalid_initializer_placement(&script.content, "$state") {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidPlacement,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_state_invalid_placement_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_static_state_call(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidPlacement,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_invalid_rune_name_component(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some(error) = detect_invalid_name(source, &script.content) {
            return Some(error);
        }
    }
    None
}

pub(super) fn detect_invalid_name(source: &str, program: &EstreeNode) -> Option<CompileError> {
    let (name, start, end) = find_invalid_rune_name(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::RuneInvalidName { name },
        start,
        end,
    ))
}

pub(super) fn detect_renamed_effect_active(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_renamed_effect_active(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::RuneRenamedEffectActive,
        start,
        end,
    ))
}

pub(super) fn detect_store_invalid_subscription_module(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_store_invalid_subscription(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::StoreInvalidSubscriptionModule,
        start,
        end,
    ))
}

pub(super) fn detect_dollar_binding_error(
    source: &str,
    program: &EstreeNode,
    runes_mode: bool,
) -> Option<CompileError> {
    detect_dollar_binding_error_in_program(source, program, runes_mode)
}

pub(super) fn detect_constant_assignment(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_constant_assignment(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::ConstantAssignment,
        start,
        end,
    ))
}

pub(super) fn detect_bindable_invalid_location(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_bindable_invalid_location(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::BindableInvalidLocation,
        start,
        end,
    ))
}

pub(super) fn detect_rune_argument_count(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (kind, start, end) = find_invalid_rune_argument_count(program)?;
    Some(compile_error_with_range(source, kind, start, end))
}

pub(super) fn detect_state_invalid_placement(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    detect_initializer_placement(
        source,
        program,
        "$state",
        CompilerDiagnosticKind::StateInvalidPlacement,
    )
}

pub(super) fn detect_derived_invalid_placement(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    detect_initializer_placement(
        source,
        program,
        "$derived",
        CompilerDiagnosticKind::StateInvalidPlacementDerived,
    )
}

pub(super) fn detect_effect_invalid_placement(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_effect_invalid_placement(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::EffectInvalidPlacement,
        start,
        end,
    ))
}

pub(super) fn detect_host_invalid_placement(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_first_call_span_by_name(program, "$host")?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::HostInvalidPlacement,
        start,
        end,
    ))
}

pub(super) fn detect_class_state_field_error(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let error = find_class_state_field_error(program)?;
    Some(error.compile(source))
}

pub(super) fn detect_props_invalid_placement_module(
    source: &str,
    program: &EstreeNode,
) -> Option<CompileError> {
    let (start, end) = find_first_call_span_by_name(program, "$props")?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::PropsInvalidPlacement,
        start,
        end,
    ))
}

fn find_renamed_effect_active(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node(program, &mut |node| {
        if found.is_some() || estree_node_type(node) != Some("MemberExpression") {
            return;
        }
        let Some(object) = estree_node_field_object(node, RawField::Object) else {
            return;
        };
        let Some(property) = estree_node_field_object(node, RawField::Property) else {
            return;
        };
        let Some(object_name) = raw_identifier_name(object) else {
            return;
        };
        let Some(property_name) = raw_identifier_name(property) else {
            return;
        };
        if object_name.as_ref() == "$effect" && property_name.as_ref() == "active" {
            found = estree_node_span(node);
        }
    });
    found
}

fn detect_initializer_placement(
    source: &str,
    program: &EstreeNode,
    call_name: &str,
    kind: CompilerDiagnosticKind,
) -> Option<CompileError> {
    let (start, end) = find_invalid_initializer_placement(program, call_name)?;
    Some(compile_error_with_range(source, kind, start, end))
}

pub(super) fn detect_constant_assignment_component(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        if let Some((start, end)) = find_constant_assignment(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::ConstantAssignment,
                start,
                end,
            ));
        }
    }
    None
}

fn find_before_update_import_in_program(program: &EstreeNode) -> Option<(usize, usize)> {
    let body = estree_node_field_array(program, RawField::Body)?;
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ImportDeclaration") {
            continue;
        }
        let Some(source_literal) = estree_node_field_object(statement, RawField::Source) else {
            continue;
        };
        let Some("svelte") = raw_literal_string(source_literal).as_deref() else {
            continue;
        };
        let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers) else {
            continue;
        };
        for specifier in specifiers {
            let EstreeValue::Object(specifier) = specifier else {
                continue;
            };
            if estree_node_type(specifier) != Some("ImportSpecifier") {
                continue;
            }
            let Some(imported) = estree_node_field_object(specifier, RawField::Imported) else {
                continue;
            };
            let Some("beforeUpdate") = raw_identifier_name(imported).as_deref() else {
                continue;
            };
            if let Some(local) = estree_node_field_object(specifier, RawField::Local)
                && let Some(span) = estree_node_span(local)
            {
                return Some(span);
            }
            if let Some(span) = estree_node_span(imported).or_else(|| estree_node_span(specifier)) {
                return Some(span);
            }
        }
    }
    None
}

fn find_legacy_export_let_in_program(program: &EstreeNode) -> Option<(usize, usize)> {
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
        let Some(declaration) = estree_node_field_object(statement, RawField::Declaration) else {
            continue;
        };
        if estree_node_type(declaration) != Some("VariableDeclaration")
            || estree_node_field_str(declaration, RawField::Kind) != Some("let")
        {
            continue;
        }
        if let Some((start, statement_end)) = estree_node_span(statement) {
            let end = (start + "export let".len()).min(statement_end);
            return Some((start, end));
        }
    }
    None
}

fn find_dollar_prefix_invalid_in_program(program: &EstreeNode) -> Option<(usize, usize)> {
    let body = estree_node_field_array(program, RawField::Body)?;
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        match estree_node_type(statement) {
            Some("VariableDeclaration") => {
                if let Some(span) = find_invalid_dollar_in_variable_declaration(statement) {
                    return Some(span);
                }
            }
            Some("ImportDeclaration") => {
                let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers)
                else {
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
                    if is_dollar_prefixed_invalid_identifier(name.as_ref())
                        && let Some((start, _)) = estree_node_span(local)
                    {
                        return Some((start, start + 1));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn find_invalid_dollar_in_variable_declaration(declaration: &EstreeNode) -> Option<(usize, usize)> {
    let declarations = estree_node_field_array(declaration, RawField::Declarations)?;
    for declarator in declarations {
        let EstreeValue::Object(declarator) = declarator else {
            continue;
        };
        let id = estree_node_field_object(declarator, RawField::Id)?;
        let Some(name) = raw_identifier_name(id) else {
            continue;
        };
        if is_dollar_prefixed_invalid_identifier(name.as_ref())
            && let Some((start, _)) = estree_node_span(id)
        {
            return Some((start, start + 1));
        }
    }
    None
}

fn estree_node_span(node: &EstreeNode) -> Option<(usize, usize)> {
    Some((
        estree_value_to_usize(estree_node_field(node, RawField::Start))?,
        estree_value_to_usize(estree_node_field(node, RawField::End))?,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    Prop,
    AssignedProp,
    Get,
    Set,
    Method,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PropertyFieldKind {
    Prop,
    AssignedProp,
}

#[derive(Debug)]
struct ClassStateFieldError {
    kind: CompilerDiagnosticKind,
    start: usize,
    end: usize,
}

impl ClassStateFieldError {
    fn compile(self, source: &str) -> CompileError {
        compile_error_with_range(source, self.kind, self.start, self.end)
    }
}

#[derive(Clone, Copy)]
struct StateField<'a> {
    node: &'a EstreeNode,
}

#[derive(Default)]
struct FieldHistory {
    property: Option<PropertyFieldKind>,
    has_get: bool,
    has_set: bool,
    has_method: bool,
}

impl FieldHistory {
    fn record_property(&mut self, kind: PropertyFieldKind) -> bool {
        if self.property.is_some() || self.has_get || self.has_set || self.has_method {
            return false;
        }
        self.property = Some(kind);
        true
    }

    fn record_method_kind(&mut self, kind: FieldKind) -> bool {
        if self.property.is_some() {
            return false;
        }
        match kind {
            FieldKind::Get => {
                if self.has_get || self.has_method {
                    return false;
                }
                self.has_get = true;
                true
            }
            FieldKind::Set => {
                if self.has_set || self.has_method {
                    return false;
                }
                self.has_set = true;
                true
            }
            FieldKind::Method => {
                if self.has_get || self.has_set || self.has_method {
                    return false;
                }
                self.has_method = true;
                true
            }
            FieldKind::Prop | FieldKind::AssignedProp => false,
        }
    }

    fn allows_state_field(&self) -> bool {
        self.property == Some(PropertyFieldKind::Prop)
    }
}

struct ClassFieldIndex<'a> {
    declared_kinds: HashMap<Arc<str>, FieldHistory>,
    state_fields: HashMap<Arc<str>, StateField<'a>>,
}

impl<'a> ClassFieldIndex<'a> {
    fn new() -> Self {
        Self {
            declared_kinds: HashMap::new(),
            state_fields: HashMap::new(),
        }
    }

    fn record_property_kind(
        &mut self,
        node: &EstreeNode,
        name: Arc<str>,
        kind: FieldKind,
    ) -> Option<ClassStateFieldError> {
        let property_kind = match kind {
            FieldKind::Prop => PropertyFieldKind::Prop,
            FieldKind::AssignedProp => PropertyFieldKind::AssignedProp,
            FieldKind::Get | FieldKind::Set | FieldKind::Method => {
                return Some(duplicate_class_field_error(node, name));
            }
        };
        let mut history = FieldHistory::default();
        if !history.record_property(property_kind)
            || self.declared_kinds.insert(name.clone(), history).is_some()
        {
            return Some(duplicate_class_field_error(node, name));
        }
        None
    }

    fn record_method_kind(
        &mut self,
        node: &EstreeNode,
        name: Arc<str>,
        kind: FieldKind,
    ) -> Option<ClassStateFieldError> {
        match self.declared_kinds.get_mut(&name) {
            None => {
                let mut history = FieldHistory::default();
                if !history.record_method_kind(kind) {
                    return Some(duplicate_class_field_error(node, name));
                }
                self.declared_kinds.insert(name, history);
                None
            }
            Some(existing) => existing
                .record_method_kind(kind)
                .then_some(())
                .map_or_else(|| Some(duplicate_class_field_error(node, name)), |_| None),
        }
    }

    fn record_state_field(
        &mut self,
        node: &'a EstreeNode,
        name: &Arc<str>,
    ) -> Option<ClassStateFieldError> {
        let value = estree_node_value(node)?;
        if !is_state_creation_call(value) {
            return None;
        }
        if self.state_fields.contains_key(name) {
            return Some(state_field_duplicate_error(node, name.clone()));
        }
        if let Some(kinds) = self.declared_kinds.get(name)
            && !kinds.allows_state_field()
        {
            return Some(duplicate_class_field_error(node, name.clone()));
        }
        self.state_fields.insert(name.clone(), StateField { node });
        None
    }

    fn state_field(&self, name: &str) -> Option<&StateField<'a>> {
        self.state_fields.get(name)
    }
}

fn is_dollar_prefixed_invalid_identifier(name: &str) -> bool {
    if name.len() <= 1 || !name.starts_with('$') {
        return false;
    }
    let second = name.as_bytes()[1];
    second == b'_' || second.is_ascii_alphabetic()
}

fn find_rune_invalid_spread(program: &EstreeNode) -> Option<(Arc<str>, usize, usize)> {
    let mut found = None::<(Arc<str>, usize, usize)>;
    walk_estree_node(program, &mut |node| {
        if found.is_some() || estree_node_type(node) != Some("CallExpression") {
            return;
        }

        let Some(callee) = estree_node_field_object(node, RawField::Callee) else {
            return;
        };
        let Some(name) = raw_callee_name(callee) else {
            return;
        };
        if !matches!(
            name.as_ref(),
            "$derived" | "$derived.by" | "$state" | "$state.raw"
        ) {
            return;
        }

        let Some(arguments) = estree_node_field_array(node, RawField::Arguments) else {
            return;
        };
        let has_spread = arguments.iter().any(|argument| {
            matches!(
                argument,
                EstreeValue::Object(argument) if estree_node_type(argument) == Some("SpreadElement")
            )
        });
        if !has_spread {
            return;
        }

        let Some((_, end)) = estree_node_span(node) else {
            return;
        };
        let start = estree_node_span(callee)
            .map(|(start, _)| start)
            .unwrap_or(end);
        found = Some((name, start, end));
    });
    found
}

fn find_first_call_span_by_name(program: &EstreeNode, name: &str) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, _path| {
        if found.is_some() {
            return;
        }
        let Some((call_name, start, end, _)) = call_node_info(node) else {
            return;
        };
        if call_name.as_ref() == name {
            found = Some((start, end));
        }
    });
    found
}

fn count_calls_by_name(program: &EstreeNode, name: &str) -> usize {
    let mut count = 0usize;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, _path| {
        let Some((call_name, _, _, _)) = call_node_info(node) else {
            return;
        };
        if call_name.as_ref() == name {
            count += 1;
        }
    });
    count
}

fn find_props_illegal_name(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut props_rest_bindings = NameSet::default();
    let mut found = None::<(usize, usize)>;

    walk_estree_node(program, &mut |node| {
        if found.is_some() || estree_node_type(node) != Some("VariableDeclarator") {
            return;
        }

        let Some(init) = estree_node_field_object(node, RawField::Init) else {
            return;
        };
        if call_name_for_node(init).as_deref() != Some("$props") {
            return;
        }

        let Some(id) = estree_node_field_object(node, RawField::Id) else {
            return;
        };
        match estree_node_type(id) {
            Some("Identifier") => {
                if let Some(name) = raw_identifier_name(id) {
                    props_rest_bindings.insert(name);
                }
            }
            Some("ObjectPattern") => {
                let Some(properties) = estree_node_field_array(id, RawField::Properties) else {
                    return;
                };
                for property in properties {
                    let EstreeValue::Object(property) = property else {
                        continue;
                    };
                    match estree_node_type(property) {
                        Some("RestElement") => {
                            let Some(argument) =
                                estree_node_field_object(property, RawField::Argument)
                            else {
                                continue;
                            };
                            if estree_node_type(argument) != Some("Identifier") {
                                continue;
                            }
                            if let Some(name) = raw_identifier_name(argument) {
                                props_rest_bindings.insert(name);
                            }
                        }
                        Some("Property") => {
                            if estree_node_field_bool_named(property, "computed").unwrap_or(false) {
                                continue;
                            }
                            let Some(key) = estree_node_field_object(property, RawField::Key)
                            else {
                                continue;
                            };
                            if estree_node_type(key) != Some("Identifier") {
                                continue;
                            }
                            let Some(name) = raw_identifier_name(key) else {
                                continue;
                            };
                            if !name.starts_with("$$") {
                                continue;
                            }
                            found = estree_node_span(key).or_else(|| estree_node_span(property));
                            return;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    });

    if found.is_some() {
        return found;
    }

    walk_estree_node(program, &mut |node| {
        if found.is_some() || estree_node_type(node) != Some("MemberExpression") {
            return;
        }
        if estree_node_field_bool(node, RawField::Computed).unwrap_or(false) {
            return;
        }

        let Some(object) = estree_node_field_object(node, RawField::Object) else {
            return;
        };
        if estree_node_type(object) != Some("Identifier") {
            return;
        }
        let Some(object_name) = raw_identifier_name(object) else {
            return;
        };
        if !props_rest_bindings.contains(object_name.as_ref()) {
            return;
        }

        let Some(property) = estree_node_field_object(node, RawField::Property) else {
            return;
        };
        if estree_node_type(property) != Some("Identifier") {
            return;
        }
        let Some(property_name) = raw_identifier_name(property) else {
            return;
        };
        if !property_name.starts_with("$$") {
            return;
        }

        found = estree_node_span(property).or_else(|| estree_node_span(node));
    });

    found
}

fn find_invalid_call_arg_count(
    program: &EstreeNode,
    name: &str,
    is_valid: impl Fn(usize) -> bool,
) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, _path| {
        if found.is_some() {
            return;
        }
        let Some((call_name, start, end, arg_count)) = call_node_info(node) else {
            return;
        };
        if call_name.as_ref() == name && !is_valid(arg_count) {
            found = Some((start, end));
        }
    });
    found
}

fn find_invalid_rune_argument_count(
    program: &EstreeNode,
) -> Option<(CompilerDiagnosticKind, usize, usize)> {
    let mut found = None::<(CompilerDiagnosticKind, usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, _path| {
        if found.is_some() {
            return;
        }
        let Some((name, start, end, arg_count)) = call_node_info(node) else {
            return;
        };
        let Some(kind) = invalid_rune_argument_kind(name.as_ref(), arg_count) else {
            return;
        };
        found = Some((kind, start, end));
    });
    found
}

fn invalid_rune_argument_kind(name: &str, arg_count: usize) -> Option<CompilerDiagnosticKind> {
    match name {
        "$derived" if arg_count != 1 => {
            Some(CompilerDiagnosticKind::RuneInvalidArgumentsLengthDerived)
        }
        "$effect" if arg_count != 1 => {
            Some(CompilerDiagnosticKind::RuneInvalidArgumentsLengthEffect)
        }
        "$state.raw" if arg_count > 1 => {
            Some(CompilerDiagnosticKind::RuneInvalidArgumentsLengthStateRaw)
        }
        "$state.snapshot" if arg_count != 1 => {
            Some(CompilerDiagnosticKind::RuneInvalidArgumentsLengthStateSnapshot)
        }
        "$state" if arg_count > 1 => Some(CompilerDiagnosticKind::RuneInvalidArgumentsLengthState),
        _ => None,
    }
}

fn find_props_invalid_placement_component(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() {
            return;
        }
        let Some((call_name, start, end, _)) = call_node_info(node) else {
            return;
        };
        if call_name.as_ref() != "$props" {
            return;
        }
        if !is_top_level_variable_initializer(path) {
            found = Some((start, end));
        }
    });
    found
}

fn find_bindable_invalid_location(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() {
            return;
        }
        let Some((call_name, start, end, _)) = call_node_info(node) else {
            return;
        };
        if call_name.as_ref() != "$bindable" {
            return;
        }
        if !is_bindable_props_destructure_location(path) {
            found = Some((start, end));
        }
    });
    found
}

fn is_bindable_props_destructure_location(path: &[PathStep<'_>]) -> bool {
    if path.len() < 4 {
        return false;
    }

    let assignment_step = &path[path.len() - 1];
    if estree_node_type(assignment_step.parent) != Some("AssignmentPattern")
        || assignment_step.via_key != "right"
    {
        return false;
    }

    let property_step = &path[path.len() - 2];
    if estree_node_type(property_step.parent) != Some("Property")
        || property_step.via_key != "value"
    {
        return false;
    }

    let object_pattern_step = &path[path.len() - 3];
    if estree_node_type(object_pattern_step.parent) != Some("ObjectPattern")
        || object_pattern_step.via_key != "properties"
    {
        return false;
    }

    let declarator_step = &path[path.len() - 4];
    if estree_node_type(declarator_step.parent) != Some("VariableDeclarator")
        || declarator_step.via_key != "id"
    {
        return false;
    }

    let Some(init) = estree_node_field_object(declarator_step.parent, RawField::Init) else {
        return false;
    };
    call_name_for_node(init).as_deref() == Some("$props")
}

fn find_invalid_initializer_placement(
    program: &EstreeNode,
    call_name: &str,
) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() {
            return;
        }
        let Some((name, start, end, _)) = call_node_info(node) else {
            return;
        };
        if name.as_ref() != call_name {
            return;
        }
        let Some(callee) = estree_node_field_object(node, RawField::Callee) else {
            return;
        };
        if call_resolves_to_non_rune(callee, call_name, path, program) {
            return;
        }
        if !is_initializer_context(path) {
            found = Some((start, end));
        }
    });
    found
}

fn find_effect_invalid_placement(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() {
            return;
        }
        let Some((name, start, end, _)) = call_node_info(node) else {
            return;
        };
        if name.as_ref() != "$effect" {
            return;
        }
        let Some(callee) = estree_node_field_object(node, RawField::Callee) else {
            return;
        };
        if call_resolves_to_non_rune(callee, "$effect", path, program) {
            return;
        }
        if !is_top_level_expression_statement(path) {
            found = Some((start, end));
        }
    });
    found
}

fn call_resolves_to_non_rune(
    callee: &EstreeNode,
    call_name: &str,
    path: &[PathStep<'_>],
    program: &EstreeNode,
) -> bool {
    if path_declares_alias(path, call_name) || scope_declares_alias(program, call_name) {
        return true;
    }

    let Some(alias) = call_name.strip_prefix('$') else {
        return false;
    };
    if alias.is_empty() {
        return false;
    }

    let Some(base_name) = raw_base_identifier_name(callee) else {
        return false;
    };
    if base_name.as_ref() != call_name {
        return false;
    }

    is_shadowed_store_alias_in_path(alias, path) || scope_declares_alias(program, alias)
}

fn find_static_state_call(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() {
            return;
        }
        let Some((name, start, end, _)) = call_node_info(node) else {
            return;
        };
        if name.as_ref() != "$state" {
            return;
        }
        let Some(parent) = path.last().map(|step| step.parent) else {
            return;
        };
        let is_static_field = matches!(
            estree_node_type(parent),
            Some("PropertyDefinition" | "ClassProperty")
        ) && path.last().is_some_and(|step| step.via_key == "value")
            && estree_node_field_bool(parent, RawField::Static).unwrap_or(false);
        if is_static_field {
            found = Some((start, end));
        }
    });
    found
}

fn find_rune_missing_parentheses_in_program(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() || estree_node_type(node) != Some("Identifier") {
            return;
        }
        let Some(name) = estree_node_field_str(node, RawField::Name) else {
            return;
        };
        if !matches!(name, "$bindable" | "$props") {
            return;
        }
        let is_call_callee = path.last().is_some_and(|step| {
            step.via_key == "callee" && estree_node_type(step.parent) == Some("CallExpression")
        });
        let is_member_call_object = path.last().is_some_and(|step| {
            step.via_key == "object"
                && estree_node_type(step.parent) == Some("MemberExpression")
                && path
                    .get(path.len().saturating_sub(2))
                    .is_some_and(|parent_step| {
                        parent_step.via_key == "callee"
                            && estree_node_type(parent_step.parent) == Some("CallExpression")
                    })
        });
        if is_call_callee || is_member_call_object {
            return;
        }
        found = estree_node_span(node);
    });
    found
}

fn find_invalid_rune_name(program: &EstreeNode) -> Option<(Arc<str>, usize, usize)> {
    let mut found = None::<(Arc<str>, usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, _path| {
        if found.is_some() || estree_node_type(node) != Some("MemberExpression") {
            return;
        }
        let Some(object) = estree_node_field_object(node, RawField::Object) else {
            return;
        };
        let Some(property) = estree_node_field_object(node, RawField::Property) else {
            return;
        };
        let Some(object_name) = raw_identifier_name(object) else {
            return;
        };
        let Some(property_name) = raw_identifier_name(property) else {
            return;
        };
        if object_name.as_ref() == "$state"
            && property_name.as_ref() != "raw"
            && property_name.as_ref() != "snapshot"
        {
            let full = Arc::<str>::from(format!("{object_name}.{property_name}"));
            if let Some((start, end)) = estree_node_span(node) {
                found = Some((full, start, end));
            }
            return;
        }

        if object_name.as_ref() == "$effect"
            && !matches!(
                property_name.as_ref(),
                "active" | "pre" | "tracking" | "root"
            )
        {
            let full = Arc::<str>::from(format!("{object_name}.{property_name}"));
            if let Some((start, end)) = estree_node_span(node) {
                found = Some((full, start, end));
            }
        }
    });
    found
}

fn find_constant_assignment(program: &EstreeNode) -> Option<(usize, usize)> {
    let outer_immutables = NameSet::default();
    find_constant_assignment_in_node(program, &outer_immutables)
}

pub(super) fn find_constant_assignment_in_expression(
    expression: &Expression,
    outer_immutables: &NameSet,
) -> Option<(usize, usize)> {
    find_constant_assignment_in_node(&expression.0, outer_immutables)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindingMutability {
    Immutable,
    Mutable,
}

#[derive(Default)]
struct BindingFrame {
    bindings: HashMap<Arc<str>, BindingMutability>,
}

impl BindingFrame {
    fn insert(&mut self, name: Arc<str>, mutability: BindingMutability) {
        self.bindings.insert(name, mutability);
    }

    fn get(&self, name: &str) -> Option<BindingMutability> {
        self.bindings.get(name).copied()
    }
}

struct BindingScopeStack {
    frames: Vec<BindingFrame>,
}

impl Default for BindingScopeStack {
    fn default() -> Self {
        Self {
            frames: vec![BindingFrame::default()],
        }
    }
}

impl BindingScopeStack {
    fn push_frame(&mut self) {
        self.frames.push(BindingFrame::default());
    }

    fn pop_frame(&mut self) {
        let _ = self.frames.pop();
    }

    fn current_frame_mut(&mut self) -> Option<&mut BindingFrame> {
        self.frames.last_mut()
    }

    fn declare(&mut self, name: Arc<str>, mutability: BindingMutability) {
        if let Some(frame) = self.current_frame_mut() {
            frame.insert(name, mutability);
        }
    }

    fn lookup(&self, name: &str) -> Option<BindingMutability> {
        self.frames.iter().rev().find_map(|frame| frame.get(name))
    }
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

struct ConstAssignmentAnalyzer<'a> {
    outer_immutables: &'a NameSet,
    found: Option<(usize, usize)>,
}

fn find_constant_assignment_in_node(
    node: &EstreeNode,
    outer_immutables: &NameSet,
) -> Option<(usize, usize)> {
    let mut analyzer = ConstAssignmentAnalyzer {
        outer_immutables,
        found: None,
    };
    let mut scopes = BindingScopeStack::default();
    analyzer.visit_node(node, &mut scopes);
    analyzer.found
}

impl<'a> ConstAssignmentAnalyzer<'a> {
    fn visit_node(&mut self, node: &EstreeNode, scopes: &mut BindingScopeStack) {
        if self.found.is_some() {
            return;
        }

        match estree_node_type(node) {
            Some("Program") => {
                if let Some(body) = estree_node_field_array(node, RawField::Body) {
                    self.visit_statement_list(body, scopes);
                } else {
                    self.visit_children(node, scopes);
                }
            }
            Some("BlockStatement") => {
                scopes.push_frame();
                if let Some(body) = estree_node_field_array(node, RawField::Body) {
                    self.visit_statement_list(body, scopes);
                } else {
                    self.visit_children(node, scopes);
                }
                scopes.pop_frame();
            }
            Some("VariableDeclaration") => {
                let mutability = if estree_node_field_str(node, RawField::Kind) == Some("const") {
                    BindingMutability::Immutable
                } else {
                    BindingMutability::Mutable
                };
                if let Some(declarations) = estree_node_field_array(node, RawField::Declarations) {
                    for declaration in declarations {
                        let EstreeValue::Object(declaration) = declaration else {
                            continue;
                        };
                        if let Some(id) = estree_node_field_object(declaration, RawField::Id) {
                            self.declare_pattern_bindings(id, mutability, scopes);
                        }
                    }
                    for declaration in declarations {
                        let EstreeValue::Object(declaration) = declaration else {
                            continue;
                        };
                        if let Some(init) = estree_node_field_object(declaration, RawField::Init) {
                            self.visit_node(init, scopes);
                            if self.found.is_some() {
                                return;
                            }
                        }
                    }
                }
            }
            Some("ImportDeclaration") => {
                if let Some(specifiers) = estree_node_field_array(node, RawField::Specifiers) {
                    for specifier in specifiers {
                        let EstreeValue::Object(specifier) = specifier else {
                            continue;
                        };
                        if let Some(local) = estree_node_field_object(specifier, RawField::Local) {
                            self.declare_pattern_bindings(
                                local,
                                BindingMutability::Immutable,
                                scopes,
                            );
                        }
                    }
                }
            }
            Some("ExportNamedDeclaration") => {
                if let Some(declaration) = estree_node_field_object(node, RawField::Declaration) {
                    self.visit_node(declaration, scopes);
                } else {
                    self.visit_children(node, scopes);
                }
            }
            Some("FunctionDeclaration") => {
                if let Some(id) = estree_node_field_object(node, RawField::Id)
                    && let Some(name) = raw_identifier_name(id)
                {
                    scopes.declare(name, BindingMutability::Mutable);
                }
                self.visit_function_node(node, scopes, false);
            }
            Some("FunctionExpression") => {
                self.visit_function_node(node, scopes, true);
            }
            Some("ArrowFunctionExpression") => {
                self.visit_function_node(node, scopes, false);
            }
            Some("ClassDeclaration") => {
                if let Some(id) = estree_node_field_object(node, RawField::Id)
                    && let Some(name) = raw_identifier_name(id)
                {
                    scopes.declare(name, BindingMutability::Mutable);
                }
                self.visit_children(node, scopes);
            }
            Some("CatchClause") => {
                scopes.push_frame();
                if let Some(EstreeValue::Object(param)) = node.fields.get("param") {
                    self.declare_pattern_bindings(param, BindingMutability::Mutable, scopes);
                }
                if let Some(body) = estree_node_field_object(node, RawField::Body) {
                    self.visit_node(body, scopes);
                } else {
                    self.visit_children(node, scopes);
                }
                scopes.pop_frame();
            }
            Some("AssignmentExpression") => {
                if let Some(left) = estree_node_field_object(node, RawField::Left)
                    && self.assignment_target_has_immutable_binding(left, scopes)
                    && let Some(span) = estree_node_span(node)
                {
                    self.found = Some(span);
                    return;
                }
                self.visit_children(node, scopes);
            }
            Some("UpdateExpression") => {
                if let Some(argument) = estree_node_field_object(node, RawField::Argument)
                    && self.assignment_target_has_immutable_binding(argument, scopes)
                    && let Some(span) = estree_node_span(node)
                {
                    self.found = Some(span);
                    return;
                }
                self.visit_children(node, scopes);
            }
            _ => self.visit_children(node, scopes),
        }
    }

    fn visit_statement_list(&mut self, statements: &[EstreeValue], scopes: &mut BindingScopeStack) {
        for statement in statements {
            let EstreeValue::Object(statement) = statement else {
                continue;
            };
            self.visit_node(statement, scopes);
            if self.found.is_some() {
                return;
            }
        }
    }

    fn visit_children(&mut self, node: &EstreeNode, scopes: &mut BindingScopeStack) {
        for value in node.fields.values() {
            self.visit_value(value, scopes);
            if self.found.is_some() {
                return;
            }
        }
    }

    fn visit_value(&mut self, value: &EstreeValue, scopes: &mut BindingScopeStack) {
        if self.found.is_some() {
            return;
        }
        match value {
            EstreeValue::Object(node) => self.visit_node(node, scopes),
            EstreeValue::Array(values) => {
                for value in values {
                    self.visit_value(value, scopes);
                    if self.found.is_some() {
                        return;
                    }
                }
            }
            EstreeValue::String(_)
            | EstreeValue::Int(_)
            | EstreeValue::UInt(_)
            | EstreeValue::Number(_)
            | EstreeValue::Bool(_)
            | EstreeValue::Null => {}
        }
    }

    fn visit_function_node(
        &mut self,
        node: &EstreeNode,
        scopes: &mut BindingScopeStack,
        declare_function_name_in_inner_scope: bool,
    ) {
        scopes.push_frame();

        if declare_function_name_in_inner_scope
            && let Some(id) = estree_node_field_object(node, RawField::Id)
            && let Some(name) = raw_identifier_name(id)
        {
            scopes.declare(name, BindingMutability::Mutable);
        }

        if let Some(params) = estree_node_field_array(node, RawField::Params) {
            for param in params {
                let EstreeValue::Object(param) = param else {
                    continue;
                };
                self.declare_pattern_bindings(param, BindingMutability::Mutable, scopes);
            }
        }

        if let Some(body) = estree_node_field_object(node, RawField::Body) {
            self.visit_node(body, scopes);
        }

        scopes.pop_frame();
    }

    fn declare_pattern_bindings(
        &self,
        pattern: &EstreeNode,
        mutability: BindingMutability,
        scopes: &mut BindingScopeStack,
    ) {
        let mut names = NameSet::default();
        extend_name_set_with_pattern_bindings(&mut names, pattern);
        for name in names {
            scopes.declare(name, mutability);
        }
    }

    fn assignment_target_has_immutable_binding(
        &self,
        target: &EstreeNode,
        scopes: &BindingScopeStack,
    ) -> bool {
        let mut names = Vec::new();
        crate::estree::collect_assignment_target_identifiers(target, &mut names);
        names.into_iter().any(|name| {
            self.lookup_binding_mutability(name.as_ref(), scopes)
                == Some(BindingMutability::Immutable)
        })
    }

    fn lookup_binding_mutability(
        &self,
        name: &str,
        scopes: &BindingScopeStack,
    ) -> Option<BindingMutability> {
        scopes.lookup(name).or_else(|| {
            self.outer_immutables
                .contains(name)
                .then_some(BindingMutability::Immutable)
        })
    }
}

fn call_node_info(node: &EstreeNode) -> Option<(Arc<str>, usize, usize, usize)> {
    if estree_node_type(node) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object(node, RawField::Callee)?;
    let call_name = raw_callee_name(callee)?;
    let arg_count = estree_node_field_array(node, RawField::Arguments)
        .map(|args| args.len())
        .unwrap_or(0);
    let (start, end) = estree_node_span(node)?;
    Some((call_name, start, end, arg_count))
}

fn call_name_for_node(node: &EstreeNode) -> Option<Arc<str>> {
    if estree_node_type(node) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object(node, RawField::Callee)?;
    raw_callee_name(callee)
}

fn compile_error_custom_runes(
    source: &str,
    code: &'static str,
    message: impl Into<Arc<str>>,
    start: usize,
    end: usize,
) -> CompileError {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let start_location = source_text.location_at_offset(start);
    let end_location = source_text.location_at_offset(end);

    CompileError {
        code: Arc::from(code),
        message: message.into(),
        position: Some(Box::new(SourcePosition {
            start: start_location.character,
            end: end_location.character,
        })),
        start: Some(Box::new(start_location)),
        end: Some(Box::new(end_location)),
        filename: None,
    }
}

fn find_class_state_field_error(program: &EstreeNode) -> Option<ClassStateFieldError> {
    let mut found = None;
    walk_estree_node(program, &mut |node| {
        if found.is_some() || estree_node_type(node) != Some("ClassBody") {
            return;
        }
        found = validate_class_body(node);
    });
    found
}

fn validate_class_body<'a>(body: &'a EstreeNode) -> Option<ClassStateFieldError> {
    let members = estree_node_field_array(body, RawField::Body)?;
    let mut fields = ClassFieldIndex::new();
    let mut constructor = None::<&'a EstreeNode>;

    for member in members {
        let EstreeValue::Object(member) = member else {
            continue;
        };
        match estree_node_type(member) {
            Some("PropertyDefinition") => {
                if estree_node_field_bool_named(member, "computed").unwrap_or(false)
                    || estree_node_field_bool_named(member, "static").unwrap_or(false)
                {
                    continue;
                }

                let Some(key) = estree_node_field_object(member, RawField::Key) else {
                    continue;
                };
                let Some(name) = class_key_name(key) else {
                    continue;
                };

                if let Some(error) = fields.record_state_field(member, &name) {
                    return Some(error);
                }

                let kind = if estree_node_field_object(member, RawField::Value).is_some() {
                    FieldKind::AssignedProp
                } else {
                    FieldKind::Prop
                };

                if let Some(error) = fields.record_property_kind(member, name, kind) {
                    return Some(error);
                }
            }
            Some("MethodDefinition") => {
                let kind = estree_node_field_str(member, RawField::Kind);
                if kind == Some("constructor") {
                    constructor = Some(member);
                    continue;
                }
                if estree_node_field_bool_named(member, "computed").unwrap_or(false) {
                    continue;
                }

                let Some(key) = estree_node_field_object(member, RawField::Key) else {
                    continue;
                };
                let Some(name) = class_key_name(key) else {
                    continue;
                };
                let name = if estree_node_field_bool_named(member, "static").unwrap_or(false) {
                    Arc::<str>::from(format!("@{name}"))
                } else {
                    name
                };
                let kind = match kind {
                    Some("get") => FieldKind::Get,
                    Some("set") => FieldKind::Set,
                    _ => FieldKind::Method,
                };

                if let Some(error) = fields.record_method_kind(member, name, kind) {
                    return Some(error);
                }
            }
            _ => {}
        }
    }

    if let Some(constructor) = constructor
        && let Some(error) = validate_constructor_state_fields(constructor, &mut fields)
    {
        return Some(error);
    }

    for member in members {
        let EstreeValue::Object(member) = member else {
            continue;
        };
        if estree_node_type(member) != Some("PropertyDefinition")
            || estree_node_field_object(member, RawField::Value).is_none()
        {
            continue;
        }
        let Some(key) = estree_node_field_object(member, RawField::Key) else {
            continue;
        };
        let Some(name) = class_key_name(key) else {
            continue;
        };
        let Some(field) = fields.state_field(name.as_ref()) else {
            continue;
        };
        if std::ptr::eq(member, field.node) {
            continue;
        }
        let Some((start, end)) = estree_node_span(member) else {
            continue;
        };
        let Some((field_start, _)) = estree_node_span(field.node) else {
            continue;
        };
        if start < field_start {
            return Some(ClassStateFieldError {
                kind: CompilerDiagnosticKind::StateFieldInvalidAssignment,
                start,
                end,
            });
        }
    }

    if let Some(constructor) = constructor {
        return find_constructor_state_assignment_before_declaration(constructor, &fields);
    }

    None
}

fn validate_constructor_state_fields<'a>(
    constructor: &'a EstreeNode,
    fields: &mut ClassFieldIndex<'a>,
) -> Option<ClassStateFieldError> {
    let function = estree_node_field_object(constructor, RawField::Value)?;
    let body = estree_node_field_object(function, RawField::Body)?;
    let statements = estree_node_field_array(body, RawField::Body)?;

    for statement in statements {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ExpressionStatement") {
            continue;
        }
        let Some(expression) = estree_node_field_object(statement, RawField::Expression) else {
            continue;
        };
        if estree_node_type(expression) != Some("AssignmentExpression") {
            continue;
        }
        let Some(left) = estree_node_field_object(expression, RawField::Left) else {
            continue;
        };
        let Some(name) = this_member_name(left) else {
            continue;
        };
        if let Some(error) = fields.record_state_field(expression, &name) {
            return Some(error);
        }
    }

    None
}

fn find_constructor_state_assignment_before_declaration(
    constructor: &EstreeNode,
    fields: &ClassFieldIndex<'_>,
) -> Option<ClassStateFieldError> {
    let function = estree_node_field_object(constructor, RawField::Value)?;
    let body = estree_node_field_object(function, RawField::Body)?;
    let mut found = None;

    walk_estree_node_with_path(body, &mut Vec::new(), &mut |node, path| {
        if found.is_some() || estree_node_type(node) != Some("AssignmentExpression") {
            return;
        }
        if path.iter().any(|step| {
            matches!(
                estree_node_type(step.parent),
                Some("FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression")
            )
        }) {
            return;
        }

        let Some(left) = estree_node_field_object(node, RawField::Left) else {
            return;
        };
        let Some(name) = this_member_name(left) else {
            return;
        };
        let Some(field) = fields.state_field(name.as_ref()) else {
            return;
        };
        if estree_node_type(field.node) != Some("AssignmentExpression")
            || std::ptr::eq(node, field.node)
        {
            return;
        }

        let Some((start, end)) = estree_node_span(node) else {
            return;
        };
        let Some((field_start, _)) = estree_node_span(field.node) else {
            return;
        };
        if start < field_start {
            found = Some(ClassStateFieldError {
                kind: CompilerDiagnosticKind::StateFieldInvalidAssignment,
                start,
                end,
            });
        }
    });

    found
}

fn estree_node_value(node: &EstreeNode) -> Option<&EstreeNode> {
    match estree_node_type(node) {
        Some("PropertyDefinition") => estree_node_field_object(node, RawField::Value),
        Some("AssignmentExpression") => estree_node_field_object(node, RawField::Right),
        _ => None,
    }
}

fn is_state_creation_call(node: &EstreeNode) -> bool {
    if estree_node_type(node) != Some("CallExpression") {
        return false;
    }
    let Some(callee) = estree_node_field_object(node, RawField::Callee) else {
        return false;
    };
    matches!(
        raw_callee_name(callee).as_deref(),
        Some("$state" | "$state.raw" | "$derived" | "$derived.by")
    )
}

fn state_field_duplicate_error(node: &EstreeNode, name: Arc<str>) -> ClassStateFieldError {
    let (start, end) = estree_node_span(node).unwrap_or((0, 0));
    ClassStateFieldError {
        kind: CompilerDiagnosticKind::StateFieldDuplicate { name },
        start,
        end,
    }
}

fn duplicate_class_field_error(node: &EstreeNode, name: Arc<str>) -> ClassStateFieldError {
    let (start, end) = estree_node_span(node).unwrap_or((0, 0));
    ClassStateFieldError {
        kind: CompilerDiagnosticKind::DuplicateClassField { name },
        start,
        end,
    }
}

fn is_initializer_context(path: &[PathStep<'_>]) -> bool {
    let path = strip_typescript_wrapper_steps(path);
    let Some(step) = path.last() else {
        return false;
    };
    if estree_node_type(step.parent) == Some("VariableDeclarator") && step.via_key == "init" {
        return true;
    }
    matches!(
        estree_node_type(step.parent),
        Some("PropertyDefinition" | "ClassProperty")
    ) && step.via_key == "value"
        || is_top_level_constructor_field_assignment(path)
}

fn is_top_level_constructor_field_assignment(path: &[PathStep<'_>]) -> bool {
    if path.len() < 5 {
        return false;
    }

    let assignment_step = &path[path.len() - 1];
    if estree_node_type(assignment_step.parent) != Some("AssignmentExpression")
        || assignment_step.via_key != "right"
    {
        return false;
    }

    let Some(left) = estree_node_field_object(assignment_step.parent, RawField::Left) else {
        return false;
    };
    if this_member_name(left).is_none() {
        return false;
    }

    let expression_step = &path[path.len() - 2];
    if estree_node_type(expression_step.parent) != Some("ExpressionStatement")
        || expression_step.via_key != "expression"
    {
        return false;
    }

    let block_step = &path[path.len() - 3];
    if estree_node_type(block_step.parent) != Some("BlockStatement") || block_step.via_key != "body"
    {
        return false;
    }

    let function_step = &path[path.len() - 4];
    if estree_node_type(function_step.parent) != Some("FunctionExpression")
        || function_step.via_key != "body"
    {
        return false;
    }

    let method_step = &path[path.len() - 5];
    estree_node_type(method_step.parent) == Some("MethodDefinition")
        && method_step.via_key == "value"
        && estree_node_field_str(method_step.parent, RawField::Kind) == Some("constructor")
}

fn is_top_level_variable_initializer(path: &[PathStep<'_>]) -> bool {
    let path = strip_typescript_wrapper_steps(path);
    let Some(last) = path.last() else {
        return false;
    };
    if estree_node_type(last.parent) != Some("VariableDeclarator") || last.via_key != "init" {
        return false;
    }
    let has_function_ancestor = path.iter().any(|step| {
        matches!(
            estree_node_type(step.parent),
            Some(
                "FunctionDeclaration"
                    | "FunctionExpression"
                    | "ArrowFunctionExpression"
                    | "MethodDefinition"
            )
        )
    });
    if has_function_ancestor {
        return false;
    }
    path.iter()
        .any(|step| estree_node_type(step.parent) == Some("Program") && step.via_key == "body")
}

fn strip_typescript_wrapper_steps<'a>(path: &'a [PathStep<'a>]) -> &'a [PathStep<'a>] {
    let mut end = path.len();
    while end > 0 {
        let step = &path[end - 1];
        let is_wrapper = matches!(
            estree_node_type(step.parent),
            Some(
                "ParenthesizedExpression"
                    | "TSAsExpression"
                    | "TSSatisfiesExpression"
                    | "TSNonNullExpression"
                    | "TSTypeAssertion"
            )
        );
        if !is_wrapper || step.via_key != "expression" {
            break;
        }
        end -= 1;
    }
    &path[..end]
}

fn is_top_level_expression_statement(path: &[PathStep<'_>]) -> bool {
    let Some(last) = path.last() else {
        return false;
    };
    if estree_node_type(last.parent) != Some("ExpressionStatement") || last.via_key != "expression"
    {
        return false;
    }
    path.iter()
        .any(|step| estree_node_type(step.parent) == Some("Program") && step.via_key == "body")
}

fn estree_node_field_bool(node: &EstreeNode, key: RawField) -> Option<bool> {
    match estree_node_field(node, key) {
        Some(EstreeValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn estree_node_field_bool_named(node: &EstreeNode, key: &str) -> Option<bool> {
    match node.fields.get(key) {
        Some(EstreeValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn find_store_invalid_subscription(program: &EstreeNode) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() || estree_node_type(node) != Some("Identifier") {
            return;
        }
        let Some(name) = estree_node_field_str(node, RawField::Name) else {
            return;
        };
        if name.len() <= 1 || !name.starts_with('$') || !name.as_bytes()[1].is_ascii_alphabetic() {
            return;
        }
        if is_allowed_rune_name(name) {
            return;
        }
        if is_ignored_store_identifier_context(path) {
            return;
        }
        if let Some(span) = estree_node_span(node) {
            found = Some(span);
        }
    });
    found
}

fn is_ignored_store_identifier_context(path: &[PathStep<'_>]) -> bool {
    let Some(step) = path.last() else {
        return false;
    };
    let parent_type = estree_node_type(step.parent);
    if matches!(
        parent_type,
        Some(
            "VariableDeclarator"
                | "FunctionDeclaration"
                | "FunctionExpression"
                | "ArrowFunctionExpression"
                | "ClassDeclaration"
                | "ImportSpecifier"
                | "ImportDefaultSpecifier"
                | "ImportNamespaceSpecifier"
                | "CatchClause"
                | "LabeledStatement"
                | "BreakStatement"
                | "ContinueStatement"
        )
    ) && matches!(step.via_key, "id" | "params" | "local" | "param" | "label")
    {
        return true;
    }
    if parent_type == Some("MemberExpression") && step.via_key == "property" {
        return true;
    }
    if parent_type == Some("Property") && step.via_key == "key" {
        return true;
    }
    false
}

fn detect_dollar_binding_error_in_program(
    source: &str,
    program: &EstreeNode,
    runes_mode: bool,
) -> Option<CompileError> {
    if let Some((start, _end)) = find_dollar_binding_invalid_declaration(program, runes_mode) {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::DollarBindingInvalid,
            start,
            start + 1,
        ));
    }
    if let Some((ident, start, end)) =
        find_invalid_global_rune_reference_in_node(program, runes_mode)
    {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::GlobalReferenceInvalid { ident },
            start,
            end,
        ));
    }
    None
}

fn find_dollar_binding_invalid_declaration(
    program: &EstreeNode,
    runes_mode: bool,
) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if found.is_some() || estree_node_type(node) != Some("Identifier") {
            return;
        }
        let Some(name) = estree_node_field_str(node, RawField::Name) else {
            return;
        };
        if !name.starts_with('$') {
            return;
        }
        let Some(step) = path.last() else {
            return;
        };
        let parent_type = estree_node_type(step.parent);
        let in_declaration = (parent_type == Some("VariableDeclarator") && step.via_key == "id")
            || (matches!(
                parent_type,
                Some("ImportSpecifier" | "ImportDefaultSpecifier" | "ImportNamespaceSpecifier")
            ) && step.via_key == "local");
        if !in_declaration {
            return;
        }
        if !runes_mode && path_has_function_scope(path) {
            return;
        }
        if let Some(span) = estree_node_span(node) {
            found = Some(span);
        }
    });
    found
}

fn find_invalid_global_reference_in_fragment(
    fragment: &Fragment,
    runes_mode: bool,
    instance: Option<&EstreeNode>,
) -> Option<(Arc<str>, usize, usize)> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        match node {
            Node::ExpressionTag(tag) => find_invalid_global_rune_reference_in_node_with_program(
                &tag.expression.0,
                runes_mode,
                instance,
            ),
            Node::RenderTag(tag) => find_invalid_global_rune_reference_in_node_with_program(
                &tag.expression.0,
                runes_mode,
                instance,
            ),
            Node::HtmlTag(tag) => find_invalid_global_rune_reference_in_node_with_program(
                &tag.expression.0,
                runes_mode,
                instance,
            ),
            Node::ConstTag(tag) => find_invalid_global_rune_reference_in_node_with_program(
                &tag.declaration.0,
                runes_mode,
                instance,
            ),
            Node::EachBlock(block) => {
                find_invalid_global_reference_in_each_block(block, runes_mode, instance)
            }
            Node::AwaitBlock(block) => {
                find_invalid_global_reference_in_await_block(block, runes_mode, instance)
            }
            Node::KeyBlock(block) => find_invalid_global_rune_reference_in_node_with_program(
                &block.expression.0,
                runes_mode,
                instance,
            ),
            _ => node.as_element().and_then(|element| {
                find_invalid_global_reference_in_attributes(
                    element.attributes(),
                    runes_mode,
                    instance,
                )
            }),
        }
    })
}

fn find_invalid_global_reference_in_each_block(
    block: &EachBlock,
    runes_mode: bool,
    instance: Option<&EstreeNode>,
) -> Option<(Arc<str>, usize, usize)> {
    find_invalid_global_rune_reference_in_node_with_program(
        &block.expression.0,
        runes_mode,
        instance,
    )
    .or_else(|| {
        block.context.as_ref().and_then(|context| {
            find_invalid_global_rune_reference_in_node_with_program(
                &context.0, runes_mode, instance,
            )
        })
    })
    .or_else(|| {
        block.key.as_ref().and_then(|key| {
            find_invalid_global_rune_reference_in_node_with_program(&key.0, runes_mode, instance)
        })
    })
}

fn find_invalid_global_reference_in_await_block(
    block: &crate::ast::modern::AwaitBlock,
    runes_mode: bool,
    instance: Option<&EstreeNode>,
) -> Option<(Arc<str>, usize, usize)> {
    find_invalid_global_rune_reference_in_node_with_program(
        &block.expression.0,
        runes_mode,
        instance,
    )
    .or_else(|| {
        block.value.as_ref().and_then(|value| {
            find_invalid_global_rune_reference_in_node_with_program(&value.0, runes_mode, instance)
        })
    })
    .or_else(|| {
        block.error.as_ref().and_then(|error| {
            find_invalid_global_rune_reference_in_node_with_program(&error.0, runes_mode, instance)
        })
    })
}

fn find_invalid_global_reference_in_attributes(
    attributes: &[Attribute],
    runes_mode: bool,
    instance: Option<&EstreeNode>,
) -> Option<(Arc<str>, usize, usize)> {
    for attribute in attributes {
        let found = match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => None,
                AttributeValueList::ExpressionTag(tag) => {
                    find_invalid_global_rune_reference_in_node_with_program(
                        &tag.expression.0,
                        runes_mode,
                        instance,
                    )
                }
                AttributeValueList::Values(values) => values.iter().find_map(|value| match value {
                    AttributeValue::ExpressionTag(tag) => {
                        find_invalid_global_rune_reference_in_node_with_program(
                            &tag.expression.0,
                            runes_mode,
                            instance,
                        )
                    }
                    AttributeValue::Text(_) => None,
                }),
            },
            Attribute::SpreadAttribute(attribute) => {
                find_invalid_global_rune_reference_in_node_with_program(
                    &attribute.expression.0,
                    runes_mode,
                    instance,
                )
            }
            Attribute::AnimateDirective(attribute)
            | Attribute::BindDirective(attribute)
            | Attribute::ClassDirective(attribute)
            | Attribute::LetDirective(attribute)
            | Attribute::OnDirective(attribute)
            | Attribute::UseDirective(attribute) => {
                find_invalid_global_rune_reference_in_node_with_program(
                    &attribute.expression.0,
                    runes_mode,
                    instance,
                )
            }
            Attribute::TransitionDirective(attribute) => {
                find_invalid_global_rune_reference_in_node_with_program(
                    &attribute.expression.0,
                    runes_mode,
                    instance,
                )
            }
            Attribute::StyleDirective(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => None,
                AttributeValueList::ExpressionTag(tag) => {
                    find_invalid_global_rune_reference_in_node_with_program(
                        &tag.expression.0,
                        runes_mode,
                        instance,
                    )
                }
                AttributeValueList::Values(values) => values.iter().find_map(|value| match value {
                    AttributeValue::ExpressionTag(tag) => {
                        find_invalid_global_rune_reference_in_node_with_program(
                            &tag.expression.0,
                            runes_mode,
                            instance,
                        )
                    }
                    AttributeValue::Text(_) => None,
                }),
            },
            Attribute::AttachTag(tag) => find_invalid_global_rune_reference_in_node_with_program(
                &tag.expression.0,
                runes_mode,
                instance,
            ),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn find_invalid_global_rune_reference_in_node(
    node: &EstreeNode,
    runes_mode: bool,
) -> Option<(Arc<str>, usize, usize)> {
    find_invalid_global_rune_reference_in_node_with_program(node, runes_mode, None)
}

fn find_invalid_global_rune_reference_in_node_with_program(
    node: &EstreeNode,
    runes_mode: bool,
    program: Option<&EstreeNode>,
) -> Option<(Arc<str>, usize, usize)> {
    let mut found = None::<(Arc<str>, usize, usize)>;
    walk_estree_node_with_path(node, &mut Vec::new(), &mut |node, path| {
        if found.is_some() {
            return;
        }
        match estree_node_type(node) {
            Some("Identifier") => {
                let Some(name) = estree_node_field_str(node, RawField::Name) else {
                    return;
                };
                if (name != "$" && !name.starts_with("$$"))
                    || is_ignored_store_identifier_context(path)
                {
                    return;
                }
                if !runes_mode && is_legacy_component_api_reference(name) {
                    return;
                }
                if let Some((start, end)) = estree_node_span(node) {
                    found = Some((Arc::from(name), start, end));
                }
            }
            Some("CallExpression") => {
                let Some(callee) = estree_node_field_object(node, RawField::Callee) else {
                    return;
                };

                if estree_node_type(callee) == Some("MemberExpression")
                    && let Some(object) = estree_node_field_object(callee, RawField::Object)
                    && let Some(object_name) = raw_identifier_name(object)
                    && matches!(
                        object_name.as_ref(),
                        "$state" | "$effect" | "$derived" | "$inspect"
                    )
                {
                    // Let rune-specific diagnostics handle known rune namespaces.
                    return;
                }

                let Some(name) = raw_callee_name(callee) else {
                    return;
                };
                if !name.starts_with('$') || is_allowed_rune_name(name.as_ref()) {
                    return;
                }
                if legacy_dollar_callee_is_allowed(
                    callee,
                    path,
                    program.or_else(|| program_scope_in_path(path)),
                ) {
                    return;
                }
                if let Some((start, end)) =
                    estree_node_span(callee).or_else(|| estree_node_span(node))
                {
                    found = Some((name, start, end));
                }
            }
            _ => {}
        }
    });
    found
}

fn is_legacy_component_api_reference(name: &str) -> bool {
    matches!(name, "$$props" | "$$restProps" | "$$slots")
}

fn program_scope_in_path<'a>(path: &'a [PathStep<'a>]) -> Option<&'a EstreeNode> {
    path.iter()
        .rev()
        .find_map(|step| (estree_node_type(step.parent) == Some("Program")).then_some(step.parent))
}

fn path_declares_alias(path: &[PathStep<'_>], alias: &str) -> bool {
    path.iter().rev().any(|step| {
        estree_node_type(step.parent) != Some("Program") && scope_declares_alias(step.parent, alias)
    })
}

fn legacy_dollar_callee_is_allowed(
    callee: &EstreeNode,
    path: &[PathStep<'_>],
    program: Option<&EstreeNode>,
) -> bool {
    let Some(base_name) = raw_base_identifier_name(callee) else {
        return false;
    };
    if is_legacy_component_api_reference(base_name.as_ref()) {
        return true;
    }
    if !base_name.starts_with('$') || base_name.starts_with("$$") {
        return false;
    }
    if path_declares_alias(path, base_name.as_ref()) {
        return true;
    }
    let Some(alias) = base_name.strip_prefix('$') else {
        return false;
    };
    if alias.is_empty() {
        return false;
    }
    program.is_some_and(|program| scope_declares_alias(program, alias))
}

fn find_state_in_each_header_fragment(fragment: &Fragment) -> Option<(usize, usize)> {
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

fn find_state_call_in_expression(expression: &Expression) -> Option<(usize, usize)> {
    find_first_call_span_by_name(&expression.0, "$state")
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
) -> Option<(usize, usize)> {
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
                        && let Some(identifier_name) = raw_identifier_name(&context.0)
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
                    .is_some_and(|context| raw_identifier_name(&context.0).is_some())
            {
                scoped_aliases.pop();
            }
        },
    )
}

fn find_store_invalid_scoped_subscription_in_attributes(
    attributes: &[Attribute],
    scoped_aliases: &AliasStack,
) -> Option<(usize, usize)> {
    for attribute in attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::Values(values) => {
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
                AttributeValueList::ExpressionTag(tag) => {
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
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::Values(values) => {
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
                AttributeValueList::ExpressionTag(tag) => {
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
) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(&expression.0, &mut Vec::new(), &mut |node, path| {
        if estree_node_type(node) != Some("Identifier") {
            return;
        }
        let Some(name) = estree_node_field_str(node, RawField::Name) else {
            return;
        };
        if is_allowed_rune_name(name) {
            return;
        }
        if is_ignored_store_identifier_context(path) {
            return;
        }
        let Some((start, end)) = estree_node_span(node) else {
            return;
        };
        if let Some(span) =
            scoped_store_identifier_span_in_path(name, start, end, scoped_aliases, path)
        {
            keep_earliest_span(&mut found, span);
        }
    });
    found
}

fn find_state_call_in_each_binding_shape(block: &EachBlock) -> Option<(usize, usize)> {
    if !block.has_as_clause {
        return None;
    }
    let context = block.context.as_ref()?;
    let key = block.key.as_ref()?;
    if expression_identifier_name(context).as_deref() != Some("$state") {
        return None;
    }
    let (start, _) = estree_node_span(&context.0)?;
    let (_, end) = estree_node_span(&key.0)?;
    Some((start, end))
}

fn find_store_invalid_scoped_subscription_in_program(
    program: &EstreeNode,
) -> Option<(usize, usize)> {
    let mut found = None::<(usize, usize)>;
    walk_estree_node_with_path(program, &mut Vec::new(), &mut |node, path| {
        if estree_node_type(node) != Some("Identifier") {
            return;
        }
        let Some(name) = estree_node_field_str(node, RawField::Name) else {
            return;
        };
        if is_allowed_rune_name(name) {
            return;
        }
        if is_ignored_store_identifier_context(path) {
            return;
        }
        let Some((start, end)) = estree_node_span(node) else {
            return;
        };
        if let Some(span) =
            scoped_store_identifier_span_in_path(name, start, end, &AliasStack::default(), path)
        {
            keep_earliest_span(&mut found, span);
        }
    });
    found
}

fn keep_earliest_span(found: &mut Option<(usize, usize)>, candidate: (usize, usize)) {
    let replace = found.is_none_or(|current| candidate.0 < current.0);
    if replace {
        *found = Some(candidate);
    }
}

fn scoped_store_identifier_span(
    identifier: &str,
    start: usize,
    end: usize,
    scoped_aliases: &AliasStack,
) -> Option<(usize, usize)> {
    let alias = identifier.strip_prefix('$')?;
    if alias.is_empty() {
        return None;
    }
    if scoped_aliases.contains(alias) {
        return Some((start, end));
    }
    None
}

fn scoped_store_identifier_span_in_path(
    identifier: &str,
    start: usize,
    end: usize,
    scoped_aliases: &AliasStack,
    path: &[PathStep<'_>],
) -> Option<(usize, usize)> {
    let alias = identifier.strip_prefix('$')?;
    if alias.is_empty() {
        return None;
    }
    if scoped_aliases.contains(alias) || is_shadowed_store_alias_in_path(alias, path) {
        return Some((start, end));
    }
    None
}

fn is_shadowed_store_alias_in_path(alias: &str, path: &[PathStep<'_>]) -> bool {
    for step in path.iter().rev() {
        let parent = step.parent;
        if estree_node_type(parent) == Some("Program") {
            continue;
        }
        if scope_declares_alias(parent, alias) {
            return true;
        }
    }
    false
}

fn scope_declares_alias(scope: &EstreeNode, alias: &str) -> bool {
    if function_scope_declares_alias(scope, alias) {
        return true;
    }

    match estree_node_type(scope) {
        Some("BlockStatement" | "Program") => {
            let Some(body) = estree_node_field_array(scope, RawField::Body) else {
                return false;
            };
            body.iter().any(|statement| {
                let EstreeValue::Object(statement) = statement else {
                    return false;
                };
                statement_declares_alias(statement, alias)
            })
        }
        Some("ForStatement") => {
            let Some(init) = estree_node_field(scope, RawField::Init) else {
                return false;
            };
            node_or_declaration_declares_alias(init, alias)
        }
        Some("ForInStatement" | "ForOfStatement") => {
            let Some(left) = estree_node_field(scope, RawField::Left) else {
                return false;
            };
            node_or_declaration_declares_alias(left, alias)
        }
        _ => false,
    }
}

fn function_scope_declares_alias(function: &EstreeNode, alias: &str) -> bool {
    if !matches!(
        estree_node_type(function),
        Some("FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression")
    ) {
        return false;
    }
    if let Some(params) = estree_node_field_array(function, RawField::Params)
        && params.iter().any(|param| {
            let EstreeValue::Object(param) = param else {
                return false;
            };
            pattern_binds_name(param, alias)
        })
    {
        return true;
    }

    if estree_node_type(function) == Some("FunctionExpression")
        && let Some(id) = estree_node_field_object(function, RawField::Id)
        && raw_identifier_name(id).as_deref() == Some(alias)
    {
        return true;
    }

    false
}

fn statement_declares_alias(statement: &EstreeNode, alias: &str) -> bool {
    match estree_node_type(statement) {
        Some("VariableDeclaration") => variable_declaration_declares_alias(statement, alias),
        Some("FunctionDeclaration" | "ClassDeclaration") => {
            estree_node_field_object(statement, RawField::Id)
                .and_then(raw_identifier_name)
                .is_some_and(|name| name.as_ref() == alias)
        }
        Some("ForStatement" | "ForInStatement" | "ForOfStatement") => {
            scope_declares_alias(statement, alias)
        }
        _ => false,
    }
}

fn variable_declaration_declares_alias(declaration: &EstreeNode, alias: &str) -> bool {
    let Some(declarations) = estree_node_field_array(declaration, RawField::Declarations) else {
        return false;
    };
    declarations.iter().any(|declarator| {
        let EstreeValue::Object(declarator) = declarator else {
            return false;
        };
        let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
            return false;
        };
        pattern_binds_name(id, alias)
    })
}

fn node_or_declaration_declares_alias(value: &EstreeValue, alias: &str) -> bool {
    let EstreeValue::Object(node) = value else {
        return false;
    };
    match estree_node_type(node) {
        Some("VariableDeclaration") => variable_declaration_declares_alias(node, alias),
        _ => pattern_binds_name(node, alias),
    }
}

struct RenderTagDiagnostic {
    kind: CompilerDiagnosticKind,
    start: usize,
    end: usize,
}

fn validate_render_tag(tag: &crate::ast::modern::RenderTag) -> Option<RenderTagDiagnostic> {
    let call = match unwrap_render_tag_call(&tag.expression, (tag.start, tag.end)) {
        Ok(call) => call,
        Err(error) => return Some(error),
    };
    let arguments = estree_node_field_array(call, RawField::Arguments).unwrap_or(&[]);
    for argument in arguments {
        let EstreeValue::Object(argument) = argument else {
            continue;
        };
        if estree_node_type(argument) == Some("SpreadElement") {
            let (start, end) = estree_node_span(argument).or_else(|| estree_node_span(call))?;
            return Some(RenderTagDiagnostic {
                kind: CompilerDiagnosticKind::RenderTagInvalidSpreadArgument,
                start,
                end,
            });
        }
    }

    let callee = estree_node_field_object(call, RawField::Callee)?;
    if matches!(
        raw_member_property_name(callee).as_deref(),
        Some("apply" | "bind" | "call")
    ) {
        return Some(RenderTagDiagnostic {
            kind: CompilerDiagnosticKind::RenderTagInvalidCallExpression,
            start: tag.start,
            end: tag.end,
        });
    }

    None
}

fn unwrap_render_tag_call(
    expression: &Expression,
    fallback_span: (usize, usize),
) -> Result<&EstreeNode, RenderTagDiagnostic> {
    let raw = unwrap_optional_expression(&expression.0);
    if estree_node_type(raw) == Some("CallExpression") {
        return Ok(raw);
    }

    let (start, end) = estree_node_span(&expression.0)
        .or_else(|| estree_node_span(raw))
        .unwrap_or(fallback_span);
    Err(RenderTagDiagnostic {
        kind: CompilerDiagnosticKind::RenderTagInvalidExpression,
        start,
        end,
    })
}

fn unwrap_optional_expression(raw: &EstreeNode) -> &EstreeNode {
    if estree_node_type(raw) == Some("ChainExpression") {
        return estree_node_field_object(raw, RawField::Expression).unwrap_or(raw);
    }
    raw
}
