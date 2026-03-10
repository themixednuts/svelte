use std::{collections::HashSet, sync::Arc};

pub(super) use super::modern::{
    RawField, estree_node_field, estree_node_field_array, estree_node_field_object,
    estree_node_field_str, estree_node_type, walk_estree_node,
};
use super::*;
use crate::ast::modern::Root;
use crate::error::CompilerDiagnosticKind;

mod css;
mod imports;
mod runes;
mod snippet;
mod template;

pub(super) fn compile_error_with_range(
    source: &str,
    kind: CompilerDiagnosticKind,
    start: usize,
    end: usize,
) -> CompileError {
    kind.to_compile_error(source, start, end)
}

fn is_error_mode_warn(options: &CompileOptions) -> bool {
    matches!(options.error_mode, crate::api::ErrorMode::Warn)
}

fn downgrade_constant_assignment_warning(error: CompileError) -> CompileError {
    CompileError {
        code: Arc::from("invalid_const_assignment"),
        message: Arc::from("Invalid assignment to const"),
        ..error
    }
}

pub(crate) fn collect_error_mode_downgraded_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Vec<CompileError> {
    if !is_error_mode_warn(options) {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    let runes_mode = infer_runes_mode(options, root);

    if let Some(error) = template::detect_constant_binding_from_root(source, root) {
        warnings.push(downgrade_constant_assignment_warning(error));
    }

    if let Some(error) =
        template::detect_bind_invalid_value_warn_mode_from_root(source, root, runes_mode)
    {
        warnings.push(error);
    }

    warnings
}

pub(crate) fn validate_component_template(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Option<CompileError> {
    let runes_mode = infer_runes_mode(options, root);
    let each_context_error = template::detect_each_context_error(source, root);
    let parse_error = template::detect_parse_error_from_root(source, root, runes_mode);
    let defer_block_unexpected_character = matches!(
        parse_error.as_ref().map(|error| error.code.as_ref()),
        Some("block_unexpected_character")
    );
    let defer_invalid_closing_tag = matches!(
        root.errors.first().map(|error| &error.kind),
        Some(
            crate::ast::common::ParseErrorKind::ElementInvalidClosingTag { .. }
                | crate::ast::common::ParseErrorKind::ElementInvalidClosingTagAutoclosed { .. }
        )
    );
    if let Some(error) = each_context_error {
        return Some(error);
    }
    if !defer_invalid_closing_tag
        && !defer_block_unexpected_character
        && let Some(error) = parse_error
    {
        return Some(error);
    }

    if let Some(error) = template::detect_svelte_meta_structure_errors(source, root) {
        return Some(error);
    }

    if let Some(error) =
        template::detect_template_directive_errors_from_root(source, root, runes_mode)
    {
        if !(is_error_mode_warn(options) && error.code.as_ref() == "constant_binding") {
            return Some(error);
        }
    }
    if let Some(error) = template::detect_script_duplicate_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_typescript_invalid_features_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_svelte_options_invalid_namespace_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) =
        template::detect_svelte_options_invalid_custom_element_from_root(source, root)
    {
        return Some(error);
    }
    if let Some(error) = template::detect_svelte_head_illegal_attribute_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_tag_invalid_name(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_let_directive_invalid_placement_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_svelte_fragment_invalid_placement_from_root(source, root)
    {
        return Some(error);
    }
    if let Some(error) = template::detect_style_directive_invalid_modifier_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_snippet_shadowing_prop_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_text_content_model_errors_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) =
        template::detect_mixed_event_handler_syntax_from_root(source, root, runes_mode)
    {
        return Some(error);
    }
    if let Some(error) = template::detect_svelte_self_invalid_placement(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_dollar_prefix_invalid(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_global_reference_invalid_markup(source, root, runes_mode) {
        return Some(error);
    }
    if let Some(error) = template::detect_missing_directive_name(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_directive_invalid_value(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_empty_attribute_shorthand(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_attribute_syntax(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_attribute_invalid_name(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_duplicate_attributes(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_each_key_without_as(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_invalid_arguments_usage(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_debug_tag_invalid_arguments_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_reactive_declaration_cycle_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = template::detect_slot_attribute_errors_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) =
        template::detect_const_tag_errors_from_root(source, root, options.experimental.r#async)
    {
        return Some(error);
    }
    if let Some(error) = runes::detect_render_tag_errors_from_root(source, root) {
        return Some(error);
    }
    if !is_error_mode_warn(options)
        && let Some(error) = template::detect_bind_invalid_value_from_root(source, root, runes_mode)
    {
        return Some(error);
    }
    if let Some(error) =
        template::detect_additional_template_structure_errors_from_root(source, root)
    {
        return Some(error);
    }

    if let Some(error) = template::detect_parse_error_from_root(source, root, runes_mode) {
        return Some(error);
    }

    None
}

pub(crate) fn validate_component_css(source: &str, root: &Root) -> Option<CompileError> {
    if let Some(error) = css::detect_css_compiler_errors(source, root) {
        return Some(error);
    }
    if let Some(error) = css::detect_multiple_top_level_styles(source, root) {
        return Some(error);
    }
    None
}

pub(crate) fn validate_component_imports(source: &str, root: &Root) -> Option<CompileError> {
    if let Some(error) = imports::detect_import_svelte_internal_forbidden(source, root) {
        return Some(error);
    }
    if let Some(error) = imports::detect_export_rules_in_module_scripts(source, root) {
        return Some(error);
    }
    if let Some(error) = imports::detect_declaration_duplicate_module_import(source, root) {
        return Some(error);
    }
    None
}

pub(crate) fn validate_component_snippets(source: &str, root: &Root) -> Option<CompileError> {
    if let Some(error) = snippet::detect_malformed_snippet_headers(source, root) {
        return Some(error);
    }
    if let Some(error) = snippet::detect_snippet_parameter_assignment(source, root) {
        return Some(error);
    }
    if let Some(error) = snippet::detect_snippet_invalid_rest_parameter(source, root) {
        return Some(error);
    }
    if let Some(error) = snippet::detect_snippet_children_conflict(source, root) {
        return Some(error);
    }
    if let Some(error) = snippet::detect_snippet_invalid_export(source, root) {
        return Some(error);
    }
    if let Some(error) = snippet::detect_slot_snippet_conflict(source, root) {
        return Some(error);
    }
    None
}

pub(crate) fn validate_component_runes(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Option<CompileError> {
    if let Some(error) = runes::detect_store_invalid_subscription_component(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_store_invalid_scoped_subscription(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_dollar_prefix_invalid(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_state_invalid_placement_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_dollar_binding_error_component(source, options, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_invalid_rune_name_component(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_render_tag_errors_from_root(source, root) {
        return Some(error);
    }
    if let Some(error) = runes::detect_rune_missing_parentheses(source, root) {
        return Some(error);
    }

    let runes_mode = infer_runes_mode(options, root);
    if runes_mode {
        if let Some(error) = runes::detect_runes_mode_invalid_import(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_props_duplicate_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_legacy_export_invalid(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_each_item_invalid_assignment(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_props_illegal_name_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_bindable_invalid_arguments_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_rune_argument_count_errors_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_rune_invalid_spread_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_props_invalid_arguments_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_props_invalid_placement_component(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_bindable_invalid_location_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_derived_invalid_placement_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_effect_invalid_placement_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_host_invalid_placement_component(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_class_state_field_error_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_state_invalid_placement_general_from_root(source, root) {
            return Some(error);
        }
        if let Some(error) = runes::detect_state_in_each_header_from_root(source, root) {
            return Some(error);
        }
    }

    if !is_error_mode_warn(options)
        && let Some(error) = runes::detect_constant_assignment_component(source, root)
    {
        return Some(error);
    }
    None
}

pub(crate) fn validate_module_program(source: &str) -> Option<CompileError> {
    let program = parse_program(source)?;
    earliest([
        imports::detect_import_svelte_internal(source, &program),
        runes::detect_dollar_binding_error(source, &program, true),
        runes::detect_store_invalid_subscription_module(source, &program),
        runes::detect_constant_assignment(source, &program),
        runes::detect_bindable_invalid_location(source, &program),
        runes::detect_rune_argument_count(source, &program),
        runes::detect_props_invalid_placement_module(source, &program),
        runes::detect_state_invalid_placement(source, &program),
        runes::detect_derived_invalid_placement(source, &program),
        runes::detect_effect_invalid_placement(source, &program),
        runes::detect_host_invalid_placement(source, &program),
        runes::detect_class_state_field_error(source, &program),
        runes::detect_invalid_name(source, &program),
        runes::detect_renamed_effect_active(source, &program),
        imports::detect_export_rules(
            source,
            &program,
            &HashSet::new(),
            imports::ExportMode::Module,
        ),
    ])
}

fn earliest<const N: usize>(errors: [Option<CompileError>; N]) -> Option<CompileError> {
    errors.into_iter().flatten().min_by_key(error_start)
}

fn error_start(error: &CompileError) -> usize {
    error
        .position
        .as_deref()
        .map(|position| position.start)
        .unwrap_or(usize::MAX)
}

fn parse_program(source: &str) -> Option<crate::ast::modern::EstreeNode> {
    crate::compiler::phases::parse::parse_modern_program_content_with_offsets(
        source,
        0,
        1,
        0,
        1,
        source.len(),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::{validate_component_runes, validate_component_template, validate_module_program};
    use crate::api::CompileOptions;
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn validate_component(source: &str, runes: bool) -> Option<crate::error::CompileError> {
        let parsed = parse_component_for_compile(source).expect("parse component");
        let options = CompileOptions {
            runes: Some(runes),
            ..CompileOptions::default()
        };
        validate_component_template(source, &options, parsed.root())
            .or_else(|| validate_component_runes(source, &options, parsed.root()))
    }

    #[test]
    fn module_allows_default_export_expressions() {
        let error = validate_module_program("export default 42;");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn module_rejects_default_export_of_derived_state() {
        let error = validate_module_program("let total = $derived(count); export default total;")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "derived_invalid_export");
    }

    #[test]
    fn module_rejects_default_export_of_reassigned_state() {
        let error =
            validate_module_program("let count = $state(0); count = 1; export default count;")
                .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "state_invalid_export");
    }

    #[test]
    fn module_rejects_invalid_rune_names() {
        let error = validate_module_program("const state = $state.invalid(0);")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "rune_invalid_name");
    }

    #[test]
    fn module_rejects_renamed_effect_active() {
        let error = validate_module_program("const active = $effect.active();")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "rune_renamed");
    }

    #[test]
    fn module_rejects_store_subscriptions_with_module_diagnostic() {
        let error = validate_module_program("let count; console.log($count);")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "store_invalid_subscription_module");
    }

    #[test]
    fn module_rejects_duplicate_constructor_state_fields() {
        let source =
            "export class Counter { count = $state(0); constructor() { this.count = $state(0); } }";
        let error = validate_module_program(source).expect("expected validation error");
        assert_eq!(error.code.as_ref(), "state_field_duplicate");
    }

    #[test]
    fn module_rejects_assignment_before_constructor_state_field_declaration() {
        let source = "export class Counter { constructor() { if (true) this.count = -1; this.count = $state(0); } }";
        let error = validate_module_program(source).expect("expected validation error");
        assert_eq!(error.code.as_ref(), "state_field_invalid_assignment");
    }

    #[test]
    fn module_rejects_duplicate_class_field_before_constructor_state_field() {
        let source = "export class Counter { count = -1; static other() {} constructor() { this.count = $state(0); } }";
        let error = validate_module_program(source).expect("expected validation error");
        assert_eq!(error.code.as_ref(), "duplicate_class_field");
    }

    #[test]
    fn legacy_allows_dollar_props_and_rest_props_references() {
        let error = validate_component(
            "<script>let props = $$props;</script><div {...$$restProps}></div>",
            false,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn legacy_allows_dollar_slots_references() {
        let error = validate_component("{#if $$slots.default}<slot />{/if}", false);
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn legacy_allows_local_dollar_parameter_member_calls() {
        let error = validate_component(
            "<script>import { derived, writable } from 'svelte/store'; const checks = writable([false]); const count = derived(checks, ($checks) => $checks.filter(Boolean).length);</script>",
            false,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_store_backed_render_callee() {
        let error = validate_component(
            "<script>import { writable } from 'svelte/store'; let snippet = writable(hello);</script>{#snippet hello()}<p>hello world</p>{/snippet}{@render $snippet()}",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_props_id_call() {
        let error = validate_component("<script>let id = $props.id();</script>", true);
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn legacy_allows_dollar_labels() {
        let error = validate_component("<script>$: { break $; }</script>", false);
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_nested_state_calls_without_store_subscription_diagnostic() {
        let error = validate_component(
            "<script>function box(value) { let state = $state(value); return state; }</script>",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_each_item_property_binding() {
        let error = validate_component(
            "<script>let entries = $state([{ selected: 'a' }])</script>{#each entries as entry}<select bind:value={entry.selected}></select>{/each}",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_each_item_property_assignment() {
        let error = validate_component(
            "<script>let people = $state([{ name: { first: 'rob' } }]);</script>{#each people as person}<button onclick={() => { person.name.first = 'dave'; people = people; }}></button>{/each}",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_typescript_bind_targets_with_as_expressions() {
        let error = validate_component(
            "<script lang='ts'>let element = null; let with_state = $state({ foo: 1 });</script><div bind:this={element as HTMLElement}></div><input bind:value={(with_state as { foo: number }).foo} />",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_typescript_bind_targets_with_non_null_assertions() {
        let error = validate_component(
            "<script lang='ts'>let binding = $state(null);</script><input bind:value={binding!} />",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn module_allows_exporting_typescript_interface_bindings() {
        let error = validate_component(
            "<script module lang='ts'>interface Hello { message: 'hello'; } export type { Hello };</script>",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_shadowed_rune_like_parameters_and_store_calls() {
        let error = validate_component(
            "<script>import { writable } from 'svelte/store'; const state = writable((nr) => nr + 1); const effect = writable(() => 0); let foo = $state(0); function bar($derived, $effect) { const x = $derived(foo + 1); $effect(() => 0); return { get x() { return x + $derived(0) }, get y() { return $effect(() => 0); } } } const baz = bar($state, $effect);</script><p>{foo} {baz.x} {baz.y}</p>",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }
}
