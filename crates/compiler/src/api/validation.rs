use std::sync::Arc;

pub(super) use super::modern::expression_identifier_name;
use super::*;
use crate::ast::modern::Root;
use crate::compiler::phases::parse::ParsedModuleProgram;
use crate::error::CompilerDiagnosticKind;
use crate::{SourceId, SourceText};

mod css;
mod imports;
mod runes;
mod scope;
mod snippet;
mod template;

pub(crate) use self::scope::{
    ScopeStack, extend_name_set_with_expression_pattern_bindings,
    extend_name_set_with_optional_name, extend_name_set_with_oxc_pattern_bindings,
    scope_frame_for_each_block, scope_frame_for_snippet_block,
};
pub(super) use crate::names::{NameMark, NameSet, NameStack, OrderedNames};

pub(super) fn compile_error_with_range(
    source: &str,
    kind: CompilerDiagnosticKind,
    start: usize,
    end: usize,
) -> CompileError {
    kind.to_compile_error_in(SourceText::new(SourceId::new(0), source, None), start, end)
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
        && !(is_error_mode_warn(options) && error.code.as_ref() == "constant_binding")
    {
        return Some(error);
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
    // Check scoped store subscriptions before global reference check, since
    // `$x` where `x` is a scoped binding is a store error, not a global error.
    if let Some(error) = runes::detect_store_invalid_scoped_subscription(source, root) {
        return Some(error);
    }
    // `$` and `$$*` names are always illegal regardless of runes mode.
    // `$foo` store subscriptions in template expressions are only flagged in runes mode.
    // In legacy mode, `$foo` in templates is a valid store auto-subscription.
    if let Some(error) =
        runes::detect_global_reference_invalid_markup(source, root, runes_mode)
    {
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
    if !is_error_mode_warn(options)
        && let Some(error) = runes::detect_constant_assignment_in_scripts(source, root)
    {
        return Some(error);
    }
    let check_store_refs = options.runes != Some(false);
    if let Some(error) = runes::detect_global_reference_invalid_in_scripts(source, root, check_store_refs) {
        return Some(error);
    }
    None
}

pub(crate) fn validate_module_program(parsed: &ParsedModuleProgram<'_>) -> Option<CompileError> {
    let source = parsed.source_text();
    let program = parsed.program();
    earliest([
        imports::detect_import_svelte_internal(source.text, program),
        runes::detect_dollar_binding_error(source.text, program, true),
        runes::detect_store_invalid_subscription_module(source.text, program),
        runes::detect_constant_assignment(source.text, program),
        runes::detect_bindable_invalid_location(source.text, program),
        runes::detect_rune_argument_count(source.text, program),
        runes::detect_props_invalid_placement_module(source.text, program),
        runes::detect_state_invalid_placement(source.text, program),
        runes::detect_derived_invalid_placement(source.text, program),
        runes::detect_effect_invalid_placement(source.text, program),
        runes::detect_host_invalid_placement(source.text, program),
        runes::detect_class_state_field_error(source.text, program),
        runes::detect_invalid_name(source.text, program),
        runes::detect_renamed_effect_active(source.text, program),
        runes::detect_global_reference_invalid_module(source.text, program),
        imports::detect_export_rules(
            source.text,
            program,
            &NameSet::default(),
            imports::ExportMode::Module,
            0,
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

#[cfg(test)]
mod tests {
    use super::{validate_component_runes, validate_component_template, validate_module_program};
    use crate::compiler::phases::parse::{
        parse_component_for_compile, parse_module_program_for_compile_source,
    };
    use crate::{SourceId, SourceText, api::CompileOptions};
    use camino::Utf8Path;

    fn parsed_module(source: &str) -> crate::compiler::phases::parse::ParsedModuleProgram<'_> {
        parse_module_program_for_compile_source(SourceText::new(SourceId::new(0), source, None))
            .expect("parse module")
    }

    fn parsed_module_with_filename<'src>(
        source: &'src str,
        filename: &'src Utf8Path,
    ) -> crate::compiler::phases::parse::ParsedModuleProgram<'src> {
        parse_module_program_for_compile_source(SourceText::new(
            SourceId::new(0),
            source,
            Some(filename),
        ))
        .expect("parse module")
    }

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
        let error = validate_module_program(&parsed_module("export default 42;"));
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn module_rejects_default_export_of_derived_state() {
        let error = validate_module_program(&parsed_module(
            "let total = $derived(count); export default total;",
        ))
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "derived_invalid_export");
    }

    #[test]
    fn module_rejects_default_export_of_reassigned_state() {
        let error = validate_module_program(&parsed_module(
            "let count = $state(0); count = 1; export default count;",
        ))
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "state_invalid_export");
    }

    #[test]
    fn module_rejects_invalid_rune_names() {
        let error = validate_module_program(&parsed_module("const state = $state.invalid(0);"))
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "rune_invalid_name");
    }

    #[test]
    fn module_rejects_renamed_effect_active() {
        let error = validate_module_program(&parsed_module("const active = $effect.active();"))
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "rune_renamed");
    }

    #[test]
    fn module_rejects_store_subscriptions_with_module_diagnostic() {
        let error = validate_module_program(&parsed_module("let count; console.log($count);"))
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "store_invalid_subscription_module");
    }

    #[test]
    fn module_rejects_duplicate_constructor_state_fields() {
        let source =
            "export class Counter { count = $state(0); constructor() { this.count = $state(0); } }";
        let error =
            validate_module_program(&parsed_module(source)).expect("expected validation error");
        assert_eq!(error.code.as_ref(), "state_field_duplicate");
    }

    #[test]
    fn module_rejects_assignment_before_constructor_state_field_declaration() {
        let source = "export class Counter { constructor() { if (true) this.count = -1; this.count = $state(0); } }";
        let error =
            validate_module_program(&parsed_module(source)).expect("expected validation error");
        assert_eq!(error.code.as_ref(), "state_field_invalid_assignment");
    }

    #[test]
    fn module_rejects_duplicate_class_field_before_constructor_state_field() {
        let source = "export class Counter { count = -1; static other() {} constructor() { this.count = $state(0); } }";
        let error =
            validate_module_program(&parsed_module(source)).expect("expected validation error");
        assert_eq!(error.code.as_ref(), "duplicate_class_field");
    }

    #[test]
    fn module_allows_typescript_when_filename_is_svelte_ts() {
        let error = validate_module_program(&parsed_module_with_filename(
            "export function loadImage(src: string): string { return src; }",
            Utf8Path::new("image-loader.svelte.ts"),
        ));
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn module_allows_typescript_without_filename_when_program_requires_it() {
        let error = validate_module_program(&parsed_module(
            "export interface DragAndDropOptions { index: number; }",
        ));
        assert!(error.is_none(), "unexpected validation error: {error:?}");
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
    fn runes_allows_each_item_member_binding() {
        let error = validate_component(
            "<script>let items = $state([{ value: '' }]);</script>{#each items as item}<input bind:value={item.value} />{/each}",
            true,
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_destructuring_assignment_to_member_expressions() {
        let error = validate_component(
            "<script>const arr = [1, 2]; [arr[0], arr[1] = arr] = [arr[1], arr[0]];</script>{arr}",
            false,
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

    #[test]
    fn module_rejects_const_assignment() {
        let error = validate_module_program(&parsed_module("const a = $state(0); a += 1;"))
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "constant_assignment");
    }

    #[test]
    fn legacy_rejects_const_assignment_in_script() {
        let error = validate_component(
            "<script>const a = createCounter(); a += 1;</script>",
            false,
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "constant_assignment");
    }
}
